// auth mailer
#![allow(non_upper_case_globals)]

use loco_rs::prelude::*;
use serde_json::json;

use super::deliver;
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
        let args = mailer::Args {
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
        };
        deliver(ctx, &settings, &forgot, args).await
    }
}

#[cfg(test)]
mod tests {
    use super::{super::render, *};

    #[test]
    fn renders_the_forgot_email_for_resend() {
        let args = mailer::Args {
            from: Some("Collaboration Tool <no-reply@rajgurung.me>".to_string()),
            to: "raj@example.com".to_string(),
            locals: json!({ "name": "raj", "reset_url": "https://collab.rajgurung.me/reset/abc" }),
            ..Default::default()
        };
        let email = render(&forgot, &args).unwrap();
        assert_eq!(email.subject, "Reset your Collaboration Tool password");
        assert!(email.text.contains("https://collab.rajgurung.me/reset/abc"));
        assert!(email
            .html
            .contains(r#"href="https://collab.rajgurung.me/reset/abc""#));
        assert!(email.html.contains("Hi raj"));
        assert_eq!(email.from, "Collaboration Tool <no-reply@rajgurung.me>");
    }
}
