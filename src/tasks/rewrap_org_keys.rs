use loco_rs::prelude::*;

use crate::{
    initializers::rewrap_org_keys::rewrapped_line,
    models::{org_keys, secrets::crypto::Kek},
};

/// Moves organisation keys from `SECRETS_KEK_PREVIOUS` onto `SECRETS_KEK`,
/// as boot does, or with `dry_run:true` only counts them.
pub struct RewrapOrgKeys;

#[async_trait]
impl Task for RewrapOrgKeys {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "rewrap_org_keys".to_string(),
            detail: "Rewrap organisation keys from SECRETS_KEK_PREVIOUS onto SECRETS_KEK.\nPrints counts only.\nUsage: cargo loco task rewrap_org_keys [dry_run:true]".to_string(),
        }
    }

    async fn run(&self, ctx: &AppContext, vars: &task::Vars) -> Result<()> {
        let kek = Kek::get(ctx)?;
        if vars.cli_arg("dry_run").is_ok_and(|v| v == "true") {
            println!("dry run: nothing changed");
        } else {
            match org_keys::Model::rewrap(&ctx.db, &kek).await? {
                Some(done) => println!("{}", rewrapped_line(&done)),
                None => println!("SECRETS_KEK_PREVIOUS is not set; nothing to rewrap"),
            }
        }
        let mut counts = org_keys::Model::count_by_kek_version(&ctx.db).await?;
        // Name the loaded versions even with no rows, so "0 on v1" is said.
        for version in std::iter::once(kek.version).chain(kek.previous_version()) {
            if !counts.iter().any(|(v, _)| *v == version) {
                counts.push((version, 0));
            }
        }
        counts.sort_unstable();
        for (version, count) in counts {
            let role = if version == kek.version {
                "current"
            } else if kek.previous_version() == Some(version) {
                "previous"
            } else {
                "not loaded"
            };
            println!("{count} organisation keys on v{version} ({role})");
        }
        Ok(())
    }
}
