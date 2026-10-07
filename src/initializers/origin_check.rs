//! CSRF defence for cookie-authenticated form posts.
//!
//! The session cookie is `SameSite=Lax`, so browsers already leave it off
//! cross-site POSTs. As a second layer, any state-changing request that carries
//! an `Origin` header must come from our own `app_url`. Browsers always send
//! `Origin` on POST, so a forged cross-site form is rejected here. Requests with
//! no `Origin` (curl, tests) carry no browser cookies to abuse and pass through.
use async_trait::async_trait;
use axum::{
    extract::Request,
    http::{header::ORIGIN, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Router as AxumRouter,
};
use loco_rs::{
    app::{AppContext, Initializer},
    Result,
};

use crate::data::settings::Settings;

pub struct OriginCheckInitializer;

#[async_trait]
impl Initializer for OriginCheckInitializer {
    fn name(&self) -> String {
        "origin-check".to_string()
    }

    async fn after_routes(&self, router: AxumRouter, ctx: &AppContext) -> Result<AxumRouter> {
        let allowed = Settings::from_context(ctx)?.app_url;
        Ok(
            router.layer(middleware::from_fn(move |req: Request, next: Next| {
                let allowed = allowed.clone();
                async move { check(&allowed, req, next).await }
            })),
        )
    }
}

async fn check(allowed: &str, req: Request, next: Next) -> Response {
    let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if !safe {
        if let Some(origin) = req.headers().get(ORIGIN) {
            if origin.to_str().map_or(true, |o| o != allowed) {
                tracing::warn!(?origin, allowed, "blocked cross-origin request");
                return (StatusCode::FORBIDDEN, "Cross-site request blocked").into_response();
            }
        }
    }
    next.run(req).await
}
