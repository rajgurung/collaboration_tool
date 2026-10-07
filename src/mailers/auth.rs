// auth mailer
#![allow(non_upper_case_globals)]

use loco_rs::prelude::*;
use serde_json::json;

use crate::{data::settings::Settings, models::users};

static forgot: Dir<'_> = include_dir!("src/mailers/auth/forgot");

#[allow(clippy::module_name_repetitions)]
pub struct AuthMailer {}
impl Mailer for AuthMailer {}
impl AuthMailer {
    /// Sending forgot password email
    ///
    /// # Errors
    ///
    /// When email sending is failed
    pub async fn forgot_password(ctx: &AppContext, user: &users::Model) -> Result<()> {
        let settings = Settings::from_context(ctx)?;
        Self::mail_template(
            ctx,
            &forgot,
            mailer::Args {
                from: Some(settings.mail_from.clone()),
                to: user.email.clone(),
                locals: json!({
                  "name": user.name,
                  "reset_url": format!(
                      "{}/reset/{}",
                      settings.app_url,
                      user.reset_token.as_deref().unwrap_or_default()
                  ),
                }),
                ..Default::default()
            },
        )
        .await?;

        Ok(())
    }
}
