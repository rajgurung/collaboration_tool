use std::collections::HashMap;

use loco_rs::prelude::*;
use sea_orm::sea_query::{ExprTrait, Func};

pub use super::_entities::memberships::{ActiveModel, Column, Entity, Model};
use super::{
    conversation_members, conversations, field_error, organisations,
    users::{self, RegisterParams},
};

pub type Memberships = Entity;

pub mod role {
    pub const OWNER: &str = "owner";
    pub const ADMIN: &str = "admin";
    pub const MEMBER: &str = "member";
}

pub mod status {
    pub const PENDING: &str = "pending";
    pub const ACTIVE: &str = "active";
    pub const REJECTED: &str = "rejected";
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
    /// Every user's membership (if any), for the platform admin.
    ///
    /// # Errors
    /// On database errors.
    pub async fn all_by_user<C: ConnectionTrait>(db: &C) -> ModelResult<HashMap<i64, Self>> {
        Ok(Entity::find()
            .all(db)
            .await?
            .into_iter()
            .map(|m| (m.user_id, m))
            .collect())
    }

    /// A user belongs to at most one organisation.
    ///
    /// # Errors
    /// On database errors.
    pub async fn find_for_user<C: ConnectionTrait>(
        db: &C,
        user_id: i64,
    ) -> ModelResult<Option<Self>> {
        Ok(Entity::find()
            .filter(Column::UserId.eq(user_id))
            .one(db)
            .await?)
    }

    /// # Errors
    /// On database errors.
    pub async fn create_owner<C: ConnectionTrait>(
        db: &C,
        org: &organisations::Model,
        user: &users::Model,
    ) -> ModelResult<Self> {
        Ok(ActiveModel {
            user_id: ActiveValue::Set(user.id),
            username: ActiveValue::Set(user.name.clone()),
            role: ActiveValue::Set(role::OWNER.to_string()),
            status: ActiveValue::Set(status::ACTIVE.to_string()),
            approved_at: ActiveValue::Set(Some(chrono::Utc::now().into())),
            ..Default::default()
        }
        .set_tenant(org.id)?
        .insert(db)
        .await?)
    }

    /// Creates the account and a pending membership. An owner or admin approves it later.
    ///
    /// # Errors
    /// Validation errors (including a username already used in this
    /// organisation) and `EntityAlreadyExists` for a taken email.
    pub async fn join(
        db: &DatabaseConnection,
        org: &organisations::Model,
        params: &RegisterParams,
    ) -> ModelResult<(users::Model, Self)> {
        ValidatorTrait::validate(params)?;
        let txn = db.begin().await?;
        let username = params.name.trim().to_string();
        let taken = Entity::find()
            .in_tenant(org.id)
            .filter(
                Expr::expr(Func::lower(Expr::col(Column::Username))).eq(username.to_lowercase()),
            )
            .one(&txn)
            .await?
            .is_some();
        if taken {
            return Err(field_error(
                "name",
                "That username is already used in this organisation.",
            ));
        }
        let user = users::Model::create_with_password(&txn, params).await?;
        let membership = ActiveModel {
            user_id: ActiveValue::Set(user.id),
            username: ActiveValue::Set(username),
            role: ActiveValue::Set(role::MEMBER.to_string()),
            status: ActiveValue::Set(status::PENDING.to_string()),
            ..Default::default()
        }
        .set_tenant(org.id)?
        .insert(&txn)
        .await?;
        txn.commit().await?;
        Ok((user, membership))
    }

    /// Every membership in the organisation with its user, oldest first.
    ///
    /// # Errors
    /// On database errors.
    pub async fn list_for_org<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
    ) -> ModelResult<Vec<(Self, users::Model)>> {
        let memberships = Entity::find()
            .in_tenant(org_id)
            .order_by_asc(Column::CreatedAt)
            .all(db)
            .await?;
        let mut users: HashMap<i64, users::Model> = users::Entity::find()
            .filter(users::users::Column::Id.is_in(memberships.iter().map(|m| m.user_id)))
            .all(db)
            .await?
            .into_iter()
            .map(|u| (u.id, u))
            .collect();
        Ok(memberships
            .into_iter()
            .filter_map(|m| users.remove(&m.user_id).map(|u| (m, u)))
            .collect())
    }

    /// `(user_id, username)` for every approved member, oldest first.
    ///
    /// # Errors
    /// On database errors.
    pub async fn team<C: ConnectionTrait>(db: &C, org_id: i64) -> ModelResult<Vec<(i64, String)>> {
        Ok(Entity::find()
            .in_tenant(org_id)
            .filter(Column::Status.eq(status::ACTIVE))
            .order_by_asc(Column::CreatedAt)
            .all(db)
            .await?
            .into_iter()
            .map(|m| (m.user_id, m.username))
            .collect())
    }

    /// # Errors
    /// `EntityNotFound` when the membership is not in this organisation.
    pub async fn find_in_org<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        id: i64,
    ) -> ModelResult<Self> {
        Entity::find_by_id(id)
            .in_tenant(org_id)
            .one(db)
            .await?
            .ok_or(ModelError::EntityNotFound)
    }

    /// Approves a pending request and adds the member to `#general`.
    ///
    /// # Errors
    /// When the membership is not pending, or on database errors.
    pub async fn approve(self, db: &DatabaseConnection, approver_id: i64) -> ModelResult<Self> {
        if !self.is_pending() {
            return Err(ModelError::msg("Only pending requests can be approved."));
        }
        let txn = db.begin().await?;
        let general = conversations::Model::find_general(&txn, self.organisation_id).await?;
        let mut membership = self.into_active_model();
        membership.status = ActiveValue::Set(status::ACTIVE.to_string());
        membership.approved_by_id = ActiveValue::Set(Some(approver_id));
        membership.approved_at = ActiveValue::Set(Some(chrono::Utc::now().into()));
        let membership = membership.update(&txn).await?;
        conversation_members::Model::add(&txn, &general, membership.user_id).await?;
        txn.commit().await?;
        Ok(membership)
    }

    /// # Errors
    /// When the membership is not pending, or on database errors.
    pub async fn reject<C: ConnectionTrait>(self, db: &C, approver_id: i64) -> ModelResult<Self> {
        if !self.is_pending() {
            return Err(ModelError::msg("Only pending requests can be declined."));
        }
        let mut membership = self.into_active_model();
        membership.status = ActiveValue::Set(status::REJECTED.to_string());
        membership.approved_by_id = ActiveValue::Set(Some(approver_id));
        Ok(membership.update(db).await?)
    }

    /// Makes an active member an admin or turns an admin back into a member.
    /// Owners keep their role.
    ///
    /// # Errors
    /// For an unknown role, an owner, an inactive member, or database errors.
    pub async fn change_role<C: ConnectionTrait>(
        self,
        db: &C,
        new_role: &str,
    ) -> ModelResult<Self> {
        if new_role != role::ADMIN && new_role != role::MEMBER {
            return Err(ModelError::msg("Choose admin or member."));
        }
        if self.role == role::OWNER {
            return Err(ModelError::msg("The owner's role cannot be changed."));
        }
        if !self.is_active() {
            return Err(ModelError::msg(
                "Approve the request before changing the role.",
            ));
        }
        let mut membership = self.into_active_model();
        membership.role = ActiveValue::Set(new_role.to_string());
        Ok(membership.update(db).await?)
    }

    /// Whether `user_id` is an approved member of the organisation.
    ///
    /// # Errors
    /// On database errors.
    pub async fn is_active_member<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
    ) -> ModelResult<bool> {
        Ok(Entity::find()
            .in_tenant(org_id)
            .filter(Column::UserId.eq(user_id))
            .filter(Column::Status.eq(status::ACTIVE))
            .one(db)
            .await?
            .is_some())
    }

    /// The viewer's own approved membership in this organisation, if any.
    /// A platform admin visiting another organisation has none.
    ///
    /// # Errors
    /// On database errors.
    pub async fn active_in<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
    ) -> ModelResult<Option<Self>> {
        Ok(Entity::find()
            .in_tenant(org_id)
            .filter(Column::UserId.eq(user_id))
            .filter(Column::Status.eq(status::ACTIVE))
            .one(db)
            .await?)
    }

    /// Closes the "Getting started" guide (`hidden`) or brings it back.
    ///
    /// # Errors
    /// On database errors.
    pub async fn set_guide_hidden<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
        hidden: bool,
    ) -> ModelResult<()> {
        let at = hidden.then(|| chrono::Utc::now().fixed_offset());
        Self::set_for(db, org_id, user_id, Column::GuideDismissedAt, at).await
    }

    /// Records that the welcome pop-up was seen, so it shows only once.
    ///
    /// # Errors
    /// On database errors.
    pub async fn mark_welcomed<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
    ) -> ModelResult<()> {
        let at = Some(chrono::Utc::now().fixed_offset());
        Self::set_for(db, org_id, user_id, Column::WelcomedAt, at).await
    }

    async fn set_for<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
        column: Column,
        at: Option<chrono::DateTime<chrono::FixedOffset>>,
    ) -> ModelResult<()> {
        Entity::update_many()
            .col_expr(column, sea_orm::sea_query::Expr::value(at))
            .col_expr(
                Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(chrono::Utc::now().fixed_offset()),
            )
            .filter(Column::OrganisationId.eq(org_id))
            .filter(Column::UserId.eq(user_id))
            .exec(db)
            .await?;
        Ok(())
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.status == status::ACTIVE
    }

    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.status == status::PENDING
    }
}

impl loco_rs::prelude::TenantEntity for Entity {
    type TenantId = i64;

    fn tenant_column() -> super::_entities::memberships::Column {
        super::_entities::memberships::Column::OrganisationId
    }
}
