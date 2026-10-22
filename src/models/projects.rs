use std::sync::LazyLock;

use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};

pub use super::_entities::projects::{ActiveModel, Column, Entity, Model};
use super::{
    field_error, memberships,
    tasks::{self, Refusal, Removed},
};

pub type Projects = Entity;

/// How far along a project is: the share of its tasks that are done, rounded
/// down so 100% means everything is finished.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub percent: usize,
}

impl Progress {
    #[must_use]
    pub fn of(project_id: i64, all_tasks: &[tasks::Model]) -> Self {
        let theirs = all_tasks
            .iter()
            .filter(|t| t.project_id == Some(project_id));
        let total = theirs.clone().count();
        let done = theirs.filter(|t| t.status == "done").count();
        Self {
            done,
            total,
            percent: (done * 100).checked_div(total).unwrap_or(0),
        }
    }
}

pub const LANES: [&str; 3] = ["now", "next", "later"];
pub const ACCENTS: [&str; 6] = [
    "#ffb454", "#df85ff", "#72e5b4", "#6cb6ff", "#ff758f", "#c9d76a",
];

static LANE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^(now|next|later)$").expect("lane regex is valid"));
static ACCENT_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^#[0-9a-fA-F]{6}$").expect("accent regex is valid"));

/// The project form. `owner_id` arrives as text so "Unassigned" can be an empty string.
#[derive(Debug, Deserialize, Validate)]
pub struct ProjectParams {
    #[validate(length(
        min = 1,
        max = 80,
        message = "Give the project a name (up to 80 characters)."
    ))]
    pub name: String,
    #[validate(regex(path = *LANE_RE, message = "Choose now, next or later."))]
    pub lane: String,
    #[validate(length(
        min = 1,
        max = 40,
        message = "Add a short status (up to 40 characters)."
    ))]
    pub status: String,
    #[validate(regex(path = *ACCENT_RE, message = "Choose a colour."))]
    pub accent: String,
    #[serde(default)]
    pub owner_id: String,
    #[serde(default)]
    #[validate(length(max = 500, message = "Keep the summary under 500 characters."))]
    pub summary: String,
}

impl ProjectParams {
    /// The chosen owner, which must be an approved member of the organisation.
    async fn owner<C: ConnectionTrait>(&self, db: &C, org_id: i64) -> ModelResult<Option<i64>> {
        let raw = self.owner_id.trim();
        if raw.is_empty() {
            return Ok(None);
        }
        let id: i64 = raw
            .parse()
            .map_err(|_| field_error("owner_id", "Choose someone from the team."))?;
        if memberships::Model::is_active_member(db, org_id, id).await? {
            Ok(Some(id))
        } else {
            Err(field_error("owner_id", "Choose someone from the team."))
        }
    }
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
    /// How many projects this Claude connection made after `since`, for the
    /// brake on bulk creates.
    ///
    /// # Errors
    /// On database errors.
    pub async fn count_made_by<C: ConnectionTrait>(
        db: &C,
        grant: Uuid,
        since: chrono::DateTime<chrono::Utc>,
    ) -> ModelResult<u64> {
        Ok(Entity::find()
            .filter(Column::ClaudeGrantId.eq(grant))
            .filter(Column::CreatedAt.gt(since))
            .count(db)
            .await?)
    }

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
    /// `EntityNotFound` when the project is not in this organisation.
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

    /// Adds a project at the end of its organisation's list. `claude_grant` is
    /// the Claude connection making it, `None` from the web app. It is an
    /// argument, not a form field, so no form can set it.
    ///
    /// # Errors
    /// Validation errors, or database errors.
    pub async fn create<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        params: &ProjectParams,
        claude_grant: Option<Uuid>,
    ) -> ModelResult<Self> {
        ValidatorTrait::validate(params)?;
        let owner_id = params.owner(db, org_id).await?;
        let last = Entity::find()
            .in_tenant(org_id)
            .order_by_desc(Column::SortOrder)
            .one(db)
            .await?
            .map_or(0, |p| p.sort_order);
        let mut project = ActiveModel {
            sort_order: ActiveValue::Set(last + 1),
            claude_grant_id: ActiveValue::Set(claude_grant),
            ..Default::default()
        }
        .set_tenant(org_id)?;
        apply(&mut project, params, owner_id);
        Ok(project.insert(db).await?)
    }

    /// Deletes a project with its tasks, their notes, assignees and
    /// reactions (cascading keys), and the notifications that link to those
    /// tasks. For owners and admins in the web app. Can't be undone.
    ///
    /// # Errors
    /// `EntityNotFound` when it is not in this organisation, or database
    /// errors.
    pub async fn remove(db: &DatabaseConnection, org_id: i64, id: i64) -> ModelResult<Removed> {
        let txn = db.begin().await?;
        let (project, task_ids) = lock_with_tasks(&txn, org_id, id).await?;
        let removed = delete_locked(&txn, project, &task_ids).await?;
        txn.commit().await?;
        Ok(removed)
    }

    /// Deletes a project Claude made, for that same connection (`grant`),
    /// within [`tasks::UNDO_HOURS`], when every task in it was made by the
    /// same connection and no one but `user_id` has added notes or reactions.
    ///
    /// # Errors
    /// `EntityNotFound` when it is not in this organisation, or database
    /// errors.
    pub async fn remove_made_by(
        db: &DatabaseConnection,
        org_id: i64,
        id: i64,
        grant: Uuid,
        user_id: i64,
    ) -> ModelResult<Result<Removed, Refusal>> {
        let txn = db.begin().await?;
        let (project, locked) = lock_with_tasks(&txn, org_id, id).await?;
        // An early return drops `txn`, which rolls it back.
        if !tasks::made_lately(project.claude_grant_id, project.created_at, grant) {
            return Ok(Err(Refusal::NotMadeHere));
        }
        if locked.iter().any(|(_, g)| *g != Some(grant)) {
            return Ok(Err(Refusal::OthersTasks));
        }
        let ids: Vec<i64> = locked.iter().map(|(id, _)| *id).collect();
        if tasks::others_content(&txn, &ids, user_id).await? {
            return Ok(Err(Refusal::OthersContent));
        }
        let removed = delete_locked(&txn, project, &locked).await?;
        txn.commit().await?;
        Ok(Ok(removed))
    }

    /// # Errors
    /// Validation errors, or database errors.
    pub async fn update_from<C: ConnectionTrait>(
        self,
        db: &C,
        params: &ProjectParams,
    ) -> ModelResult<Self> {
        ValidatorTrait::validate(params)?;
        let owner_id = params.owner(db, self.organisation_id).await?;
        let mut project = self.into_active_model();
        apply(&mut project, params, owner_id);
        Ok(project.update(db).await?)
    }
}

/// Locks a project, then its tasks, `FOR UPDATE`. A task being added to the
/// project, or moved into it, waits on the project row (its foreign key
/// check), so the task list can't change underneath a delete. Returns the
/// tasks as `(id, claude_grant_id)`.
async fn lock_with_tasks<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: i64,
) -> ModelResult<(Model, Vec<(i64, Option<Uuid>)>)> {
    let project = Entity::find_by_id(id)
        .in_tenant(org_id)
        .lock_exclusive()
        .one(db)
        .await?
        .ok_or(ModelError::EntityNotFound)?;
    let task_ids = tasks::Entity::find()
        .select_only()
        .column(tasks::Column::Id)
        .column(tasks::Column::ClaudeGrantId)
        .filter(tasks::Column::ProjectId.eq(project.id))
        .filter(tasks::Column::OrganisationId.eq(org_id))
        .lock_exclusive()
        .into_tuple()
        .all(db)
        .await?;
    Ok((project, task_ids))
}

/// Deletes a locked project and the notifications for its locked tasks.
async fn delete_locked<C: ConnectionTrait>(
    db: &C,
    project: Model,
    task_ids: &[(i64, Option<Uuid>)],
) -> ModelResult<Removed> {
    let ids: Vec<i64> = task_ids.iter().map(|(id, _)| *id).collect();
    let notified = tasks::drop_notices(db, project.organisation_id, &ids).await?;
    let removed = Removed {
        id: project.id,
        name: project.name.clone(),
        tasks: ids.len(),
        notified,
    };
    project.into_active_model().delete(db).await?;
    Ok(removed)
}

fn apply(project: &mut ActiveModel, params: &ProjectParams, owner_id: Option<i64>) {
    project.name = ActiveValue::Set(params.name.trim().to_string());
    project.lane = ActiveValue::Set(params.lane.clone());
    project.status = ActiveValue::Set(params.status.trim().to_string());
    project.accent = ActiveValue::Set(params.accent.to_lowercase());
    project.owner_id = ActiveValue::Set(owner_id);
    project.summary = ActiveValue::Set(params.summary.trim().to_string());
}

impl loco_rs::prelude::TenantEntity for Entity {
    type TenantId = i64;

    fn tenant_column() -> super::_entities::projects::Column {
        super::_entities::projects::Column::OrganisationId
    }
}
