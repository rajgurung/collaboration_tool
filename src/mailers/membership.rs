//! Emails about someone's own membership of an organisation.
#![allow(non_upper_case_globals)]

use loco_rs::prelude::*;
use serde_json::json;

use super::deliver;
use crate::{
    data::settings::Settings,
    models::{memberships, organisations, users},
};

static approved: Dir<'_> = include_dir!("src/mailers/membership/approved");

/// At most this many approval emails per organisation in any 24 hours, so a
/// busy or misused organisation can't send a stream of mail through the app.
pub const APPROVAL_EMAILS_PER_DAY: u64 = 20;

pub struct MembershipMailer {}

impl MembershipMailer {
    /// Tells someone an owner or admin let them in. The subject has no
    /// organisation name, as names are free text anyone can type; the body
    /// names it, escaped. It never names the approver. Until links can name
    /// an organisation it links to Home and says to choose it from the menu.
    /// Over [`APPROVAL_EMAILS_PER_DAY`] nothing is sent.
    ///
    /// # Errors
    /// When the count fails or the email cannot be rendered or handed off.
    pub async fn approved(
        ctx: &AppContext,
        user: &users::Model,
        org: &organisations::Model,
        username: &str,
    ) -> Result<()> {
        let since = chrono::Utc::now() - chrono::Duration::hours(24);
        if memberships::Model::approved_since(&ctx.db, org.id, since).await?
            > APPROVAL_EMAILS_PER_DAY
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
        assert_eq!(email.subject, "Your request to join was approved");
        assert!(
            email.html.contains("Bakers &lt;b&gt;&amp;"),
            "{}",
            email.html
        );
        assert!(!email.html.contains("<b>"));
        assert!(email.text.contains("https://collab.rajgurung.me/dashboard"));
    }
}
