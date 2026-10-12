//! Where a request came from, for the login activity log and rate limits.
use axum::{
    extract::FromRequestParts,
    http::{header::USER_AGENT, request::Parts},
};
use loco_rs::prelude::*;

use crate::models::login_events::Client;

/// The address limits are counted against. Requests without one (never
/// expected behind Railway) share a single bucket.
#[must_use]
pub fn client_ip(remote: RemoteIP) -> String {
    match remote {
        RemoteIP::Forwarded(ip) | RemoteIP::Socket(ip) => ip.to_string(),
        RemoteIP::None => "unknown".to_string(),
    }
}

impl FromRequestParts<AppContext> for Client {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        ctx: &AppContext,
    ) -> std::result::Result<Self, Self::Rejection> {
        let remote = RemoteIP::from_request_parts(parts, ctx)
            .await
            .map_err(IntoResponse::into_response)?;
        Ok(Self {
            ip: client_ip(remote),
            user_agent: parts
                .headers
                .get(USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string(),
        })
    }
}
