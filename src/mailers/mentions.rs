//! Emails people about @mentions they haven't seen, so a tag still reaches
//! them when the app isn't open.
#![allow(non_upper_case_globals)]

use std::collections::{BTreeMap, HashMap};

use loco_rs::prelude::*;
use serde::Serialize;
use serde_json::json;

use super::deliver;
use crate::{
    data::settings::Settings,
    models::{memberships, notifications, organisations, users},
};

static unread: Dir<'_> = include_dir!("src/mailers/mentions/unread");

/// One mention in the email.
#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub who: String,
    pub headline: String,
    pub excerpt: String,
    /// A full URL into the app.
    pub url: String,
}

pub struct MentionMailer {}

impl MentionMailer {
    /// One email listing every mention the person hasn't seen.
    ///
    /// # Errors
    /// When the email cannot be rendered or handed off.
    pub async fn unread(
        ctx: &AppContext,
        user: &users::Model,
        org: &str,
        items: &[Item],
    ) -> Result<()> {
        let settings = Settings::from_context(ctx)?;
        let subject = match items {
            [one] => format!("{} {}", one.who, one.headline),
            many => format!("{} mentions waiting for you in {org}", many.len()),
        };
        let args = mailer::Args {
            from: Some(settings.mail_from.clone()),
            to: user.email.clone(),
            locals: json!({
                "subject": subject,
                "name": user.name,
                "org": org,
                "count": items.len(),
                "items": items,
                "notifications_url": format!("{}/notifications", settings.app_url),
            }),
            ..Default::default()
        };
        deliver(ctx, &settings, &unread, args).await
    }
}

/// How long a mention waits unread before it's emailed.
pub const WAIT_MINUTES: i64 = 15;

/// Emails everyone who has mentions unread for [`WAIT_MINUTES`]: one email per
/// person. Mentions are marked emailed before sending, so a failure or a
/// restart never sends the same one twice. Returns how many emails went out.
///
/// # Errors
/// On database errors. A failed email is logged and skipped.
pub async fn send_due(ctx: &AppContext) -> Result<usize> {
    let due =
        notifications::Model::due_mention_emails(&ctx.db, chrono::Duration::minutes(WAIT_MINUTES))
            .await?;
    if due.is_empty() {
        return Ok(0);
    }
    let ids: Vec<i64> = due.iter().map(|n| n.id).collect();
    notifications::Model::mark_emailed(&ctx.db, &ids).await?;

    let mut by_person: BTreeMap<(i64, i64), Vec<notifications::Model>> = BTreeMap::new();
    for n in due {
        by_person
            .entry((n.organisation_id, n.user_id))
            .or_default()
            .push(n);
    }
    let app_url = Settings::from_context(ctx)?.app_url;
    let mut sent = 0;
    for ((org_id, user_id), list) in by_person {
        let (Some(user), Some(org)) = (
            users::Entity::find_by_id(user_id).one(&ctx.db).await?,
            organisations::Entity::find_by_id(org_id)
                .one(&ctx.db)
                .await?,
        ) else {
            continue;
        };
        let names: HashMap<i64, String> = memberships::Model::team(&ctx.db, org_id)
            .await?
            .into_iter()
            .collect();
        let items: Vec<Item> = list
            .iter()
            .map(|n| {
                let (headline, excerpt) = n.body.split_once('\n').unwrap_or((&n.body, ""));
                Item {
                    who: n
                        .actor_id
                        .and_then(|id| names.get(&id).cloned())
                        .unwrap_or_else(|| "Someone".to_string()),
                    headline: headline.to_string(),
                    excerpt: excerpt.to_string(),
                    url: format!("{app_url}{}", n.link),
                }
            })
            .collect();
        match MentionMailer::unread(ctx, &user, &org.name, &items).await {
            Ok(()) => sent += 1,
            Err(err) => tracing::error!(error = %err, user_id, "could not email mentions"),
        }
    }
    Ok(sent)
}
