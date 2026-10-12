//! Emails about account security, such as a changed password.
#![allow(non_upper_case_globals)]

use loco_rs::prelude::*;
use serde_json::json;

use super::deliver;
use crate::{data::settings::Settings, models::users};

static password_changed: Dir<'_> = include_dir!("src/mailers/security/password_changed");

pub struct SecurityMailer {}

impl SecurityMailer {
    /// Tells someone their password was changed, so a reset they did not ask
    /// for does not go unnoticed.
    ///
    /// # Errors
    /// When the email cannot be rendered or handed off.
    pub async fn password_changed(ctx: &AppContext, user: &users::Model) -> Result<()> {
        let settings = Settings::from_context(ctx)?;
        let args = mailer::Args {
            from: Some(settings.mail_from.clone()),
            to: user.email.clone(),
            locals: json!({
                "name": user.name,
                "email": user.email,
                "forgot_url": format!("{}/forgot", settings.app_url),
            }),
            ..Default::default()
        };
        deliver(ctx, &settings, &password_changed, args).await
    }
}

#[cfg(test)]
mod tests {
    use super::{super::render, *};

    #[test]
    fn renders_the_password_changed_email() {
        let args = mailer::Args {
            to: "raj@example.com".to_string(),
            locals: json!({
                "name": "raj",
                "email": "raj@example.com",
                "forgot_url": "https://collab.rajgurung.me/forgot",
            }),
            ..Default::default()
        };
        let email = render(&password_changed, &args).unwrap();
        assert_eq!(
            email.subject,
            "Your Collaboration Tool password was changed"
        );
        assert!(email.text.contains("https://collab.rajgurung.me/forgot"));
        assert!(email
            .html
            .contains(r#"href="https://collab.rajgurung.me/forgot""#));
    }
}
