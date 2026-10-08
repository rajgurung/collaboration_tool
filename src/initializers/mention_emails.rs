//! Checks every minute for @mentions left unread and emails them. Which
//! mentions were emailed is kept in the database, so restarts are safe. Like
//! the live hubs, this assumes a single app instance.
use std::time::Duration;

use async_trait::async_trait;
use loco_rs::{
    app::{AppContext, Initializer},
    environment::Environment,
    Result,
};

use crate::mailers::mentions;

pub struct MentionEmailsInitializer;

#[async_trait]
impl Initializer for MentionEmailsInitializer {
    fn name(&self) -> String {
        "mention-emails".to_string()
    }

    async fn before_run(&self, ctx: &AppContext) -> Result<()> {
        // Tests call `mentions::send_due` directly instead.
        if ctx.environment == Environment::Test {
            return Ok(());
        }
        let ctx = ctx.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(60));
            loop {
                every.tick().await;
                match mentions::send_due(&ctx).await {
                    Ok(0) => {}
                    Ok(sent) => tracing::info!(sent, "emailed unread mentions"),
                    Err(err) => tracing::error!(error = %err, "mention emails failed"),
                }
            }
        });
        Ok(())
    }
}
