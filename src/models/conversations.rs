use loco_rs::prelude::*;
use serde::Deserialize;

pub use super::_entities::conversations::{ActiveModel, Column, Entity, Model};
use super::{conversation_members, field_error, memberships, organisations, users};

/// The new-group form. `member_ids` comes from repeated checkbox fields.
#[derive(Debug, Deserialize, Validate)]
pub struct GroupParams {
    #[validate(length(
        min = 1,
        max = 40,
        message = "Give the group a short name (up to 40 characters)."
    ))]
    pub name: String,
    #[serde(default)]
    pub member_ids: Vec<i64>,
}

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

impl Model {
    /// Conversations `user_id` belongs to, channels and groups first, then DMs.
    ///
    /// # Errors
    /// On database errors.
    pub async fn list_for_user<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
    ) -> ModelResult<Vec<Self>> {
        let ids: Vec<i64> = conversation_members::Entity::find()
            .in_tenant(org_id)
            .filter(conversation_members::Column::UserId.eq(user_id))
            .all(db)
            .await?
            .into_iter()
            .map(|m| m.conversation_id)
            .collect();
        let mut list = Entity::find()
            .in_tenant(org_id)
            .filter(Column::Id.is_in(ids))
            .order_by_asc(Column::CreatedAt)
            .order_by_asc(Column::Id)
            .all(db)
            .await?;
        list.sort_by_key(|c| c.kind == kind::DM);
        Ok(list)
    }

    /// A conversation the user belongs to. Anything else is "not found", so
    /// private groups and DMs are not revealed.
    ///
    /// # Errors
    /// `EntityNotFound` when it does not exist in the org or the user is not a member.
    pub async fn find_for_member<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        id: i64,
        user_id: i64,
    ) -> ModelResult<Self> {
        let conversation = Entity::find_by_id(id)
            .in_tenant(org_id)
            .one(db)
            .await?
            .ok_or(ModelError::EntityNotFound)?;
        if conversation_members::Model::is_member(db, &conversation, user_id).await? {
            Ok(conversation)
        } else {
            Err(ModelError::EntityNotFound)
        }
    }

    /// User ids of everyone in the conversation.
    ///
    /// # Errors
    /// On database errors.
    pub async fn member_ids<C: ConnectionTrait>(&self, db: &C) -> ModelResult<Vec<i64>> {
        Ok(conversation_members::Entity::find()
            .in_tenant(self.organisation_id)
            .filter(conversation_members::Column::ConversationId.eq(self.id))
            .order_by_asc(conversation_members::Column::Id)
            .all(db)
            .await?
            .into_iter()
            .map(|m| m.user_id)
            .collect())
    }

    /// Creates a private group with the creator and chosen teammates.
    ///
    /// # Errors
    /// Validation errors (including members outside the team), or database errors.
    pub async fn create_group(
        db: &DatabaseConnection,
        org_id: i64,
        creator_id: i64,
        params: &GroupParams,
    ) -> ModelResult<Self> {
        let name = params
            .name
            .trim()
            .trim_start_matches('#')
            .trim()
            .to_string();
        ValidatorTrait::validate(&GroupParams {
            name: name.clone(),
            member_ids: Vec::new(),
        })?;
        let team: Vec<i64> = memberships::Model::team(db, org_id)
            .await?
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let mut member_ids = params.member_ids.clone();
        member_ids.push(creator_id);
        member_ids.sort_unstable();
        member_ids.dedup();
        if member_ids
            .iter()
            .any(|id| *id != creator_id && !team.contains(id))
        {
            return Err(field_error("member_ids", "Members must be on the team."));
        }
        let txn = db.begin().await?;
        let group = ActiveModel {
            kind: ActiveValue::Set(kind::GROUP.to_string()),
            name: ActiveValue::Set(Some(name)),
            created_by_id: ActiveValue::Set(Some(creator_id)),
            ..Default::default()
        }
        .set_tenant(org_id)?
        .insert(&txn)
        .await?;
        for user_id in member_ids {
            conversation_members::Model::add(&txn, &group, user_id).await?;
        }
        txn.commit().await?;
        Ok(group)
    }

    /// The direct conversation between two teammates, created the first time.
    ///
    /// # Errors
    /// When `other_id` is not an approved teammate, or on database errors.
    pub async fn start_dm(
        db: &DatabaseConnection,
        org_id: i64,
        me: i64,
        other_id: i64,
    ) -> ModelResult<Self> {
        if other_id == me || !memberships::Model::is_active_member(db, org_id, other_id).await? {
            return Err(ModelError::msg("Choose someone else on the team."));
        }
        let key = format!("dm:{}:{}", me.min(other_id), me.max(other_id));
        let existing = Entity::find()
            .in_tenant(org_id)
            .filter(Column::DmKey.eq(&key))
            .one(db)
            .await?;
        if let Some(dm) = existing {
            return Ok(dm);
        }
        let txn = db.begin().await?;
        let dm = ActiveModel {
            kind: ActiveValue::Set(kind::DM.to_string()),
            dm_key: ActiveValue::Set(Some(key)),
            created_by_id: ActiveValue::Set(Some(me)),
            ..Default::default()
        }
        .set_tenant(org_id)?
        .insert(&txn)
        .await?;
        conversation_members::Model::add(&txn, &dm, me).await?;
        conversation_members::Model::add(&txn, &dm, other_id).await?;
        txn.commit().await?;
        Ok(dm)
    }
}

impl loco_rs::prelude::TenantEntity for Entity {
    type TenantId = i64;

    fn tenant_column() -> super::_entities::conversations::Column {
        super::_entities::conversations::Column::OrganisationId
    }
}
