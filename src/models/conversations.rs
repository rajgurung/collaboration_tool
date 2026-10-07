use loco_rs::prelude::*;

pub use super::_entities::conversations::{ActiveModel, Column, Entity, Model};
use super::{conversation_members, organisations, users};

pub type Conversations = Entity;

pub mod kind {
    pub const CHANNEL: &str = "channel";
    pub const GROUP: &str = "group";
    pub const DM: &str = "dm";
}

pub const GENERAL: &str = "general";

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
    /// The `general` channel every organisation starts with, with its creator as a member.
    ///
    /// # Errors
    /// On database errors.
    pub async fn create_general<C: ConnectionTrait>(
        db: &C,
        org: &organisations::Model,
        creator: &users::Model,
    ) -> ModelResult<Self> {
        let channel = ActiveModel {
            kind: ActiveValue::Set(kind::CHANNEL.to_string()),
            name: ActiveValue::Set(Some(GENERAL.to_string())),
            created_by_id: ActiveValue::Set(Some(creator.id)),
            ..Default::default()
        }
        .set_tenant(org.id)?
        .insert(db)
        .await?;
        conversation_members::Model::add(db, &channel, creator.id).await?;
        Ok(channel)
    }

    /// # Errors
    /// `EntityNotFound` when the organisation has no `general` channel.
    pub async fn find_general<C: ConnectionTrait>(db: &C, org_id: i64) -> ModelResult<Self> {
        Entity::find()
            .in_tenant(org_id)
            .filter(Column::Kind.eq(kind::CHANNEL))
            .filter(Column::Name.eq(GENERAL))
            .one(db)
            .await?
            .ok_or(ModelError::EntityNotFound)
    }
}

impl loco_rs::prelude::TenantEntity for Entity {
    type TenantId = i64;

    fn tenant_column() -> super::_entities::conversations::Column {
        super::_entities::conversations::Column::OrganisationId
    }
}
