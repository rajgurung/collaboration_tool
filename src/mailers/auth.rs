// auth mailer
#![allow(non_upper_case_globals)]

use loco_rs::prelude::*;
use serde_json::json;

use crate::{
    data::settings::Settings,
    models::users,
    workers::resend_email::{Worker as ResendEmail, WorkerArgs as ResendEmailArgs},
};

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

/// Sends through Resend's HTTPS API when a key is configured, otherwise
/// through Loco's SMTP mailer (development uses Mailpit, tests use the stub).
async fn deliver(
    ctx: &AppContext,
    settings: &Settings,
    dir: &Dir<'_>,
    args: mailer::Args,
) -> Result<()> {
    if settings.resend_api_key.is_empty() {
        return AuthMailer::mail_template(ctx, dir, args).await;
    }
    let email = render(dir, &args)?;
    ResendEmail::perform_later(ctx, email).await?;
    Ok(())
}

/// Renders `subject.t`, `text.t` and `html.t` from a mailer template folder.
fn render(dir: &Dir<'_>, args: &mailer::Args) -> Result<ResendEmailArgs> {
    let context =
        tera::Context::from_serialize(&args.locals).map_err(|e| Error::string(&e.to_string()))?;
    let part = |name: &str, autoescape: bool| -> Result<String> {
        let source = dir
            .get_file(name)
            .and_then(|f| f.contents_utf8())
            .ok_or_else(|| Error::string(&format!("missing mailer template {name}")))?;
        tera::Tera::one_off(source, &context, autoescape).map_err(|e| Error::string(&e.to_string()))
    };
    Ok(ResendEmailArgs {
        from: args.from.clone().unwrap_or_default(),
        to: args.to.clone(),
        subject: part("subject.t", false)?.trim().to_string(),
        text: part("text.t", false)?,
        html: part("html.t", true)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
