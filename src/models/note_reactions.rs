//! Emoji reactions on task notes, the same six as chat. One row per person,
//! note and emoji. They send no notification and have no live update.
use std::collections::HashMap;

use loco_rs::prelude::*;

pub use super::_entities::note_reactions::{ActiveModel, Column, Entity, Model};
use super::{field_error, reactions::EMOJI, task_notes};

pub type NoteReactions = Entity;

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
    /// Sets whether `user_id` has reacted to `note` with `emoji`. The client
    /// sends the state it wants, so a retry or a double tap changes nothing.
    /// The organisation comes from the note, never the request. The caller
    /// must already know the person can see the note.
    ///
    /// # Errors
    /// A field error for an emoji outside [`EMOJI`], or database errors.
    pub async fn set<C: ConnectionTrait>(
        db: &C,
        note: &task_notes::Model,
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
                task_note_id: ActiveValue::Set(note.id),
                user_id: ActiveValue::Set(user_id),
                ..Default::default()
            }
            .set_tenant(note.organisation_id)?;
            Entity::insert(row)
                .on_conflict_do_nothing_on([Column::TaskNoteId, Column::UserId, Column::Emoji])
                .exec(db)
                .await?;
        } else {
            Entity::delete_many()
                .filter(Column::OrganisationId.eq(note.organisation_id))
                .filter(Column::TaskNoteId.eq(note.id))
                .filter(Column::UserId.eq(user_id))
                .filter(Column::Emoji.eq(emoji))
                .exec(db)
                .await?;
        }
        Ok(())
    }

    /// Every reaction on the given notes, grouped by note, oldest first. One
    /// query, whatever the number of notes.
    ///
    /// # Errors
    /// On database errors.
    pub async fn for_notes<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        note_ids: &[i64],
    ) -> ModelResult<HashMap<i64, Vec<Self>>> {
        let mut grouped: HashMap<i64, Vec<Self>> = HashMap::new();
        if note_ids.is_empty() {
            return Ok(grouped);
        }
        for row in Entity::find()
            .in_tenant(org_id)
            .filter(Column::TaskNoteId.is_in(note_ids.iter().copied()))
            .order_by_asc(Column::CreatedAt)
            .order_by_asc(Column::Id)
            .all(db)
            .await?
        {
            grouped.entry(row.task_note_id).or_default().push(row);
        }
        Ok(grouped)
    }
}

impl loco_rs::prelude::TenantEntity for Entity {
    type TenantId = i64;

    fn tenant_column() -> super::_entities::note_reactions::Column {
        super::_entities::note_reactions::Column::OrganisationId
    }
}
