use std::collections::HashMap;

use loco_rs::prelude::*;
use serde::Deserialize;

pub use super::_entities::task_notes::{ActiveModel, Column, Entity, Model};
use super::tasks;

pub type TaskNotes = Entity;

#[derive(Debug, Deserialize, Validate)]
pub struct NoteParams {
    #[validate(length(
        min = 1,
        max = 2000,
        message = "Write a note (up to 2,000 characters)."
    ))]
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
    /// Notes on one task, oldest first.
    ///
    /// # Errors
    /// On database errors.
    pub async fn list_for_task<C: ConnectionTrait>(
        db: &C,
        task: &tasks::Model,
    ) -> ModelResult<Vec<Self>> {
        Ok(Entity::find()
            .in_tenant(task.organisation_id)
            .filter(Column::TaskId.eq(task.id))
            .order_by_asc(Column::CreatedAt)
            .order_by_asc(Column::Id)
            .all(db)
            .await?)
    }

    /// Number of notes per task id, for the board.
    ///
    /// # Errors
    /// On database errors.
    pub async fn counts_for_org<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
    ) -> ModelResult<HashMap<i64, usize>> {
        let mut counts = HashMap::new();
        for note in Entity::find().in_tenant(org_id).all(db).await? {
            *counts.entry(note.task_id).or_insert(0) += 1;
        }
        Ok(counts)
    }

    /// # Errors
    /// Validation errors, or database errors.
    pub async fn create<C: ConnectionTrait>(
        db: &C,
        task: &tasks::Model,
        author_id: i64,
        params: &NoteParams,
    ) -> ModelResult<Self> {
        let body = params.body.trim().to_string();
        ValidatorTrait::validate(&NoteParams { body: body.clone() })?;
        Ok(ActiveModel {
            body: ActiveValue::Set(body),
            task_id: ActiveValue::Set(task.id),
            author_id: ActiveValue::Set(Some(author_id)),
            ..Default::default()
        }
        .set_tenant(task.organisation_id)?
        .insert(db)
        .await?)
    }
}

impl loco_rs::prelude::TenantEntity for Entity {
    type TenantId = i64;

    fn tenant_column() -> super::_entities::task_notes::Column {
        super::_entities::task_notes::Column::OrganisationId
    }
}
