use std::sync::LazyLock;

use loco_rs::prelude::*;
use serde::Serialize;

pub use super::_entities::notifications::{ActiveModel, Column, Entity, Model};

pub type Notifications = Entity;

/// What a notification is about. Stored as text in `notifications.kind`.
pub mod kind {
    pub const MENTION: &str = "mention";
    pub const ASSIGNED: &str = "assigned";
    pub const NOTE: &str = "note";
    pub const OWNER: &str = "owner";
    pub const PROJECT: &str = "project";
}

/// One notification to send: the same text and link for every recipient.
pub struct Notice {
    pub kind: &'static str,
    pub body: String,
    pub link: String,
}

/// An `@name` that is not part of an email address or a longer word.
static MENTION: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(^|[^A-Za-z0-9_.@])@([A-Za-z][A-Za-z0-9]{2,29})\b")
        .expect("mention regex is valid")
});

/// Team members mentioned in `body`, in the order they first appear. Names
/// match without regard to case; two teammates with the same name are both
/// mentioned.
#[must_use]
pub fn mentioned_ids(body: &str, team: &[(i64, String)]) -> Vec<i64> {
    let mut ids = Vec::new();
    for caps in MENTION.captures_iter(body) {
        for (id, name) in team {
            if name.eq_ignore_ascii_case(&caps[2]) && !ids.contains(id) {
                ids.push(*id);
            }
        }
    }
    ids
}

/// A piece of message text; `mention` pieces are an `@name` of a teammate.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Part {
    pub text: String,
    pub mention: bool,
}

/// Splits `body` so templates can highlight mentions while still escaping
/// every piece. `@names` that are not on the team stay plain text.
#[must_use]
pub fn mention_parts(body: &str, team: &[(i64, String)]) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut last = 0;
    for caps in MENTION.captures_iter(body) {
        let at = caps.get(2).map_or(0, |m| m.start() - 1);
        let end = caps.get(0).map_or(0, |m| m.end());
        if !team.iter().any(|(_, n)| n.eq_ignore_ascii_case(&caps[2])) {
            continue;
        }
        if at > last {
            parts.push(Part {
                text: body[last..at].to_string(),
                mention: false,
            });
        }
        parts.push(Part {
            text: body[at..end].to_string(),
            mention: true,
        });
        last = end;
    }
    if last < body.len() || parts.is_empty() {
        parts.push(Part {
            text: body[last..].to_string(),
            mention: false,
        });
    }
    parts
}

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        if !insert && self.updated_at.is_unchanged() {
            let mut this = self;
            this.updated_at = sea_orm::ActiveValue::Set(chrono::Utc::now().into());
            Ok(this)
        } else {
            Ok(self)
        }
    }
}

impl Model {
    /// Sends `notice` to each recipient once. The person who caused it is
    /// skipped, except for mentions: tagging yourself is a reminder you asked for.
    /// Returns who was notified, so callers can skip them for a second notice
    /// about the same action. Callers pass ids from the organisation's team.
    ///
    /// # Errors
    /// On database errors.
    pub async fn notify<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        actor_id: i64,
        recipients: &[i64],
        notice: &Notice,
    ) -> ModelResult<Vec<i64>> {
        let mut sent = Vec::new();
        for user_id in recipients {
            let skip_actor = notice.kind != kind::MENTION && *user_id == actor_id;
            if skip_actor || sent.contains(user_id) {
                continue;
            }
            ActiveModel {
                user_id: ActiveValue::Set(*user_id),
                actor_id: ActiveValue::Set(Some(actor_id)),
                kind: ActiveValue::Set(notice.kind.to_string()),
                body: ActiveValue::Set(notice.body.clone()),
                link: ActiveValue::Set(notice.link.clone()),
                ..Default::default()
            }
            .set_tenant(org_id)?
            .insert(db)
            .await?;
            sent.push(*user_id);
        }
        Ok(sent)
    }

    /// Someone's newest notifications, newest first.
    ///
    /// # Errors
    /// On database errors.
    pub async fn recent<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
        limit: u64,
    ) -> ModelResult<Vec<Self>> {
        Ok(Entity::find()
            .in_tenant(org_id)
            .filter(Column::UserId.eq(user_id))
            .order_by_desc(Column::CreatedAt)
            .order_by_desc(Column::Id)
            .limit(limit)
            .all(db)
            .await?)
    }

    /// # Errors
    /// On database errors.
    pub async fn unread_count<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
    ) -> ModelResult<u64> {
        Ok(Entity::find()
            .in_tenant(org_id)
            .filter(Column::UserId.eq(user_id))
            .filter(Column::ReadAt.is_null())
            .count(db)
            .await?)
    }

    /// Marks one of the viewer's own notifications read and returns it.
    ///
    /// # Errors
    /// `EntityNotFound` when it belongs to someone else or another organisation.
    pub async fn mark_read<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
        id: i64,
    ) -> ModelResult<Self> {
        let found = Entity::find_by_id(id)
            .in_tenant(org_id)
            .filter(Column::UserId.eq(user_id))
            .one(db)
            .await?
            .ok_or_else(|| ModelError::EntityNotFound)?;
        if found.read_at.is_some() {
            return Ok(found);
        }
        let mut active = found.into_active_model();
        active.read_at = ActiveValue::Set(Some(chrono::Utc::now().into()));
        Ok(active.update(db).await?)
    }

    /// Mentions still unread `wait` after they were made and not yet emailed,
    /// in every organisation, oldest first. The email sender uses these.
    ///
    /// # Errors
    /// On database errors.
    pub async fn due_mention_emails<C: ConnectionTrait>(
        db: &C,
        wait: chrono::Duration,
    ) -> ModelResult<Vec<Self>> {
        Ok(Entity::find()
            .filter(Column::Kind.eq(kind::MENTION))
            .filter(Column::ReadAt.is_null())
            .filter(Column::EmailedAt.is_null())
            .filter(Column::CreatedAt.lte(chrono::Utc::now() - wait))
            .order_by_asc(Column::Id)
            .all(db)
            .await?)
    }

    /// # Errors
    /// On database errors.
    pub async fn mark_emailed<C: ConnectionTrait>(db: &C, ids: &[i64]) -> ModelResult<()> {
        Entity::update_many()
            .col_expr(
                Column::EmailedAt,
                sea_orm::sea_query::Expr::value(chrono::Utc::now()),
            )
            .filter(Column::Id.is_in(ids.to_vec()))
            .exec(db)
            .await?;
        Ok(())
    }

    /// # Errors
    /// On database errors.
    pub async fn mark_all_read<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
    ) -> ModelResult<()> {
        Entity::update_many()
            .col_expr(
                Column::ReadAt,
                sea_orm::sea_query::Expr::value(chrono::Utc::now()),
            )
            .filter(Column::OrganisationId.eq(org_id))
            .filter(Column::UserId.eq(user_id))
            .filter(Column::ReadAt.is_null())
            .exec(db)
            .await?;
        Ok(())
    }
}

impl loco_rs::prelude::TenantEntity for Entity {
    type TenantId = i64;

    fn tenant_column() -> Column {
        Column::OrganisationId
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn team() -> Vec<(i64, String)> {
        vec![(1, "raj".into()), (2, "Maya".into()), (3, "maya".into())]
    }

    #[test]
    fn finds_mentions_of_teammates_only() {
        assert_eq!(
            mentioned_ids("hey @raj and @MAYA, also @nobody", &team()),
            vec![1, 2, 3]
        );
        assert_eq!(mentioned_ids("@raj @raj", &team()), vec![1]);
        assert!(mentioned_ids("mail raj@raj.com or x@raj", &team()).is_empty());
        assert!(mentioned_ids("@rajesh", &team()).is_empty());
    }

    #[test]
    fn splits_text_around_mentions() {
        let parts = mention_parts("ping @raj, not @bob.", &team());
        let texts: Vec<(&str, bool)> = parts.iter().map(|p| (p.text.as_str(), p.mention)).collect();
        assert_eq!(
            texts,
            vec![("ping ", false), ("@raj", true), (", not @bob.", false)]
        );
        assert_eq!(
            mention_parts("", &team()),
            vec![Part {
                text: String::new(),
                mention: false
            }]
        );
        assert_eq!(mention_parts("@raj", &team()).len(), 1);
    }
}
