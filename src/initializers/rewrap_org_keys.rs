//! During a KEK rotation (`SECRETS_KEK_PREVIOUS` set), moves every
//! organisation's data key onto the new KEK at boot, after migrations. Later
//! boots find nothing left. A failure is logged, not fatal: the previous key
//! still reads the rows that were not moved.
use async_trait::async_trait;
use loco_rs::{
    app::{AppContext, Initializer},
    environment::Environment,
    Result,
};

use crate::models::{org_keys, secrets::crypto::Kek};

pub struct RewrapOrgKeysInitializer;

#[async_trait]
impl Initializer for RewrapOrgKeysInitializer {
    fn name(&self) -> String {
        "rewrap-org-keys".to_string()
    }

    async fn before_run(&self, ctx: &AppContext) -> Result<()> {
        // Tests call `rewrap` directly instead.
        if ctx.environment == Environment::Test {
            return Ok(());
        }
        let kek = Kek::get(ctx)?;
        match org_keys::Model::rewrap(&ctx.db, &kek).await {
            Ok(Some(done)) => tracing::info!("{}", rewrapped_line(&done)),
            Ok(None) => {}
            Err(err) => tracing::error!(
                error = %err,
                "rewrapping organisation keys failed and was rolled back; they still read through SECRETS_KEK_PREVIOUS"
            ),
        }
        match org_keys::Model::count_by_kek_version(&ctx.db).await {
            Ok(counts) => {
                for (version, count) in counts {
                    if !kek.has_version(version) {
                        tracing::error!(
                            "{count} organisation keys are on KEK v{version}, which is not loaded; their logins cannot be read"
                        );
                    }
                }
            }
            Err(err) => tracing::error!(error = %err, "could not count organisation keys"),
        }
        Ok(())
    }
}

/// The line the boot log and `cargo loco task rewrap_org_keys` both print.
#[must_use]
pub fn rewrapped_line(done: &org_keys::Rewrapped) -> String {
    format!(
        "rewrapped {} organisation keys from v{} to v{}; {} left on v{}",
        done.rewrapped, done.from, done.to, done.left, done.from
    )
}
