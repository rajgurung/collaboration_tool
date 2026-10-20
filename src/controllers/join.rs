//! The join link, `/join/{slug}`. Signed out it makes a new account with a
//! pending membership. Signed in it asks to join with the account the
//! person already has. Every response here is `guarded`: the "Ask to join"
//! button must not be clickjacked, and the pages are never cached.
use axum::http::HeaderMap;
use loco_rs::prelude::*;
use serde::Serialize;

use crate::{
    extractors::{
        current_member::{ACTING_ORG_COOKIE, CURRENT_ORG_COOKIE},
        current_user::CurrentUser,
        session::{append_cookie, cleared_acting_org_cookie, redirect_response, signed_in_cookies},
    },
    models::{
        memberships::{self, History, JoinOutcome, JoinParams},
        organisations,
        users::{self, RegisterParams},
    },
    views::{
        forms::{field_errors, FieldErrors},
        headers::guarded,
        time,
    },
};

#[derive(Debug, Default, Serialize)]
struct FormValues<'a> {
    name: &'a str,
    email: &'a str,
}

/// Signed out: the new-account form, with "Sign in to join". Signed in:
/// the person's own membership of this org, or the form to ask. It only
/// reads, so a link or a prefetch can never file a request.
#[debug_handler]
async fn new(
    user: Option<CurrentUser>,
    ViewEngine(v): ViewEngine<TeraView>,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Result<Response> {
    let Ok(org) = organisations::Model::find_by_slug(&ctx.db, &slug).await else {
        return not_found_page(&v);
    };
    match user {
        Some(CurrentUser(user)) => {
            let page = Page {
                ctx: &ctx,
                v: &v,
                headers: &headers,
                org: &org,
                user: &user,
            };
            page.render(None, &FieldErrors::new()).await
        }
        None => render_form(&v, &org, &FormValues::default(), &FieldErrors::new(), 200),
    }
}

#[debug_handler]
async fn create(
    ViewEngine(v): ViewEngine<TeraView>,
    State(ctx): State<AppContext>,
    Path(slug): Path<String>,
    Form(params): Form<RegisterParams>,
) -> Result<Response> {
    let Ok(org) = organisations::Model::find_by_slug(&ctx.db, &slug).await else {
        return not_found_page(&v);
    };
    let values = FormValues {
        name: &params.name,
        email: &params.email,
    };
    match memberships::Model::join_new_account(&ctx.db, &org, &params).await {
        Ok((user, _membership)) => format::render()
            .cookies(&signed_in_cookies(&ctx, &user)?)?
            .redirect("/dashboard"),
        Err(ModelError::EntityAlreadyExists) => {
            // The new-account form never touches an existing account. The
            // page links to signing in, which comes back here.
            let errors = FieldErrors::from([(
                "email".to_string(),
                format!(
                    "An account with this email already exists. Sign in to join {}.",
                    org.name
                ),
            )]);
            render_form(&v, &org, &values, &errors, 422)
        }
        Err(err) => match field_errors(&err) {
            Some(errors) => render_form(&v, &org, &values, &errors, 422),
            None => Err(err.into()),
        },
    }
}

/// "Ask to join" from a signed-in account. `CurrentUser`, not
/// `CurrentMember`: it acts on the account, never on the org this browser
/// is in, so there is no tab claim to check, and it never switches org. A
/// super admin who was acting stops acting.
#[debug_handler]
async fn request(
    CurrentUser(user): CurrentUser,
    ViewEngine(v): ViewEngine<TeraView>,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    Form(params): Form<JoinParams>,
) -> Result<Response> {
    let Ok(org) = organisations::Model::find_by_slug(&ctx.db, &slug).await else {
        return not_found_page(&v);
    };
    match memberships::Model::request_to_join(&ctx.db, &org, user.id, &params).await {
        Ok(JoinOutcome::Requested(_) | JoinOutcome::AlreadyPending(_)) => {
            tracing::info!(user_id = user.id, org_id = org.id, "requested to join");
            let mut response =
                redirect_response(&headers, &format!("/dashboard?requested={}", org.id));
            if cookie(&headers, ACTING_ORG_COOKIE).is_some() {
                append_cookie(&mut response, &cleared_acting_org_cookie(&ctx)?)?;
            }
            Ok(guarded(response))
        }
        Ok(JoinOutcome::AlreadyMember(_)) => Ok(guarded(redirect_response(
            &headers,
            &format!("/join/{}", org.slug),
        ))),
        Err(err) => {
            let errors = field_errors(&err).ok_or(err)?;
            let page = Page {
                ctx: &ctx,
                v: &v,
                headers: &headers,
                org: &org,
                user: &user,
            };
            page.render(Some(&params.name), &errors).await
        }
    }
}

/// The value of a request cookie.
fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    cookie::CookieJar::from_headers(headers)
        .get(name)
        .map(|c| c.value().to_string())
}

/// The signed-in join page for `user`, who is always the signed-in person.
struct Page<'a> {
    ctx: &'a AppContext,
    v: &'a TeraView,
    headers: &'a HeaderMap,
    org: &'a organisations::Model,
    user: &'a users::Model,
}

impl Page<'_> {
    /// The org this browser is working in, if the person is active there:
    /// the `current_org` cookie, else their default membership, as
    /// `CurrentMember` picks it (acting aside).
    async fn browser_org(&self) -> Result<Option<memberships::Model>> {
        let db = &self.ctx.db;
        let chosen = cookie(self.headers, CURRENT_ORG_COOKIE).and_then(|v| v.parse().ok());
        if let Some(org_id) = chosen {
            if let Some(m) = memberships::Model::active_in(db, org_id, self.user.id).await? {
                return Ok(Some(m));
            }
        }
        Ok(memberships::Model::default_for_user(db, self.user.id)
            .await?
            .filter(memberships::Model::is_active))
    }

    async fn org_name(&self, id: i64) -> Option<String> {
        organisations::Model::find_by_id(&self.ctx.db, id)
            .await
            .ok()
            .map(|o| o.name)
    }

    /// The page in the state of the person's own membership here: active,
    /// waiting, declined or removed (the form with a note), or none (the
    /// form). `typed` and `errors` redraw it after a refused ask.
    async fn render(&self, typed: Option<&str>, errors: &FieldErrors) -> Result<Response> {
        let (org, user) = (self.org, self.user);
        let mine = memberships::Model::in_org_for_user(&self.ctx.db, org.id, user.id).await?;
        let here = self.browser_org().await?;
        let state = match &mine {
            Some(m) if m.is_active() => {
                if here.as_ref().is_some_and(|h| h.organisation_id == org.id) {
                    return Ok(guarded(redirect_response(self.headers, "/dashboard")));
                }
                "active"
            }
            Some(m) if m.is_pending() => "waiting",
            _ => "form",
        };
        let tz = time::zone(&org.timezone);
        let note = mine
            .as_ref()
            .and_then(memberships::Model::history)
            .map(|h| match h {
                History::Removed(at) => format!(
                    "You were removed from {} on {}. You can ask to join again.",
                    org.name,
                    time::day(at, tz)
                ),
                History::Declined(at) => format!(
                    "Your last request was declined on {}. You can ask again.",
                    time::day(at, tz)
                ),
                History::DeclinedBefore => {
                    "Your last request was declined. You can ask again.".to_string()
                }
            });
        // Their name in the org this browser is in, else in their default
        // membership, else their old one here.
        let default = memberships::Model::default_for_user(&self.ctx.db, user.id).await?;
        let username = typed.map_or_else(
            || {
                here.as_ref()
                    .or(default.as_ref())
                    .or(mine.as_ref())
                    .map_or_else(|| user.name.clone(), |m| m.username.clone())
            },
            str::to_string,
        );
        let acting_id = cookie(self.headers, ACTING_ORG_COOKIE).and_then(|v| v.parse().ok());
        let acting = match acting_id {
            Some(id) if user.is_super_admin => self.org_name(id).await,
            _ => None,
        };
        // While acting, this browser is in someone else's org, so the page
        // speaks of "your other organisations" instead of naming one.
        let from = match &here {
            Some(m) if acting.is_none() => self.org_name(m.organisation_id).await,
            _ => None,
        };
        let status = if errors.is_empty() { 200 } else { 422 };
        Ok(guarded(format::render().status(status).view(
            self.v,
            "join/existing.html",
            data!({
                "org": { "id": org.id, "name": org.name, "slug": org.slug },
                "state": state,
                "account": { "email": user.email, "name": user.name },
                // A pending row's last change is the ask itself.
                "mine": mine.as_ref().map(|m| data!({
                    "username": m.username,
                    "asked_on": time::day(m.updated_at, tz),
                })),
                "note": note,
                "from": from,
                "acting": acting,
                "form": { "name": username },
                "errors": errors,
            }),
        )?))
    }
}

fn render_form(
    v: &TeraView,
    org: &organisations::Model,
    values: &FormValues,
    errors: &FieldErrors,
    status: u16,
) -> Result<Response> {
    Ok(guarded(format::render().status(status).view(
        v,
        "join/new.html",
        data!({
            "org": { "name": org.name, "slug": org.slug },
            "form": values,
            "errors": errors,
        }),
    )?))
}

fn not_found_page(v: &TeraView) -> Result<Response> {
    Ok(guarded(format::render().status(404).view(
        v,
        "errors/not_found_public.html",
        data!({}),
    )?))
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/join/{slug}", get(new))
        .add("/join/{slug}", post(create))
        .add("/join/{slug}/request", post(request))
}
