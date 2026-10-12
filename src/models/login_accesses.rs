//! Members named on a login, who may see its password as well as owners and
//! admins.
use std::collections::HashSet;

use loco_rs::prelude::*;

pub use super::_entities::login_accesses::{ActiveModel, Column, Entity, Model};

pub type LoginAccesses = Entity;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {}

impl Model {
    /// The user ids named on a login.
    ///
    /// # Errors
    /// On database errors.
    pub async fn user_ids<C: ConnectionTrait>(db: &C, login_id: i64) -> ModelResult<Vec<i64>> {
        Ok(Entity::find()
            .filter(Column::LoginId.eq(login_id))
            .all(db)
            .await?
            .into_iter()
            .map(|a| a.user_id)
            .collect())
    }

    /// Whether `user_id` is named on the login.
    ///
    /// # Errors
    /// On database errors.
    pub async fn is_named<C: ConnectionTrait>(
        db: &C,
        login_id: i64,
        user_id: i64,
    ) -> ModelResult<bool> {
        Ok(Entity::find_by_id((login_id, user_id))
            .one(db)
            .await?
            .is_some())
    }

    /// The logins, among `login_ids`, that name `user_id`.
    ///
    /// # Errors
    /// On database errors.
    pub async fn named_among<C: ConnectionTrait>(
        db: &C,
        user_id: i64,
        login_ids: impl IntoIterator<Item = i64>,
    ) -> ModelResult<HashSet<i64>> {
        Ok(Entity::find()
            .filter(Column::UserId.eq(user_id))
            .filter(Column::LoginId.is_in(login_ids))
            .all(db)
            .await?
            .into_iter()
            .map(|a| a.login_id)
            .collect())
    }

    /// Names exactly `user_ids` on the login. Callers check they are active
    /// members of the organisation first.
    ///
    /// # Errors
    /// On database errors.
    pub async fn replace<C: ConnectionTrait>(
        db: &C,
        login_id: i64,
        user_ids: &[i64],
    ) -> ModelResult<()> {
        Entity::delete_many()
            .filter(Column::LoginId.eq(login_id))
            .exec(db)
            .await?;
        if user_ids.is_empty() {
            return Ok(());
        }
        Entity::insert_many(user_ids.iter().map(|user_id| ActiveModel {
            login_id: ActiveValue::Set(login_id),
            user_id: ActiveValue::Set(*user_id),
            ..Default::default()
        }))
        .exec(db)
        .await?;
        Ok(())
    }
}
