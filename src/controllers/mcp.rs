//! `POST /mcp`: the MCP server Claude talks to. It is a normal Loco route, so
//! Loco's middleware and the Origin check run first, then the bearer gate.
//! GET and DELETE answer 405: the server is stateless and never streams.
use std::sync::Arc;

use loco_rs::{environment::Environment, prelude::*};
use rmcp::transport::streamable_http_server::{
    session::never::NeverSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};

use crate::{data::settings::Settings, extractors::bearer::require_bearer, mcp::CollabServer};

/// Stateless JSON responses. Browsers from other sites are refused by Origin.
fn config(ctx: &AppContext) -> StreamableHttpServerConfig {
    let local = matches!(
        ctx.environment,
        Environment::Development | Environment::Test
    );
    let mut config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None);
    // rmcp's Host check defends local servers against DNS rebinding, so it
    // stays on in development and test. In production every call needs a
    // bearer token and the Origin check still runs, so it adds little, while a
    // Host forwarded differently by Railway or Cloudflare would break every
    // tool call.
    if !local {
        config = config.disable_allowed_hosts();
    }
    let Some(app_url) = Settings::from_context(ctx)
        .ok()
        .and_then(|s| url::Url::parse(&s.app_url).ok())
    else {
        tracing::error!("settings.app_url is missing or invalid");
        return config;
    };
    let Some(host) = app_url.host_str() else {
        return config;
    };
    if local {
        config = config.with_allowed_hosts([host, "localhost", "127.0.0.1"]);
    }
    let port = app_url.port_or_known_default().unwrap_or(443);
    config.with_allowed_origins([format!("{}://{host}:{port}", app_url.scheme())])
}

pub fn routes(ctx: &AppContext) -> Routes {
    let server_ctx = ctx.clone();
    let service = StreamableHttpService::new(
        move || Ok(CollabServer::new(server_ctx.clone())),
        Arc::new(NeverSessionManager::default()),
        config(ctx),
    );
    Routes::new().add(
        "/mcp",
        axum::routing::post_service(service).route_layer(axum::middleware::from_fn_with_state(
            ctx.clone(),
            require_bearer,
        )),
    )
}
