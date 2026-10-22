//! On the first start after the release that made Claude's writes name their
//! organisation, puts one notice in the bell of everyone with a Claude
//! connection. Later starts find nothing left to announce.
use async_trait::async_trait;
use loco_rs::{
    app::{AppContext, Initializer},
    environment::Environment,
    Result,
};

use crate::models::claude_update_notices;

pub struct ClaudeUpdateInitializer;

#[async_trait]
impl Initializer for ClaudeUpdateInitializer {
    fn name(&self) -> String {
        "claude-update".to_string()
    }

    async fn before_run(&self, ctx: &AppContext) -> Result<()> {
        // Tests call `announce` directly instead.
        if ctx.environment == Environment::Test {
            return Ok(());
        }
        // A failure is logged, not fatal: the notice isn't worth a failed boot.
        match claude_update_notices::Model::announce(&ctx.db).await {
            Ok(told) if told.is_empty() => {}
            Ok(told) => tracing::info!(told = told.len(), "announced the Claude connector update"),
            Err(err) => tracing::error!(error = %err, "Claude connector update notice failed"),
        }
        Ok(())
    }
}
