use std::sync::LazyLock;

use loco_rs::prelude::*;
use serde::Deserialize;

pub use super::_entities::tasks::{ActiveModel, Column, Entity, Model};
use super::{
    field_error, memberships, note_reactions, notifications,
    order::{self, Place, Slot},
    projects, task_assignees, task_notes,
};

pub type Tasks = Entity;

/// How long Claude can undo what its own connection made.
pub const UNDO_HOURS: i64 = 24;

/// What a hard delete removed, for the tool result and the log.
#[derive(Debug)]
pub struct Removed {
    pub id: i64,
    /// The task's title or the project's name.
    pub name: String,
    /// Tasks deleted: 1 for a task, the project's tasks for a project.
    pub tasks: usize,
    /// People whose notifications went with it, so their bells can refresh.
    pub notified: Vec<i64>,
}

/// A move that changed nothing: the card it was placed against isn't in
/// that column (any more, or ever, as for another organisation's card).
#[derive(Debug, PartialEq, Eq)]
pub struct Stale;

/// What a move did.
#[derive(Debug)]
pub struct Moved {
    pub task: Model,
    /// It went to another column, not only to another place in its own.
    pub changed_status: bool,
}

/// The number that puts a card at the bottom of any column: past the
/// organisation's highest. Not under the order lock: two at once can tie,
/// and the id keeps that stable until a move renumbers it.
///
/// # Errors
/// On database errors.
pub async fn next_sort<C: ConnectionTrait>(db: &C, org_id: i64) -> ModelResult<i64> {
    let last = Entity::find()
        .in_tenant(org_id)
        .order_by_desc(Column::SortOrder)
        .one(db)
        .await?
        .map_or(0, |t| t.sort_order);
    Ok(last.saturating_add(order::STEP))
}

/// Why Claude may not delete something.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not made by this connection in the last [`UNDO_HOURS`].
    NotMadeHere,
    /// A project with tasks made by someone else (the web app or another
    /// connection).
    OthersTasks,
    /// Someone else's note, or reaction to a note, is on it.
    OthersContent,
}

/// Whether a row was made by this connection within [`UNDO_HOURS`]. The
/// grant is compared as `Some(grant)`, so a web-made row (NULL) never matches.
#[must_use]
pub fn made_lately(row_grant: Option<Uuid>, created_at: DateTimeWithTimeZone, grant: Uuid) -> bool {
    row_grant == Some(grant)
        && created_at > chrono::Utc::now() - chrono::Duration::hours(UNDO_HOURS)
}

/// Whether anyone but `user_id` has a note, or a reaction to a note, on these
/// tasks. A note whose author was deleted (NULL) counts as someone else's.
/// Run it inside the delete's transaction, after locking the tasks: the
/// notes are locked `FOR UPDATE` too, so a reaction being added (its foreign
/// key check needs the note) waits for the delete instead of being cascaded
/// away unseen.
///
/// # Errors
/// On database errors.
pub async fn others_content<C: ConnectionTrait>(
    db: &C,
    task_ids: &[i64],
    user_id: i64,
) -> ModelResult<bool> {
    let notes: Vec<(i64, Option<i64>)> = task_notes::Entity::find()
        .select_only()
        .column(task_notes::Column::Id)
        .column(task_notes::Column::AuthorId)
        .filter(task_notes::Column::TaskId.is_in(task_ids.iter().copied()))
        .lock_exclusive()
        .into_tuple()
        .all(db)
        .await?;
    if notes.iter().any(|(_, author)| *author != Some(user_id)) {
        return Ok(true);
    }
    let reactions = note_reactions::Entity::find()
        .filter(note_reactions::Column::TaskNoteId.is_in(notes.iter().map(|(id, _)| *id)))
        .filter(note_reactions::Column::UserId.ne(user_id))
        .count(db)
        .await?;
    Ok(reactions > 0)
}

/// Deletes this organisation's notifications that link to these tasks, and
/// returns whose they were. Use ids read inside the delete's transaction.
///
/// # Errors
/// On database errors.
pub async fn drop_notices<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    task_ids: &[i64],
) -> ModelResult<Vec<i64>> {
    if task_ids.is_empty() {
        return Ok(Vec::new());
    }
    let links: Vec<String> = task_ids
        .iter()
        .map(|id| format!("/tasks?open={id}"))
        .collect();
    let mut users: Vec<i64> = notifications::Entity::delete_many()
        .filter(notifications::Column::OrganisationId.eq(org_id))
        .filter(notifications::Column::Link.is_in(links))
        .exec_with_returning(db)
        .await?
        .into_iter()
        .map(|n| n.user_id)
        .collect();
    users.sort_unstable();
    users.dedup();
    Ok(users)
}

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
        let mut this = self;
        if !insert && this.updated_at.is_unchanged() {
            this.updated_at = ActiveValue::Set(chrono::Utc::now().into());
        }
        // Every route that changes the status saves through here, so this is
        // the one place that records when a task was finished.
        if let Some(status) = this.status.try_as_ref() {
            let done_at = this.done_at.try_as_ref().copied().flatten();
            if status != "done" && done_at.is_some() {
                this.done_at = ActiveValue::Set(None);
            } else if status == "done" && done_at.is_none() {
                this.done_at = ActiveValue::Set(Some(chrono::Utc::now().into()));
            }
        }
        // A status set to anything but done means someone changed the task
        // since a close, so a reopen leaves it alone.
        if let ActiveValue::Set(status) = &this.status {
            if status != "done" {
                this.status_before_close = ActiveValue::Set(None);
            }
        }
        Ok(this)
    }
}

impl Model {
    /// How many tasks a project has.
    ///
    /// # Errors
    /// On database errors.
    pub async fn count_in_project<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        project_id: i64,
    ) -> ModelResult<u64> {
        Ok(Entity::find()
            .in_tenant(org_id)
            .filter(Column::ProjectId.eq(project_id))
            .count(db)
            .await?)
    }

    /// How many tasks this Claude connection made after `since`, for the
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

    /// Adds a task and its assignees together. `claude_grant` is the Claude
    /// connection making it, `None` from the web app (see
    /// [`super::projects::Model::create`]). A closed project takes no new
    /// tasks.
    ///
    /// # Errors
    /// Validation errors, or database errors.
    pub async fn create(
        db: &DatabaseConnection,
        org_id: i64,
        params: &TaskParams,
        claude_grant: Option<Uuid>,
    ) -> ModelResult<Self> {
        let checked = params.check(db, org_id).await?;
        let txn = db.begin().await?;
        if let Some(id) = checked.project_id {
            projects::lock_open_for_tasks(&txn, org_id, id).await?;
        }
        let sort_order = next_sort(&txn, org_id).await?;
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
            sort_order: ActiveValue::Set(sort_order),
            project_id: ActiveValue::Set(checked.project_id),
            claude_grant_id: ActiveValue::Set(claude_grant),
            ..Default::default()
        }
        .set_tenant(org_id)?
        .insert(&txn)
        .await?;
        task_assignees::Model::replace(&txn, org_id, task.id, &checked.assignee_ids).await?;
        txn.commit().await?;
        Ok(task)
    }

    /// Saves an edited task and its assignees together. It can't move into a
    /// closed project, but a task already in one can still be edited.
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
        let moved = checked.project_id != self.project_id;
        if let (true, Some(id)) = (moved, checked.project_id) {
            projects::lock_open_for_tasks(&txn, org_id, id).await?;
        }
        // A new status puts it at the bottom of its new column.
        let new_column = !params.status.is_empty() && params.status != self.status;
        let bottom = if new_column {
            Some(next_sort(&txn, org_id).await?)
        } else {
            None
        };
        let mut task = self.into_active_model();
        if let Some(n) = bottom {
            task.sort_order = ActiveValue::Set(n);
        }
        // A task moved elsewhere must not carry a saved status into another
        // project's reopen.
        if moved {
            task.status_before_close = ActiveValue::Set(None);
        }
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

    /// Deletes a task Claude made, for that same connection (`grant`), within
    /// [`UNDO_HOURS`], when no one but `user_id` has added notes or reactions
    /// to it. Notes, assignees and reactions cascade; notifications that link
    /// to it are deleted too. The task is locked first, so the checks and the
    /// delete see the same row.
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
        let task = Entity::find_by_id(id)
            .in_tenant(org_id)
            .lock_exclusive()
            .one(&txn)
            .await?
            .ok_or(ModelError::EntityNotFound)?;
        // An early return drops `txn`, which rolls it back.
        if !made_lately(task.claude_grant_id, task.created_at, grant) {
            return Ok(Err(Refusal::NotMadeHere));
        }
        if others_content(&txn, &[task.id], user_id).await? {
            return Ok(Err(Refusal::OthersContent));
        }
        let notified = drop_notices(&txn, org_id, &[task.id]).await?;
        let removed = Removed {
            id: task.id,
            name: task.title.clone(),
            tasks: 1,
            notified,
        };
        task.into_active_model().delete(&txn).await?;
        txn.commit().await?;
        Ok(Ok(removed))
    }

    /// Moves the task to another board column, at the bottom of it.
    ///
    /// # Errors
    /// For an unknown status, or database errors.
    pub async fn set_status<C: ConnectionTrait>(self, db: &C, status: &str) -> ModelResult<Self> {
        if !is_status(status) {
            return Err(ModelError::msg("Unknown status."));
        }
        let bottom = if self.status == status {
            None
        } else {
            Some(next_sort(db, self.organisation_id).await?)
        };
        let mut task = self.into_active_model();
        task.status = ActiveValue::Set(status.to_string());
        if let Some(n) = bottom {
            task.sort_order = ActiveValue::Set(n);
        }
        Ok(task.update(db).await?)
    }

    /// Puts a task at `place` in the `status` column, from a drag or the
    /// Reorder arrows. The column is every task in the organisation with that
    /// status, whatever the person's view filters out, so hidden cards keep
    /// their places. Moves in one organisation run one at a time
    /// ([`order::lock`]). A move inside a column isn't an edit, so it leaves
    /// `updated_at` alone; one into another column saves the status through
    /// `before_save`, as any status change does.
    ///
    /// # Errors
    /// A message for Done (it keeps its own order), an unknown status, or an
    /// anchor that is the card itself; `EntityNotFound` when the task isn't
    /// in this organisation; database errors, including a lock timeout
    /// ([`order::is_busy`]).
    pub async fn move_to(
        db: &DatabaseConnection,
        org_id: i64,
        id: i64,
        status: &str,
        place: Place,
    ) -> ModelResult<Result<Moved, Stale>> {
        if status == "done" {
            return Err(ModelError::msg("Done keeps its own order."));
        }
        if !is_status(status) {
            return Err(ModelError::msg("Unknown status."));
        }
        if matches!(place, Place::Before(a) | Place::After(a) if a == id) {
            return Err(ModelError::msg("Choose another card to move it next to."));
        }
        let txn = db.begin().await?;
        order::lock(&txn, "tasks", org_id).await?;
        let task = Entity::find_by_id(id)
            .in_tenant(org_id)
            .lock_exclusive()
            .one(&txn)
            .await?
            .ok_or(ModelError::EntityNotFound)?;
        let column: Vec<(i64, i64)> = Entity::find()
            .select_only()
            .column(Column::Id)
            .column(Column::SortOrder)
            .in_tenant(org_id)
            .filter(Column::Status.eq(status))
            .filter(Column::Id.ne(id))
            .order_by_asc(Column::SortOrder)
            .order_by_asc(Column::Id)
            .into_tuple()
            .all(&txn)
            .await?;
        // An early return drops `txn`, which rolls it back.
        let Some(slot) = order::slot(&column, id, place) else {
            return Ok(Err(Stale));
        };
        let number = match slot {
            Slot::At(n) => n,
            Slot::Renumber(rows) => {
                let (mine, others): (Vec<_>, Vec<_>) =
                    rows.into_iter().partition(|(r, _)| *r == id);
                order::write_renumber(
                    &txn,
                    "tasks",
                    org_id,
                    &others,
                    "t.status = $4",
                    vec![status.into()],
                )
                .await?;
                mine.first().map_or(order::STEP, |(_, n)| *n)
            }
        };
        let changed_status = task.status != status;
        let task = if changed_status {
            let mut active = task.into_active_model();
            active.status = ActiveValue::Set(status.to_string());
            active.sort_order = ActiveValue::Set(number);
            active.update(&txn).await?
        } else {
            Entity::update_many()
                .col_expr(Column::SortOrder, Expr::value(number))
                .filter(Column::Id.eq(id))
                .filter(Column::OrganisationId.eq(org_id))
                .exec(&txn)
                .await?;
            Model {
                sort_order: number,
                ..task
            }
        };
        txn.commit().await?;
        Ok(Ok(Moved {
            task,
            changed_status,
        }))
    }
}

impl loco_rs::prelude::TenantEntity for Entity {
    type TenantId = i64;

    fn tenant_column() -> super::_entities::tasks::Column {
        super::_entities::tasks::Column::OrganisationId
    }
}
