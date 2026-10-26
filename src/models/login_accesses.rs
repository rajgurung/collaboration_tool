//! Members named on a login, who may see its password as well as owners and
//! admins.
use std::collections::HashSet;

use loco_rs::prelude::*;

pub use super::_entities::login_accesses::{ActiveModel, Column, Entity, Model};
use super::{logins, memberships};

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

    /// The organisation's logins that name `user_id`, by title: a member's
    /// "Shared with you".
    ///
    /// # Errors
    /// On database errors.
    pub async fn logins_for<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
    ) -> ModelResult<Vec<logins::Model>> {
        let named = sea_orm::sea_query::Query::select()
            .column(Column::LoginId)
            .from(Entity)
            .and_where(Column::UserId.eq(user_id))
            .to_owned();
        Ok(logins::Entity::find()
            .in_tenant(org_id)
            .filter(logins::Column::Id.in_subquery(named))
            .order_by_asc(logins::Column::Title)
            .all(db)
            .await?)
    }

    /// Names exactly `user_ids` among the organisation's active members.
    /// Callers check every id is an active member first.
    ///
    /// Rows for people who were removed stay: the "change these at the
    /// source" list uses them to show what that person could see. They grant
    /// nothing, because only active members get past `CurrentMember`. If the
    /// person asks to join again, approval clears them
    /// ([`Self::remove_for_member`]).
    ///
    /// # Errors
    /// On database errors.
    pub async fn replace<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        login_id: i64,
        user_ids: &[i64],
    ) -> ModelResult<()> {
        let active: Vec<i64> = memberships::Model::team(db, org_id)
            .await?
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        Entity::delete_many()
            .filter(Column::LoginId.eq(login_id))
            .filter(Column::UserId.is_in(active))
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

    /// Takes `user_id` off every login of this organisation that names
    /// them, and only this organisation's. Approving someone who was
    /// removed calls it, so old shares don't come back with them.
    ///
    /// # Errors
    /// On database errors.
    pub async fn remove_for_member<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
    ) -> ModelResult<()> {
        let this_org = sea_orm::sea_query::Query::select()
            .column(logins::Column::Id)
            .from(logins::Entity)
            .and_where(logins::Column::OrganisationId.eq(org_id))
            .to_owned();
        Entity::delete_many()
            .filter(Column::UserId.eq(user_id))
            .filter(Column::LoginId.in_subquery(this_org))
            .exec(db)
            .await?;
        Ok(())
    }
}
