//! Emails about account security, such as a changed password.
#![allow(non_upper_case_globals)]

use loco_rs::prelude::*;
use serde_json::json;

use super::deliver;
use crate::{
    data::settings::Settings,
    models::{memberships, organisations, users},
    views::time,
};

static password_changed: Dir<'_> = include_dir!("src/mailers/security/password_changed");
static login_seen: Dir<'_> = include_dir!("src/mailers/security/login_seen");
static account_locked: Dir<'_> = include_dir!("src/mailers/security/account_locked");
static your_account_locked: Dir<'_> = include_dir!("src/mailers/security/your_account_locked");
static member_password_changed: Dir<'_> =
    include_dir!("src/mailers/security/member_password_changed");

pub struct SecurityMailer {}

impl SecurityMailer {
    /// Tells someone their password was changed, so a reset they did not ask
    /// for does not go unnoticed. `claude_disconnected` adds how to connect
    /// Claude again, when the reset revoked a token.
    ///
    /// # Errors
    /// When the email cannot be rendered or handed off.
    pub async fn password_changed(
        ctx: &AppContext,
        user: &users::Model,
        claude_disconnected: bool,
    ) -> Result<()> {
        let settings = Settings::from_context(ctx)?;
        let args = mailer::Args {
            from: Some(settings.mail_from.clone()),
            to: user.email.clone(),
            locals: json!({
                "name": user.name,
                "email": user.email,
                "forgot_url": format!("{}/forgot", settings.app_url),
                "claude_disconnected": claude_disconnected,
            }),
            ..Default::default()
        };
        deliver(ctx, &settings, &password_changed, args).await
    }

    /// Tells every owner that someone saw or copied a login's password or
    /// recovery codes. Never includes the secret.
    ///
    /// # Errors
    /// When an email cannot be rendered or handed off.
    pub async fn login_seen(
        ctx: &AppContext,
        org: &organisations::Model,
        who: &str,
        title: &str,
        copied: bool,
        recovery: bool,
    ) -> Result<()> {
        let locals = json!({
            "who": who,
            "title": title,
            "verb": if copied { "copied" } else { "saw" },
            "what": if recovery { "the recovery codes" } else { "the password" },
        });
        Self::to_owners(ctx, org, None, &login_seen, locals).await
    }

    /// Tells every owner of the organisation where it happened that
    /// someone's account was locked after too many wrong app passwords. The
    /// locked person is skipped: an owner who locked themselves gets
    /// [`Self::your_account_locked`] only.
    ///
    /// # Errors
    /// When an email cannot be rendered or handed off.
    pub async fn account_locked(
        ctx: &AppContext,
        org: &organisations::Model,
        user_id: i64,
        who: &str,
    ) -> Result<()> {
        let locals = json!({ "who": who });
        Self::to_owners(ctx, org, Some(user_id), &account_locked, locals).await
    }

    /// Tells the person their account was locked, in which organisation,
    /// and how to unlock it. The link is the plain "Forgot password" page,
    /// never a reset token.
    ///
    /// # Errors
    /// When the email cannot be rendered or handed off.
    pub async fn your_account_locked(
        ctx: &AppContext,
        org: &organisations::Model,
        user: &users::Model,
    ) -> Result<()> {
        let settings = Settings::from_context(ctx)?;
        let args = mailer::Args {
            from: Some(settings.mail_from.clone()),
            to: user.email.clone(),
            locals: json!({
                "name": user.name,
                "org": org.name,
                "forgot_url": format!("{}/forgot", settings.app_url),
            }),
            ..Default::default()
        };
        deliver(ctx, &settings, &your_account_locked, args).await
    }

    /// Tells the other owners that the password of someone who can see
    /// login passwords was reset.
    ///
    /// # Errors
    /// When an email cannot be rendered or handed off.
    pub async fn member_password_changed(
        ctx: &AppContext,
        org: &organisations::Model,
        user: &users::Model,
        who: &str,
    ) -> Result<()> {
        Self::to_owners(
            ctx,
            org,
            Some(user.id),
            &member_password_changed,
            json!({ "who": who }),
        )
        .await
    }

    /// One email to each active owner except `skip`, with the organisation,
    /// the time and a link to Login activity added to `locals`.
    async fn to_owners(
        ctx: &AppContext,
        org: &organisations::Model,
        skip: Option<i64>,
        dir: &Dir<'_>,
        mut locals: serde_json::Value,
    ) -> Result<()> {
        let settings = Settings::from_context(ctx)?;
        let now = time::local(chrono::Utc::now().fixed_offset(), time::zone(&org.timezone));
        locals["org"] = json!(org.name);
        locals["at"] = json!(now.format("%H:%M on %-d %b").to_string());
        locals["activity_url"] = json!(format!("{}/documents/activity", settings.app_url));
        for (owner, user) in memberships::Model::owners(&ctx.db, org.id).await? {
            if Some(user.id) == skip {
                continue;
            }
            let mut locals = locals.clone();
            locals["name"] = json!(owner.username);
            let args = mailer::Args {
                from: Some(settings.mail_from.clone()),
                to: user.email,
                locals,
                ..Default::default()
            };
            deliver(ctx, &settings, dir, args).await?;
        }
        Ok(())
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

    #[test]
    fn owner_emails_escape_names_and_never_carry_a_secret() {
        let locals = json!({
            "name": "raj",
            "who": "<b>eve</b>",
            "title": "<script>x</script> Bank",
            "verb": "saw",
            "what": "the password",
            "org": "Acme & Co",
            "at": "14:41 on 12 Oct",
            "activity_url": "https://collab.rajgurung.me/documents/activity",
        });
        for dir in [&login_seen, &account_locked, &member_password_changed] {
            let args = mailer::Args {
                to: "raj@example.com".to_string(),
                locals: locals.clone(),
                ..Default::default()
            };
            let email = render(dir, &args).unwrap();
            assert!(!email.html.contains("<b>eve</b>"), "{}", email.html);
            assert!(!email.html.contains("<script>"), "{}", email.html);
            assert!(
                email.html.contains("&lt;b&gt;eve&lt;/b&gt;"),
                "{}",
                email.html
            );
            assert!(email.html.contains("/documents/activity"));
        }
        let args = mailer::Args {
            to: "raj@example.com".to_string(),
            locals,
            ..Default::default()
        };
        let email = render(&login_seen, &args).unwrap();
        assert_eq!(
            email.subject,
            "<b>eve</b> saw the password for <script>x</script> Bank"
        );
    }
}
