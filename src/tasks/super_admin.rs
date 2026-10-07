use loco_rs::prelude::*;

use crate::{
    data::settings::Settings,
    models::{memberships, organisations, users},
};

/// Creates or promotes the platform super admin and gives them their own
/// organisation. Safe to run more than once.
pub struct SuperAdmin;

#[async_trait]
impl Task for SuperAdmin {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "super_admin".to_string(),
            detail: "Create or promote the platform super admin from `settings.super_admin`.\nThe first run needs SUPER_ADMIN_PASSWORD set.\nUsage: cargo loco task super_admin".to_string(),
        }
    }

    async fn run(&self, ctx: &AppContext, _vars: &task::Vars) -> Result<()> {
        let config = Settings::from_context(ctx)?.super_admin;
        let txn = ctx.db.begin().await?;

        let user = match users::Model::find_by_email(&txn, &config.email).await {
            Ok(user) => user,
            Err(ModelError::EntityNotFound) => {
                if config.password.is_empty() {
                    return Err(Error::string(
                        "the super admin does not exist yet: set SUPER_ADMIN_PASSWORD and run again",
                    ));
                }
                users::Model::create_with_password(
                    &txn,
                    &users::RegisterParams {
                        email: config.email.clone(),
                        password: config.password.clone(),
                        name: config.username.clone(),
                    },
                )
                .await?
            }
            Err(err) => return Err(err.into()),
        };
        let user = if user.is_super_admin {
            user
        } else {
            user.into_active_model().make_super_admin(&txn).await?
        };

        let org_name = match memberships::Model::find_for_user(&txn, user.id).await? {
            Some(membership) => {
                organisations::Model::find_by_id(&txn, membership.organisation_id)
                    .await?
                    .name
            }
            None => {
                organisations::Model::create_with_owner(&txn, &config.organisation, &user)
                    .await?
                    .name
            }
        };
        txn.commit().await?;

        println!(
            "Super admin ready: {} ({}), organisation: {org_name}",
            user.email, user.name
        );
        Ok(())
    }
}
