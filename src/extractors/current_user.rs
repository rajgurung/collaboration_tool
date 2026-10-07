use axum::{extract::FromRequestParts, http::request::Parts};
use loco_rs::{controller::extractor::auth::extract_jwt_from_request_parts, prelude::*};

use super::session::redirect_response;
use crate::models::users;

/// The signed-in user. Handlers that take this never run for anonymous
/// requests: those are redirected to `/login`.
pub struct CurrentUser(pub users::Model);

impl FromRequestParts<AppContext> for CurrentUser {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        ctx: &AppContext,
    ) -> std::result::Result<Self, Self::Rejection> {
        let login = || redirect_response(&parts.headers, "/login");
        let Ok(jwt) = extract_jwt_from_request_parts(parts, ctx) else {
            return Err(login());
        };
        match users::Model::find_by_pid(&ctx.db, &jwt.claims.pid).await {
            Ok(user) => Ok(Self(user)),
            Err(_) => Err(login()),
        }
    }
}

/// `Option<CurrentUser>`: `None` for anonymous requests instead of a redirect.
impl axum::extract::OptionalFromRequestParts<AppContext> for CurrentUser {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        ctx: &AppContext,
    ) -> std::result::Result<Option<Self>, Self::Rejection> {
        Ok(
            <Self as FromRequestParts<AppContext>>::from_request_parts(parts, ctx)
                .await
                .ok(),
        )
    }
}
