use std::collections::{BTreeMap, HashMap};

use loco_rs::{
    prelude::*,
    validation::{ModelValidationErrors, ValidationError},
};
use sea_orm::sea_query::{ExprTrait, Func};

pub use super::_entities::memberships::{ActiveModel, Column, Entity, Model};
use super::{
    organisations,
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

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.status == status::ACTIVE
    }

    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.status == status::PENDING
    }
}

/// A validation error for one field, shaped like the ones `Validatable` produces.
fn field_error(field: &str, message: &str) -> ModelError {
    ModelError::Validation(ModelValidationErrors {
        errors: BTreeMap::from([(
            field.to_string(),
            vec![ValidationError {
                code: "taken".to_string(),
                message: Some(message.to_string()),
                params: HashMap::new(),
            }],
        )]),
    })
}

impl loco_rs::prelude::TenantEntity for Entity {
    type TenantId = i64;

    fn tenant_column() -> super::_entities::memberships::Column {
        super::_entities::memberships::Column::OrganisationId
    }
}
