use std::collections::HashMap;

use loco_rs::prelude::*;
use serde::Deserialize;

pub use super::_entities::messages::{ActiveModel, Column, Entity, Model};
use super::{conversation_members::ReadSpan, conversations};

pub type Messages = Entity;

/// How many messages a conversation shows when it is opened.
pub const HISTORY_LIMIT: u64 = 200;

#[derive(Debug, Deserialize, Validate)]
pub struct MessageParams {
    #[validate(length(min = 1, max = 1000, message = "Messages are 1 to 1,000 characters."))]
    pub body: String,
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
    /// The latest messages in a conversation, oldest first.
    ///
    /// # Errors
    /// On database errors.
    pub async fn recent<C: ConnectionTrait>(
        db: &C,
        conversation: &conversations::Model,
    ) -> ModelResult<Vec<Self>> {
        let mut latest = Entity::find()
            .in_tenant(conversation.organisation_id)
            .filter(Column::ConversationId.eq(conversation.id))
            .order_by_desc(Column::CreatedAt)
            .order_by_desc(Column::Id)
            .limit(HISTORY_LIMIT)
            .all(db)
            .await?;
        latest.reverse();
        Ok(latest)
    }

    /// Messages from other people that a read just covered, newest first and
    /// capped at what a conversation shows.
    ///
    /// # Errors
    /// On database errors.
    pub async fn read_in_span<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        conversation_id: i64,
        reader_id: i64,
        span: &ReadSpan,
    ) -> ModelResult<Vec<Self>> {
        Ok(Entity::find()
            .in_tenant(org_id)
            .filter(Column::ConversationId.eq(conversation_id))
            .filter(Column::UserId.ne(reader_id))
            .filter(Column::CreatedAt.gt(span.from))
            .filter(Column::CreatedAt.lte(span.to))
            .order_by_desc(Column::CreatedAt)
            .order_by_desc(Column::Id)
            .limit(HISTORY_LIMIT)
            .all(db)
            .await?)
    }

    /// The newest message in each of the given conversations.
    ///
    /// # Errors
    /// On database errors.
    pub async fn latest_per_conversation<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        conversation_ids: &[i64],
    ) -> ModelResult<HashMap<i64, Self>> {
        let mut latest = HashMap::new();
        for message in Entity::find()
            .in_tenant(org_id)
            .filter(Column::ConversationId.is_in(conversation_ids.iter().copied()))
            .order_by_desc(Column::CreatedAt)
            .order_by_desc(Column::Id)
            .all(db)
            .await?
        {
            latest.entry(message.conversation_id).or_insert(message);
        }
        Ok(latest)
    }

    /// Saves a message. The caller must already know the sender is a member.
    ///
    /// # Errors
    /// Validation errors, or database errors.
    pub async fn create<C: ConnectionTrait>(
        db: &C,
        conversation: &conversations::Model,
        user_id: i64,
        params: &MessageParams,
    ) -> ModelResult<Self> {
        let body = params.body.trim().to_string();
        ValidatorTrait::validate(&MessageParams { body: body.clone() })?;
        Ok(ActiveModel {
            body: ActiveValue::Set(body),
            conversation_id: ActiveValue::Set(conversation.id),
            user_id: ActiveValue::Set(user_id),
            ..Default::default()
        }
        .set_tenant(conversation.organisation_id)?
        .insert(db)
        .await?)
    }
}

impl loco_rs::prelude::TenantEntity for Entity {
    type TenantId = i64;

    fn tenant_column() -> super::_entities::messages::Column {
        super::_entities::messages::Column::OrganisationId
    }
}
