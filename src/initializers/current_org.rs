//! Clears a stale `current_org` cookie.
//!
//! `CurrentMember` decides whether the cookie still points at an organisation
//! the person is active in, but an extractor cannot set cookies. So this layer
//! puts a [`StaleCurrentOrg`] flag in every request, and when `CurrentMember`
//! raises it, adds a clearing `Set-Cookie` to the response, whatever it is.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use async_trait::async_trait;
use axum::{
    extract::Request,
    http::{header::SET_COOKIE, HeaderValue},
    middleware::{self, Next},
    Router as AxumRouter,
};
use loco_rs::{
    app::{AppContext, Initializer},
    Error, Result,
};

use crate::extractors::session::cleared_current_org_cookie;

/// Raised by `CurrentMember` when the request's `current_org` cookie did not
/// lead to an active membership.
#[derive(Clone, Default)]
pub struct StaleCurrentOrg(Arc<AtomicBool>);

impl StaleCurrentOrg {
    pub fn raise(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    fn is_raised(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

pub struct CurrentOrgInitializer;

#[async_trait]
impl Initializer for CurrentOrgInitializer {
    fn name(&self) -> String {
        "current-org".to_string()
    }

    async fn after_routes(&self, router: AxumRouter, ctx: &AppContext) -> Result<AxumRouter> {
        let clear = HeaderValue::from_str(&cleared_current_org_cookie(ctx)?.to_string())
            .map_err(|_| Error::string("invalid cookie header"))?;
        Ok(
            router.layer(middleware::from_fn(move |mut req: Request, next: Next| {
                let clear = clear.clone();
                async move {
                    let stale = StaleCurrentOrg::default();
                    req.extensions_mut().insert(stale.clone());
                    let mut response = next.run(req).await;
                    if stale.is_raised() {
                        response.headers_mut().append(SET_COOKIE, clear);
                    }
                    response
                }
            })),
        )
    }
}
