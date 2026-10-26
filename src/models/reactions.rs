//! Emoji reactions on chat messages, from a fixed set of six. One row per
//! person, message and emoji. Reactions send no notification.
use std::collections::HashMap;

use loco_rs::prelude::*;

pub use super::_entities::reactions::{ActiveModel, Column, Entity, Model};
use super::{field_error, messages};

pub type Reactions = Entity;

/// The reactions people can choose, in the order chips show. ❤️ is two code
/// points (U+2764 U+FE0F); a bare ❤ is refused, so compare exact strings.
pub const EMOJI: [&str; 6] = ["👍", "❤️", "😂", "🎉", "👀", "✅"];

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
    /// Sets whether `user_id` has reacted to `message` with `emoji`. The client
    /// sends the state it wants, so a retry or a double tap changes nothing.
    /// The caller must already know the person can see the message. A new
    /// reaction is saved under a lock on the message, so it never lands on
    /// one being deleted.
    ///
    /// # Errors
    /// `EntityNotFound` for a message deleted meanwhile; a field error for an
    /// emoji outside [`EMOJI`], or database errors.
    pub async fn set(
        db: &DatabaseConnection,
        message: &messages::Model,
        user_id: i64,
        emoji: &str,
        on: bool,
    ) -> ModelResult<()> {
        if !EMOJI.contains(&emoji) {
            return Err(field_error("emoji", "Pick one of the six reactions."));
        }
        if on {
            let row = ActiveModel {
                emoji: ActiveValue::Set(emoji.to_string()),
                message_id: ActiveValue::Set(message.id),
                user_id: ActiveValue::Set(user_id),
                ..Default::default()
            }
            .set_tenant(message.organisation_id)?;
            let txn = db.begin().await?;
            if messages::Model::lock_live(&txn, message.organisation_id, message.id)
                .await?
                .is_none()
            {
                // Dropping `txn` rolls it back.
                return Err(ModelError::EntityNotFound);
            }
            // `on_conflict_do_nothing_on` treats a conflict as success; a plain
            // `on_conflict(.. do_nothing ..)` returns `RecordNotInserted`.
            Entity::insert(row)
                .on_conflict_do_nothing_on([Column::MessageId, Column::UserId, Column::Emoji])
                .exec(&txn)
                .await?;
            txn.commit().await?;
        } else {
            Entity::delete_many()
                .filter(Column::OrganisationId.eq(message.organisation_id))
                .filter(Column::MessageId.eq(message.id))
                .filter(Column::UserId.eq(user_id))
                .filter(Column::Emoji.eq(emoji))
                .exec(db)
                .await?;
        }
        Ok(())
    }

    /// Every reaction on the given messages, grouped by message, oldest first.
    /// One query, so a feed never asks per message.
    ///
    /// # Errors
    /// On database errors.
    pub async fn for_messages<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        message_ids: &[i64],
    ) -> ModelResult<HashMap<i64, Vec<Self>>> {
        let mut grouped: HashMap<i64, Vec<Self>> = HashMap::new();
        if message_ids.is_empty() {
            return Ok(grouped);
        }
        for row in Entity::find()
            .in_tenant(org_id)
            .filter(Column::MessageId.is_in(message_ids.iter().copied()))
            .order_by_asc(Column::CreatedAt)
            .order_by_asc(Column::Id)
            .all(db)
            .await?
        {
            grouped.entry(row.message_id).or_default().push(row);
        }
        Ok(grouped)
    }
}

impl loco_rs::prelude::TenantEntity for Entity {
    type TenantId = i64;

    fn tenant_column() -> super::_entities::reactions::Column {
        super::_entities::reactions::Column::OrganisationId
    }
}
