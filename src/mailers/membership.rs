//! Emails about someone's own membership of an organisation.
#![allow(non_upper_case_globals)]

use loco_rs::prelude::*;
use serde_json::json;

use super::deliver;
use crate::{
    data::settings::Settings,
    models::{auth_attempts, organisations, users},
};

static approved: Dir<'_> = include_dir!("src/mailers/membership/approved");

pub struct MembershipMailer {}

impl MembershipMailer {
    /// Tells someone an owner or admin let them in. The subject has no
    /// organisation name, as names are free text anyone can type; the body
    /// names it, escaped. It never names the approver. Until links can name
    /// an organisation it links to Home and says to choose it from the menu.
    /// Each one is recorded first and counted
    /// (`auth_attempts::Model::begin_approval_email`): over 20 for the
    /// organisation or 3 for the person in 24 hours, nothing is sent.
    ///
    /// # Errors
    /// When the count fails or the email cannot be rendered or handed off.
    pub async fn approved(
        ctx: &AppContext,
        user: &users::Model,
        org: &organisations::Model,
        username: &str,
    ) -> Result<()> {
        if !auth_attempts::Model::begin_approval_email(&ctx.db, &user.email, user.id, org.id)
            .await?
        {
            tracing::info!(org_id = org.id, "approval email limit reached");
            return Ok(());
        }
        let settings = Settings::from_context(ctx)?;
        let args = mailer::Args {
            from: Some(settings.mail_from.clone()),
            to: user.email.clone(),
            locals: json!({
                "username": username,
                "org_name": org.name,
                "dashboard_url": format!("{}/dashboard", settings.app_url),
            }),
            ..Default::default()
        };
        deliver(ctx, &settings, &approved, args).await
    }
}

#[cfg(test)]
mod tests {
    use super::{super::render, *};

    #[test]
    fn the_subject_never_names_the_org() {
        let args = mailer::Args {
            from: Some("Collaboration Tool <no-reply@rajgurung.me>".to_string()),
            to: "priya@example.com".to_string(),
            locals: json!({
                "username": "priya",
                "org_name": "Bakers <b>&</b> Co",
                "dashboard_url": "https://collab.rajgurung.me/dashboard",
            }),
            ..Default::default()
        };
        let email = render(&approved, &args).unwrap();
        assert_eq!(email.subject, "Your join request was approved");
        assert!(
            email.html.contains("Bakers &lt;b&gt;&amp;"),
            "{}",
            email.html
        );
        assert!(!email.html.contains("<b>"));
        assert!(email.text.contains("https://collab.rajgurung.me/dashboard"));
        assert!(email
            .html
            .contains("Or paste this address into your browser: https:"));
        assert!(email
            .text
            .contains("Bakers <b>&</b> Co shows in your organisations next time a page loads."));
    }
}
