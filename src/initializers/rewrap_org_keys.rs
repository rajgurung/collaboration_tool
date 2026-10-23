//! At boot, after migrations: during a KEK rotation (`SECRETS_KEK_PREVIOUS`
//! set) moves every organisation's data key onto the new KEK, then unwraps
//! every data key once. Any failure stops the boot. Railway keeps the old
//! deploy serving when a new one fails its health check, so a wrong or
//! missing KEK never reaches users as unreadable passwords.
use async_trait::async_trait;
use loco_rs::{
    app::{AppContext, Initializer},
    environment::Environment,
    Error, Result,
};
use sea_orm::DatabaseConnection;

use crate::models::{org_keys, secrets::crypto::Kek};

pub struct RewrapOrgKeysInitializer;

#[async_trait]
impl Initializer for RewrapOrgKeysInitializer {
    fn name(&self) -> String {
        "rewrap-org-keys".to_string()
    }

    async fn before_run(&self, ctx: &AppContext) -> Result<()> {
        // Each test boot sees the last test's keys; tests call `check_keys`.
        if ctx.environment == Environment::Test {
            return Ok(());
        }
        let kek = Kek::get(ctx)?;
        check_keys(&ctx.db, &kek).await.inspect_err(|err| {
            tracing::error!(error = %err, "organisation keys failed the boot check");
        })
    }
}

/// Rewraps (during a rotation), then checks every organisation key opens
/// with the loaded KEKs. Logs counts and versions only.
///
/// # Errors
/// When the rewrap fails (and was rolled back), or any key does not open.
pub async fn check_keys(db: &DatabaseConnection, kek: &Kek) -> Result<()> {
    let rewrapped = org_keys::Model::rewrap(db, kek).await.map_err(|err| {
        Error::Message(format!(
            "rewrapping organisation keys failed and was rolled back: {err}"
        ))
    })?;
    if let Some(done) = rewrapped {
        tracing::info!("{}", rewrapped_line(&done));
    }
    let checked = org_keys::Model::check_all(db, kek)
        .await
        .map_err(|err| Error::Message(err.to_string()))?;
    tracing::info!("all {checked} organisation keys unwrap with the loaded KEKs");
    Ok(())
}

/// The line the boot log and `cargo loco task rewrap_org_keys` both print.
#[must_use]
pub fn rewrapped_line(done: &org_keys::Rewrapped) -> String {
    format!(
        "rewrapped {} organisation keys from v{} to v{}; {} left on v{}",
        done.rewrapped, done.from, done.to, done.left, done.from
    )
}
