//! The session is a JWT in an HttpOnly cookie. Loco reads it back through the
//! `auth.jwt.location` setting, so the cookie name must match the config.
use loco_rs::prelude::{cookie::Cookie, *};

use crate::{data::settings::Settings, models::users};

pub const SESSION_COOKIE: &str = "auth_token";

/// Builds the session cookie for a freshly authenticated user.
///
/// # Errors
/// When the JWT config is missing or the token cannot be signed.
pub fn session_cookie(ctx: &AppContext, user: &users::Model) -> Result<Cookie<'static>> {
    let jwt = ctx.config.get_jwt_config()?;
    let token = user
        .generate_jwt(&jwt.secret, jwt.expiration)
        .or_else(|_| unauthorized("could not create session"))?;
    let max_age = i64::try_from(jwt.expiration).unwrap_or(i64::MAX);
    Ok(base_cookie(ctx, token)?
        .max_age(::cookie::time::Duration::seconds(max_age))
        .build())
}

/// A cookie that overwrites the session with an expired, empty value.
///
/// # Errors
/// When settings cannot be read.
pub fn cleared_session_cookie(ctx: &AppContext) -> Result<Cookie<'static>> {
    Ok(base_cookie(ctx, String::new())?
        .max_age(::cookie::time::Duration::ZERO)
        .build())
}

fn base_cookie(ctx: &AppContext, value: String) -> Result<::cookie::CookieBuilder<'static>> {
    let settings = Settings::from_context(ctx)?;
    Ok(Cookie::build((SESSION_COOKIE, value))
        .path("/")
        .http_only(true)
        .secure(settings.secure_cookies)
        .same_site(cookie::SameSite::Lax))
}

/// Sends the browser to `to`. HTMX requests get an `HX-Redirect` header instead,
/// because HTMX would otherwise swap the redirected page into the current one.
#[must_use]
pub fn redirect_response(headers: &axum::http::HeaderMap, to: &str) -> Response {
    if headers.contains_key("hx-request") {
        return (
            axum::http::StatusCode::OK,
            [("HX-Redirect", to.to_string())],
        )
            .into_response();
    }
    (
        axum::http::StatusCode::SEE_OTHER,
        [(axum::http::header::LOCATION, to.to_string())],
    )
        .into_response()
}
