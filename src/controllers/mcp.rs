use std::sync::Arc;

use axum::{extract::Request, middleware::Next};
use loco_rs::prelude::*;
use rmcp::transport::streamable_http_server::{
    session::never::NeverSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};

use crate::mcp::{CollabServer, Marker};

async fn mark(mut req: Request, next: Next) -> Response {
    req.extensions_mut().insert(Marker("raj".to_string()));
    next.run(req).await
}

pub fn routes(ctx: &AppContext) -> Routes {
    let ctx = ctx.clone();
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true);
    let svc = StreamableHttpService::new(
        move || Ok(CollabServer::new(ctx.clone())),
        Arc::new(NeverSessionManager::default()),
        config,
    );
    Routes::new().add(
        "/mcp",
        axum::routing::post_service(svc).route_layer(axum::middleware::from_fn(mark)),
    )
}
