use std::sync::LazyLock;

use loco_rs::prelude::*;
use serde::Deserialize;

pub use super::_entities::tasks::{ActiveModel, Column, Entity, Model};
use super::{field_error, memberships, projects, task_assignees};

pub type Tasks = Entity;

/// Board columns, in order, with their labels and colours.
pub const STATUSES: [(&str, &str, &str); 4] = [
    ("todo", "To do", "#9a968d"),
    ("progress", "In progress", "#3b6fe0"),
    ("blocked", "Blocked", "#e0603a"),
    ("done", "Done", "#2f9e6b"),
];
pub const PRIORITIES: [&str; 3] = ["high", "medium", "low"];

static PRIORITY_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^(high|medium|low)$").expect("priority regex is valid"));

/// The new and edit task form. The project and date arrive as text so empty
/// choices are allowed (a task without a project is a chore); `assignee_ids` comes from repeated checkbox fields.
#[derive(Debug, Deserialize, Validate)]
pub struct TaskParams {
    #[validate(length(
        min = 1,
        max = 140,
        message = "Describe the task (up to 140 characters)."
    ))]
    pub title: String,
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub assignee_ids: Vec<i64>,
    #[validate(regex(path = *PRIORITY_RE, message = "Choose high, medium or low."))]
    pub priority: String,
    #[serde(default)]
    pub due_on: String,
    /// Empty keeps the current status (or "todo" for a new task).
    #[serde(default)]
    pub status: String,
}

/// The checked, parsed values of a [`TaskParams`].
struct Checked {
    project_id: Option<i64>,
    assignee_ids: Vec<i64>,
    due_on: Option<chrono::NaiveDate>,
}

impl TaskParams {
    async fn check<C: ConnectionTrait>(&self, db: &C, org_id: i64) -> ModelResult<Checked> {
        ValidatorTrait::validate(self)?;
        let project_id = match self.project_id.trim() {
            "" => None,
            raw => {
                let id: i64 = raw
                    .parse()
                    .map_err(|_| field_error("project_id", "Choose a project."))?;
                projects::Model::find_in_org(db, org_id, id)
                    .await
                    .map_err(|_| field_error("project_id", "Choose a project."))?;
                Some(id)
            }
        };
        let mut assignee_ids = Vec::new();
        for id in &self.assignee_ids {
            if !assignee_ids.contains(id) {
                if !memberships::Model::is_active_member(db, org_id, *id).await? {
                    return Err(field_error("assignee_ids", "Choose people from the team."));
                }
                assignee_ids.push(*id);
            }
        }
        let due_on = match self.due_on.trim() {
            "" => None,
            raw => Some(
                chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                    .map_err(|_| field_error("due_on", "Use a valid date."))?,
            ),
        };
        if !self.status.is_empty() && !is_status(&self.status) {
            return Err(field_error("status", "Choose a status."));
        }
        Ok(Checked {
            project_id,
            assignee_ids,
            due_on,
        })
    }
}

#[must_use]
pub fn is_status(value: &str) -> bool {
    STATUSES.iter().any(|(key, _, _)| *key == value)
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
    /// # Errors
    /// On database errors.
    pub async fn list_for_org<C: ConnectionTrait>(db: &C, org_id: i64) -> ModelResult<Vec<Self>> {
        Ok(Entity::find()
            .in_tenant(org_id)
            .order_by_asc(Column::SortOrder)
            .order_by_asc(Column::Id)
            .all(db)
            .await?)
    }

    /// # Errors
    /// `EntityNotFound` when the task is not in this organisation.
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

    /// Adds a task and its assignees together.
    ///
    /// # Errors
    /// Validation errors, or database errors.
    pub async fn create(
        db: &DatabaseConnection,
        org_id: i64,
        params: &TaskParams,
    ) -> ModelResult<Self> {
        let checked = params.check(db, org_id).await?;
        let txn = db.begin().await?;
        let last = Entity::find()
            .in_tenant(org_id)
            .order_by_desc(Column::SortOrder)
            .one(&txn)
            .await?
            .map_or(0, |t| t.sort_order);
        let status = if params.status.is_empty() {
            "todo"
        } else {
            &params.status
        };
        let task = ActiveModel {
            title: ActiveValue::Set(params.title.trim().to_string()),
            status: ActiveValue::Set(status.to_string()),
            priority: ActiveValue::Set(params.priority.clone()),
            due_on: ActiveValue::Set(checked.due_on),
            sort_order: ActiveValue::Set(last + 1),
            project_id: ActiveValue::Set(checked.project_id),
            ..Default::default()
        }
        .set_tenant(org_id)?
        .insert(&txn)
        .await?;
        task_assignees::Model::replace(&txn, org_id, task.id, &checked.assignee_ids).await?;
        txn.commit().await?;
        Ok(task)
    }

    /// Saves an edited task and its assignees together.
    ///
    /// # Errors
    /// Validation errors, or database errors.
    pub async fn update_from(
        self,
        db: &DatabaseConnection,
        params: &TaskParams,
    ) -> ModelResult<Self> {
        let org_id = self.organisation_id;
        let checked = params.check(db, org_id).await?;
        let txn = db.begin().await?;
        let mut task = self.into_active_model();
        task.title = ActiveValue::Set(params.title.trim().to_string());
        task.project_id = ActiveValue::Set(checked.project_id);
        task.priority = ActiveValue::Set(params.priority.clone());
        task.due_on = ActiveValue::Set(checked.due_on);
        if !params.status.is_empty() {
            task.status = ActiveValue::Set(params.status.clone());
        }
        let task = task.update(&txn).await?;
        task_assignees::Model::replace(&txn, org_id, task.id, &checked.assignee_ids).await?;
        txn.commit().await?;
        Ok(task)
    }

    /// Removes the task. Its notes and assignees go with it (cascading keys).
    ///
    /// # Errors
    /// On database errors.
    pub async fn remove<C: ConnectionTrait>(self, db: &C) -> ModelResult<()> {
        self.into_active_model().delete(db).await?;
        Ok(())
    }

    /// Moves the task to another board column.
    ///
    /// # Errors
    /// For an unknown status, or database errors.
    pub async fn set_status<C: ConnectionTrait>(self, db: &C, status: &str) -> ModelResult<Self> {
        if !is_status(status) {
            return Err(ModelError::msg("Unknown status."));
        }
        let mut task = self.into_active_model();
        task.status = ActiveValue::Set(status.to_string());
        Ok(task.update(db).await?)
    }
}

impl loco_rs::prelude::TenantEntity for Entity {
    type TenantId = i64;

    fn tenant_column() -> super::_entities::tasks::Column {
        super::_entities::tasks::Column::OrganisationId
    }
}
