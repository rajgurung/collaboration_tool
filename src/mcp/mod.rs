use axum::http::request::Parts;
use loco_rs::app::AppContext;
use rmcp::{
    handler::server::{router::tool::ToolRouter, tool::Extension},
    model::{ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router, ServerHandler,
};

#[derive(Clone)]
pub struct Marker(pub String);

#[derive(Clone)]
pub struct CollabServer {
    #[allow(dead_code)]
    ctx: AppContext,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl CollabServer {
    #[must_use]
    pub fn new(ctx: AppContext) -> Self {
        Self {
            ctx,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl CollabServer {
    #[tool(description = "spike")]
    async fn whoami(&self, Extension(parts): Extension<Parts>) -> String {
        parts
            .extensions
            .get::<Marker>()
            .map_or_else(|| "none".to_string(), |m| m.0.clone())
    }
}

#[tool_handler]
impl ServerHandler for CollabServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
}
