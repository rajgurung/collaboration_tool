use axum::http::HeaderMap;
use loco_rs::{hash, prelude::*};
use serde::Deserialize;

use crate::{
    extractors::session::{
        cleared_acting_org_cookie, cleared_session_cookie, redirect_response, session_cookie,
    },
    mailers::auth::AuthMailer,
    models::users,
    views::forms::field_errors,
};

/// Hash of a throwaway password. Checked when an email is unknown so a failed
/// login takes the same time whether or not the account exists.
const DUMMY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$ETQBx4rTgNAZhSaeYZKOZg$eYTdH26CRT6nUJtacLDEboP0li6xUwUF/q5nSlQ8uuc";

#[derive(Debug, Deserialize)]
pub struct LoginForm {
    pub email: String,
    pub password: String,
    /// Where to go after signing in; see [`safe_next`].
    #[serde(default)]
    pub next: String,
}

#[derive(Debug, Deserialize)]
pub struct ForgotForm {
    pub email: String,
}

#[derive(Debug, Deserialize)]
pub struct ResetForm {
    pub password: String,
    pub password_confirmation: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginQuery {
    pub reset: Option<String>,
    pub next: Option<String>,
}

/// Only the Claude "Allow access" page sends people to log in and back, so
/// `next` must be that page. Anything else (other paths, `//host`, absolute
/// URLs, backslashes or line breaks, raw or percent-encoded) is dropped.
#[must_use]
pub fn safe_next(next: &str) -> Option<&str> {
    let lower = next.to_ascii_lowercase();
    let sneaky = next.contains(['\\', '\r', '\n'])
        || ["%5c", "%0d", "%0a"].iter().any(|s| lower.contains(s));
    (next.starts_with("/oauth/authorize?") && !sneaky).then_some(next)
}

#[debug_handler]
async fn login_page(
    ViewEngine(v): ViewEngine<TeraView>,
    Query(query): Query<LoginQuery>,
) -> Result<Response> {
    let notice = query
        .reset
        .map(|_| "Your password has been changed. Sign in with the new one.");
    let next = query.next.as_deref().and_then(safe_next);
    format::render().view(
        &v,
        "auth/login.html",
        data!({ "email": "", "notice": notice, "next": next }),
    )
}

#[debug_handler]
async fn login(
    ViewEngine(v): ViewEngine<TeraView>,
    State(ctx): State<AppContext>,
    Form(form): Form<LoginForm>,
) -> Result<Response> {
    let user = match users::Model::find_by_email(&ctx.db, &form.email).await {
        Ok(user) if user.verify_password(&form.password) => user,
        Ok(_) => return login_failed(&v, &form),
        Err(_) => {
            let _ = hash::verify_password(&form.password, DUMMY_HASH);
            return login_failed(&v, &form);
        }
    };
    format::render()
        .cookies(&[session_cookie(&ctx, &user)?])?
        .redirect(safe_next(&form.next).unwrap_or("/"))
}

fn login_failed(v: &TeraView, form: &LoginForm) -> Result<Response> {
    format::render().status(422).view(
        v,
        "auth/login.html",
        data!({
            "email": form.email,
            "next": safe_next(&form.next),
            "error": "Email or password is incorrect.",
        }),
    )
}

#[debug_handler]
async fn logout(State(ctx): State<AppContext>, headers: HeaderMap) -> Result<Response> {
    let mut response = redirect_response(&headers, "/login");
    for cookie in [
        cleared_session_cookie(&ctx)?,
        cleared_acting_org_cookie(&ctx)?,
    ] {
        response.headers_mut().append(
            axum::http::header::SET_COOKIE,
            cookie
                .to_string()
                .parse()
                .map_err(|_| Error::string("invalid cookie header"))?,
        );
    }
    Ok(response)
}

#[debug_handler]
async fn forgot_page(ViewEngine(v): ViewEngine<TeraView>) -> Result<Response> {
    format::render().view(&v, "auth/forgot.html", data!({ "email": "" }))
}

/// Always shows the same confirmation, so the form cannot be used to find out
/// which emails have accounts.
#[debug_handler]
async fn forgot(
    ViewEngine(v): ViewEngine<TeraView>,
    State(ctx): State<AppContext>,
    Form(form): Form<ForgotForm>,
) -> Result<Response> {
    if let Ok(user) = users::Model::find_by_email(&ctx.db, &form.email).await {
        let user = user
            .into_active_model()
            .set_forgot_password_sent(&ctx.db)
            .await?;
        AuthMailer::forgot_password(&ctx, &user).await?;
    } else {
        tracing::debug!("password reset requested for unknown email");
    }
    format::render().view(&v, "auth/forgot_sent.html", data!({}))
}

#[debug_handler]
async fn reset_page(
    ViewEngine(v): ViewEngine<TeraView>,
    State(ctx): State<AppContext>,
    Path(token): Path<String>,
) -> Result<Response> {
    if users::Model::find_by_reset_token(&ctx.db, &token)
        .await
        .is_err()
    {
        return reset_invalid(&v);
    }
    format::render().view(&v, "auth/reset.html", data!({ "token": token }))
}

#[debug_handler]
async fn reset(
    ViewEngine(v): ViewEngine<TeraView>,
    State(ctx): State<AppContext>,
    Path(token): Path<String>,
    Form(form): Form<ResetForm>,
) -> Result<Response> {
    let Ok(user) = users::Model::find_by_reset_token(&ctx.db, &token).await else {
        return reset_invalid(&v);
    };
    if form.password != form.password_confirmation {
        return reset_form_error(&v, &token, "The two passwords do not match.");
    }
    match user
        .into_active_model()
        .reset_password(&ctx.db, &form.password)
        .await
    {
        Ok(_) => format::redirect("/login?reset=1"),
        Err(err) => {
            let message = field_errors(&err)
                .and_then(|errors| errors.into_values().next())
                .ok_or(err)?;
            reset_form_error(&v, &token, &message)
        }
    }
}

fn reset_form_error(v: &TeraView, token: &str, message: &str) -> Result<Response> {
    format::render().status(422).view(
        v,
        "auth/reset.html",
        data!({ "token": token, "error": message }),
    )
}

fn reset_invalid(v: &TeraView) -> Result<Response> {
    format::render()
        .status(404)
        .view(v, "auth/reset_invalid.html", data!({}))
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/login", get(login_page))
        .add("/login", post(login))
        .add("/logout", post(logout))
        .add("/forgot", get(forgot_page))
        .add("/forgot", post(forgot))
        .add("/reset/{token}", get(reset_page))
        .add("/reset/{token}", post(reset))
}

#[cfg(test)]
mod tests {
    use super::safe_next;

    #[test]
    fn next_only_returns_to_the_allow_access_page() {
        assert_eq!(
            safe_next("/oauth/authorize?client_id=a&state=b"),
            Some("/oauth/authorize?client_id=a&state=b")
        );
        for bad in [
            "",
            "/",
            "/dashboard",
            "//evil.example",
            "/\\evil.example",
            "/%5Cevil.example",
            "https://evil.example/oauth/authorize?x=1",
            "/oauth/authorize",
            "/oauth/authorizeX?x=1",
            "/oauth/authorize?x=%0d%0aSet-Cookie:a=b",
            "/oauth/authorize?x=\\y",
        ] {
            assert_eq!(safe_next(bad), None, "{bad}");
        }
    }
}
