use std::sync::LazyLock;

use loco_rs::prelude::*;
use serde::Deserialize;

pub use super::_entities::tasks::{ActiveModel, Column, Entity, Model};
use super::{field_error, memberships, projects};

pub type Tasks = Entity;

/// Board columns, in order, with their labels and colours from the original design.
pub const STATUSES: [(&str, &str, &str); 4] = [
    ("todo", "To do", "#9ca3af"),
    ("progress", "In progress", "#ffb454"),
    ("blocked", "Blocked", "#ff758f"),
    ("done", "Done", "#72e5b4"),
];
pub const PRIORITIES: [&str; 3] = ["high", "medium", "low"];

static PRIORITY_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^(high|medium|low)$").expect("priority regex is valid"));

/// The new-task form. Ids and the date arrive as text so empty choices are allowed.
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
    pub owner_id: String,
    #[validate(regex(path = *PRIORITY_RE, message = "Choose high, medium or low."))]
    pub priority: String,
    #[serde(default)]
    pub due_on: String,
}

/// Per-person momentum, as in the original app: the average of each task's
/// weight (done 100, in progress 55, blocked 10, to do 15). No tasks scores 0.
#[must_use]
pub fn progress_score<'a>(statuses: impl IntoIterator<Item = &'a str>) -> u32 {
    let (sum, count) = statuses
        .into_iter()
        .fold((0u32, 0u32), |(sum, count), status| {
            let weight = match status {
                "done" => 100,
                "progress" => 55,
                "blocked" => 10,
                _ => 15,
            };
            (sum + weight, count + 1)
        });
    if count == 0 {
        0
    } else {
        (f64::from(sum) / f64::from(count)).round() as u32
    }
}

/// Share of tasks that are done, as a whole percentage.
#[must_use]
pub fn completion<'a>(statuses: impl IntoIterator<Item = &'a str>) -> u32 {
    let (done, total) = statuses.into_iter().fold((0u32, 0u32), |(done, total), s| {
        (done + u32::from(s == "done"), total + 1)
    });
    if total == 0 {
        0
    } else {
        (f64::from(done) * 100.0 / f64::from(total)).round() as u32
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

    /// # Errors
    /// Validation errors, or database errors.
    pub async fn create<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        params: &TaskParams,
    ) -> ModelResult<Self> {
        ValidatorTrait::validate(params)?;
        let project_id: i64 = params
            .project_id
            .trim()
            .parse()
            .map_err(|_| field_error("project_id", "Choose a project."))?;
        projects::Model::find_in_org(db, org_id, project_id)
            .await
            .map_err(|_| field_error("project_id", "Choose a project."))?;
        let owner_id = match params.owner_id.trim() {
            "" => None,
            raw => {
                let id: i64 = raw
                    .parse()
                    .map_err(|_| field_error("owner_id", "Choose someone from the team."))?;
                if !memberships::Model::is_active_member(db, org_id, id).await? {
                    return Err(field_error("owner_id", "Choose someone from the team."));
                }
                Some(id)
            }
        };
        let due_on = match params.due_on.trim() {
            "" => None,
            raw => Some(
                chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                    .map_err(|_| field_error("due_on", "Use a valid date."))?,
            ),
        };
        let last = Entity::find()
            .in_tenant(org_id)
            .order_by_desc(Column::SortOrder)
            .one(db)
            .await?
            .map_or(0, |t| t.sort_order);
        Ok(ActiveModel {
            title: ActiveValue::Set(params.title.trim().to_string()),
            status: ActiveValue::Set("todo".to_string()),
            priority: ActiveValue::Set(params.priority.clone()),
            due_on: ActiveValue::Set(due_on),
            sort_order: ActiveValue::Set(last + 1),
            project_id: ActiveValue::Set(project_id),
            owner_id: ActiveValue::Set(owner_id),
            ..Default::default()
        }
        .set_tenant(org_id)?
        .insert(db)
        .await?)
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

#[cfg(test)]
mod tests {
    use super::{completion, progress_score};

    #[test]
    fn progress_score_matches_the_original_weights() {
        assert_eq!(progress_score([]), 0);
        assert_eq!(progress_score(["done"]), 100);
        assert_eq!(progress_score(["done", "todo"]), 58); // (100 + 15) / 2 = 57.5
        assert_eq!(progress_score(["progress", "blocked", "todo"]), 27); // 80 / 3 = 26.7
    }

    #[test]
    fn completion_is_the_share_done() {
        assert_eq!(completion([]), 0);
        assert_eq!(completion(["done", "todo", "progress"]), 33);
        assert_eq!(completion(["done", "done"]), 100);
    }
}
