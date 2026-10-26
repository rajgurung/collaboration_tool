use std::collections::HashMap;

use loco_rs::prelude::*;

pub use super::_entities::project_owners::{ActiveModel, Column, Entity, Model};

pub type ProjectOwners = Entity;

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
    /// Every project's owners in an organisation, in the order they were added.
    ///
    /// # Errors
    /// On database errors.
    pub async fn by_project<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
    ) -> ModelResult<HashMap<i64, Vec<i64>>> {
        let mut map: HashMap<i64, Vec<i64>> = HashMap::new();
        for row in Entity::find()
            .in_tenant(org_id)
            .order_by_asc(Column::Id)
            .all(db)
            .await?
        {
            map.entry(row.project_id).or_default().push(row.user_id);
        }
        Ok(map)
    }

    /// One project's owners, in the order they were added.
    ///
    /// # Errors
    /// On database errors.
    pub async fn for_project<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        project_id: i64,
    ) -> ModelResult<Vec<i64>> {
        Ok(Entity::find()
            .in_tenant(org_id)
            .filter(Column::ProjectId.eq(project_id))
            .order_by_asc(Column::Id)
            .all(db)
            .await?
            .into_iter()
            .map(|o| o.user_id)
            .collect())
    }

    /// Sets a project's owners to exactly `user_ids`, keeping existing rows
    /// (and their order) for people who stay, so the first owner stays first.
    /// Callers check the ids are on the team.
    ///
    /// # Errors
    /// On database errors.
    pub async fn replace<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        project_id: i64,
        user_ids: &[i64],
    ) -> ModelResult<()> {
        let current = Self::for_project(db, org_id, project_id).await?;
        let removed: Vec<i64> = current
            .iter()
            .filter(|id| !user_ids.contains(id))
            .copied()
            .collect();
        if !removed.is_empty() {
            Entity::delete_many()
                .filter(Column::OrganisationId.eq(org_id))
                .filter(Column::ProjectId.eq(project_id))
                .filter(Column::UserId.is_in(removed))
                .exec(db)
                .await?;
        }
        for user_id in user_ids.iter().filter(|id| !current.contains(id)) {
            ActiveModel {
                project_id: ActiveValue::Set(project_id),
                user_id: ActiveValue::Set(*user_id),
                ..Default::default()
            }
            .set_tenant(org_id)?
            .insert(db)
            .await?;
        }
        Ok(())
    }

    /// Takes someone off every project they own in this organisation, when
    /// they are removed from it.
    ///
    /// # Errors
    /// On database errors.
    pub async fn remove_person<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
    ) -> ModelResult<()> {
        Entity::delete_many()
            .filter(Column::OrganisationId.eq(org_id))
            .filter(Column::UserId.eq(user_id))
            .exec(db)
            .await?;
        Ok(())
    }
}

impl loco_rs::prelude::TenantEntity for Entity {
    type TenantId = i64;

    fn tenant_column() -> Column {
        Column::OrganisationId
    }
}
