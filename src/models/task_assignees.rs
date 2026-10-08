use std::collections::HashMap;

use loco_rs::prelude::*;

pub use super::_entities::task_assignees::{ActiveModel, Column, Entity, Model};

pub type TaskAssignees = Entity;

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
    /// Every task's assignees in an organisation, in the order they were added.
    ///
    /// # Errors
    /// On database errors.
    pub async fn by_task<C: ConnectionTrait>(
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
            map.entry(row.task_id).or_default().push(row.user_id);
        }
        Ok(map)
    }

    /// # Errors
    /// On database errors.
    pub async fn for_task<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        task_id: i64,
    ) -> ModelResult<Vec<i64>> {
        Ok(Entity::find()
            .in_tenant(org_id)
            .filter(Column::TaskId.eq(task_id))
            .order_by_asc(Column::Id)
            .all(db)
            .await?
            .into_iter()
            .map(|a| a.user_id)
            .collect())
    }

    /// Sets a task's assignees to exactly `user_ids`, keeping existing rows (and
    /// their order) for people who stay. Callers check the ids are on the team.
    ///
    /// # Errors
    /// On database errors.
    pub async fn replace<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        task_id: i64,
        user_ids: &[i64],
    ) -> ModelResult<()> {
        let current = Self::for_task(db, org_id, task_id).await?;
        let removed: Vec<i64> = current
            .iter()
            .filter(|id| !user_ids.contains(id))
            .copied()
            .collect();
        if !removed.is_empty() {
            Entity::delete_many()
                .filter(Column::OrganisationId.eq(org_id))
                .filter(Column::TaskId.eq(task_id))
                .filter(Column::UserId.is_in(removed))
                .exec(db)
                .await?;
        }
        for user_id in user_ids.iter().filter(|id| !current.contains(id)) {
            ActiveModel {
                task_id: ActiveValue::Set(task_id),
                user_id: ActiveValue::Set(*user_id),
                ..Default::default()
            }
            .set_tenant(org_id)?
            .insert(db)
            .await?;
        }
        Ok(())
    }
}

impl loco_rs::prelude::TenantEntity for Entity {
    type TenantId = i64;

    fn tenant_column() -> Column {
        Column::OrganisationId
    }
}
