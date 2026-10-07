use loco_rs::prelude::*;

pub use super::_entities::conversation_members::{ActiveModel, Column, Entity, Model};
use super::conversations;

pub type ConversationMembers = Entity;

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
    /// Adds a user to a conversation in the conversation's organisation.
    ///
    /// # Errors
    /// On database errors, including a duplicate membership.
    pub async fn add<C: ConnectionTrait>(
        db: &C,
        conversation: &conversations::Model,
        user_id: i64,
    ) -> ModelResult<Self> {
        Ok(ActiveModel {
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

    fn tenant_column() -> super::_entities::conversation_members::Column {
        super::_entities::conversation_members::Column::OrganisationId
    }
}
