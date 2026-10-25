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

/// What closing a project would do, worked out from its tasks for the
/// confirm step: how many of each status it has now.
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
pub struct CloseSummary {
    pub todo: usize,
    pub progress: usize,
    pub blocked: usize,
    pub done: usize,
}

impl CloseSummary {
    #[must_use]
    pub fn of(project_id: i64, all_tasks: &[tasks::Model]) -> Self {
        let mut summary = Self::default();
        for t in all_tasks
            .iter()
            .filter(|t| t.project_id == Some(project_id))
        {
            match t.status.as_str() {
                "done" => summary.done += 1,
                "progress" => summary.progress += 1,
                "blocked" => summary.blocked += 1,
                _ => summary.todo += 1,
            }
        }
        summary
    }

    /// Tasks that a close would mark done.
    #[must_use]
    pub const fn open(&self) -> usize {
        self.todo + self.progress + self.blocked
    }

    #[must_use]
    pub const fn total(&self) -> usize {
        self.open() + self.done
    }

    /// The confirm step's sentence: "4 open tasks will be marked done. 1 is blocked."
    #[must_use]
    pub fn sentence(&self) -> String {
        let open = self.open();
        match (open, self.total()) {
            (0, 0) => "It has no tasks.".to_string(),
            (0, 1) => "Its 1 task is already done.".to_string(),
            (0, total) => format!("All {total} tasks are already done."),
            _ => {
                let lead = format!("{open} open {} will be marked done.", tasks_word(open));
                match (open, self.blocked) {
                    (_, 0) => lead,
                    (1, _) => format!("{lead} It is blocked."),
                    (_, 1) => format!("{lead} 1 is blocked."),
                    (_, n) => format!("{lead} {n} are blocked."),
                }
            }
        }
    }

    /// The line in the "Mark project done" box, before anyone taps it.
    #[must_use]
    pub fn box_line(&self) -> String {
        match self.open() {
            0 => self.sentence(),
            1 => "Its 1 open task is marked done with it.".to_string(),
            n => format!("Its {n} open tasks are marked done with it."),
        }
    }
}

fn tasks_word(n: usize) -> &'static str {
    if n == 1 {
        "task"
    } else {
        "tasks"
    }
}

/// Who a close would notify, as the confirm step says it: "Maya and Tom get
/// a notice.", or "Maya, Tom and 3 others get a notice." for five or more.
#[must_use]
pub fn notice_line(names: &[String]) -> String {
    match names {
        [] => "No one else gets a notice.".to_string(),
        [one] => format!("{one} gets a notice."),
        [first, second, rest @ ..] if rest.len() >= 3 => {
            format!("{first}, {second} and {} others get a notice.", rest.len())
        }
        [init @ .., last] => format!("{} and {last} get a notice.", init.join(", ")),
    }
}

/// The refusal for adding or moving a task into a closed project, or editing
/// a closed project. One string for the web and Claude.
#[must_use]
pub fn closed_refusal(name: &str) -> String {
    format!("{name} is done. Reopen the project first.")
}

/// "Now", "Next" or "Later" for a lane key.
#[must_use]
pub fn lane_label(lane: &str) -> &'static str {
    match lane {
        "now" => "Now",
        "next" => "Next",
        _ => "Later",
    }
}

/// What a close did. `tasks` are the rows it marked done, so the toast and
/// the notices count what really changed. `already` means it was closed
/// before, and nothing changed.
#[derive(Debug)]
pub struct Closed {
    pub project: Model,
    pub tasks: Vec<tasks::Model>,
    pub already: bool,
}

/// What a reopen did. `tasks` are the rows that went back to their earlier
/// status, so the toast and the notices count what really changed.
#[derive(Debug)]
pub struct Reopened {
    pub project: Model,
    pub tasks: Vec<tasks::Model>,
    pub already: bool,
}

pub const LANES: [&str; 3] = ["now", "next", "later"];
/// The roadmap's fourth column. Not a lane: a project is closed, and keeps
/// its lane for a reopen, so no form or Claude call can set it.
pub const DONE: &str = "done";
/// How many projects the Done column shows before "Show all".
pub const DONE_SHOWN: usize = 5;
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

    /// Moves the project to another lane, from a drag on the roadmap. The
    /// caller refuses a closed project first.
    ///
    /// # Errors
    /// A message when `lane` is not now, next or later, or database errors.
    pub async fn move_to_lane<C: ConnectionTrait>(self, db: &C, lane: &str) -> ModelResult<Self> {
        if !LANES.contains(&lane) {
            return Err(ModelError::msg("Choose now, next or later."));
        }
        let mut project = self.into_active_model();
        project.lane = ActiveValue::Set(lane.to_string());
        Ok(project.update(db).await?)
    }

    /// Marked done. A closed project keeps its lane but shows in none.
    #[must_use]
    pub const fn is_closed(&self) -> bool {
        self.closed_at.is_some()
    }

    /// Whether the project shows in `lane`: it sits there and isn't closed.
    /// Every lane check in the app goes through this.
    #[must_use]
    pub fn in_lane(&self, lane: &str) -> bool {
        !self.is_closed() && self.lane == lane
    }

    /// Who can close and reopen it: owners, admins and the project's owner.
    /// A close changes other people's tasks, so it is a step above editing.
    #[must_use]
    pub fn closable_by(&self, can_manage: bool, user_id: i64) -> bool {
        can_manage || self.owner_id == Some(user_id)
    }

    /// Marks the project done, and every task in it that isn't done yet, in
    /// one transaction. Each task remembers its status in
    /// `status_before_close` for a reopen. Tasks already done are left as they
    /// are. Closing a closed project changes nothing.
    ///
    /// The tasks are written in one statement, past `before_save`, so it sets
    /// `done_at` itself. Postgres reads the old row on the right of `SET`, so
    /// `status_before_close` gets the status from before.
    ///
    /// # Errors
    /// `EntityNotFound` when it is not in this organisation, or database
    /// errors.
    pub async fn close(
        db: &DatabaseConnection,
        org_id: i64,
        id: i64,
        user_id: i64,
    ) -> ModelResult<Closed> {
        let txn = db.begin().await?;
        let (project, _) = lock_with_tasks(&txn, org_id, id).await?;
        // An early return drops `txn`, which rolls it back.
        if project.is_closed() {
            return Ok(Closed {
                project,
                tasks: Vec::new(),
                already: true,
            });
        }
        let now: DateTimeWithTimeZone = chrono::Utc::now().into();
        let closed = tasks::Entity::update_many()
            .col_expr(
                tasks::Column::StatusBeforeClose,
                Expr::col(tasks::Column::Status),
            )
            .col_expr(tasks::Column::Status, Expr::value("done"))
            .col_expr(tasks::Column::DoneAt, Expr::value(now))
            .col_expr(tasks::Column::UpdatedAt, Expr::value(now))
            .filter(tasks::Column::OrganisationId.eq(org_id))
            .filter(tasks::Column::ProjectId.eq(project.id))
            .filter(tasks::Column::Status.ne("done"))
            .exec_with_returning(&txn)
            .await?;
        let mut active = project.into_active_model();
        active.closed_at = ActiveValue::Set(Some(now));
        active.closed_by_id = ActiveValue::Set(Some(user_id));
        let project = active.update(&txn).await?;
        txn.commit().await?;
        Ok(Closed {
            project,
            tasks: closed,
            already: false,
        })
    }

    /// Puts a closed project back in its lane. Tasks the close marked done go
    /// back to their earlier status, unless someone changed them since (that
    /// cleared the saved status). Tasks done before the close stay done.
    /// Reopening an open project changes nothing.
    ///
    /// # Errors
    /// `EntityNotFound` when it is not in this organisation, or database
    /// errors.
    pub async fn reopen(db: &DatabaseConnection, org_id: i64, id: i64) -> ModelResult<Reopened> {
        let txn = db.begin().await?;
        let (project, _) = lock_with_tasks(&txn, org_id, id).await?;
        if !project.is_closed() {
            return Ok(Reopened {
                project,
                tasks: Vec::new(),
                already: true,
            });
        }
        let now: DateTimeWithTimeZone = chrono::Utc::now().into();
        let restored = tasks::Entity::update_many()
            .col_expr(
                tasks::Column::Status,
                Expr::col(tasks::Column::StatusBeforeClose),
            )
            .col_expr(
                tasks::Column::StatusBeforeClose,
                Expr::value(Option::<String>::None),
            )
            .col_expr(
                tasks::Column::DoneAt,
                Expr::value(Option::<DateTimeWithTimeZone>::None),
            )
            .col_expr(tasks::Column::UpdatedAt, Expr::value(now))
            .filter(tasks::Column::OrganisationId.eq(org_id))
            .filter(tasks::Column::ProjectId.eq(project.id))
            .filter(tasks::Column::Status.eq("done"))
            .filter(tasks::Column::StatusBeforeClose.is_not_null())
            .exec_with_returning(&txn)
            .await?;
        // Any saved status left (there should be none) would be stale by the
        // next close.
        tasks::Entity::update_many()
            .col_expr(
                tasks::Column::StatusBeforeClose,
                Expr::value(Option::<String>::None),
            )
            .filter(tasks::Column::OrganisationId.eq(org_id))
            .filter(tasks::Column::ProjectId.eq(project.id))
            .filter(tasks::Column::StatusBeforeClose.is_not_null())
            .exec(&txn)
            .await?;
        let mut active = project.into_active_model();
        active.closed_at = ActiveValue::Set(None);
        active.closed_by_id = ActiveValue::Set(None);
        let project = active.update(&txn).await?;
        txn.commit().await?;
        Ok(Reopened {
            project,
            tasks: restored,
            already: false,
        })
    }
}

/// Reads a project for a task being added to it, or moved into it, inside
/// that task's transaction, `FOR SHARE`, and refuses a closed one.
///
/// A plain read would race with a close: the check passes, the close
/// commits, then the insert lands in a closed project. `FOR SHARE` waits for
/// a close holding `FOR UPDATE`, then reads the committed row. The other way
/// round, a close waits for this transaction, and its task lock then sees the
/// new task and closes it too. Both lock the project first, then tasks, so
/// they can't deadlock.
///
/// # Errors
/// A `project_id` field error for a closed project, or one not in this
/// organisation; database errors.
pub async fn lock_open_for_tasks<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: i64,
) -> ModelResult<Model> {
    let project = Entity::find_by_id(id)
        .in_tenant(org_id)
        .lock_shared()
        .one(db)
        .await?
        .ok_or_else(|| field_error("project_id", "Choose a project."))?;
    if project.is_closed() {
        return Err(field_error("project_id", &closed_refusal(&project.name)));
    }
    Ok(project)
}

/// Locks a project, then its tasks, `FOR UPDATE`, for a delete, a close or a
/// reopen. A task being added to the project, or moved into it, waits on the
/// project row (its `FOR SHARE` read in [`lock_open_for_tasks`], and its
/// foreign key check), so the task list can't change underneath. Returns the
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

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(todo: usize, progress: usize, blocked: usize, done: usize) -> CloseSummary {
        CloseSummary {
            todo,
            progress,
            blocked,
            done,
        }
    }

    fn project(lane: &str, closed: bool, owner_id: Option<i64>) -> Model {
        let now: DateTimeWithTimeZone = chrono::Utc::now().into();
        Model {
            created_at: now,
            updated_at: now,
            id: 1,
            name: "Website".to_string(),
            lane: lane.to_string(),
            status: "Active".to_string(),
            accent: ACCENTS[0].to_string(),
            summary: String::new(),
            sort_order: 1,
            organisation_id: 1,
            owner_id,
            claude_grant_id: None,
            closed_at: closed.then_some(now),
            closed_by_id: None,
        }
    }

    #[test]
    fn close_sentence_matches_the_mockup() {
        let cases = [
            (
                summary(1, 2, 1, 8),
                "4 open tasks will be marked done. 1 is blocked.",
            ),
            (
                summary(1, 0, 2, 0),
                "3 open tasks will be marked done. 2 are blocked.",
            ),
            (
                summary(0, 0, 1, 3),
                "1 open task will be marked done. It is blocked.",
            ),
            (summary(1, 1, 0, 0), "2 open tasks will be marked done."),
            (summary(0, 0, 0, 9), "All 9 tasks are already done."),
            (summary(0, 0, 0, 0), "It has no tasks."),
        ];
        for (s, sentence) in cases {
            assert_eq!(s.sentence(), sentence);
        }
    }

    #[test]
    fn box_line_and_notice_line() {
        assert_eq!(
            summary(1, 2, 1, 0).box_line(),
            "Its 4 open tasks are marked done with it."
        );
        assert_eq!(
            summary(0, 1, 0, 2).box_line(),
            "Its 1 open task is marked done with it."
        );
        assert_eq!(
            summary(0, 0, 0, 2).box_line(),
            "All 2 tasks are already done."
        );

        let names = |list: &[&str]| list.iter().map(|n| (*n).to_string()).collect::<Vec<_>>();
        assert_eq!(notice_line(&[]), "No one else gets a notice.");
        assert_eq!(notice_line(&names(&["Maya"])), "Maya gets a notice.");
        assert_eq!(
            notice_line(&names(&["Maya", "Tom"])),
            "Maya and Tom get a notice."
        );
        assert_eq!(
            notice_line(&names(&["Maya", "Tom", "Leo", "Ana"])),
            "Maya, Tom, Leo and Ana get a notice."
        );
        assert_eq!(
            notice_line(&names(&["Maya", "Tom", "Leo", "Ana", "Sam"])),
            "Maya, Tom and 3 others get a notice."
        );
    }

    #[test]
    fn in_lane_skips_closed() {
        assert!(project("now", false, None).in_lane("now"));
        assert!(!project("now", false, None).in_lane("next"));
        assert!(!project("now", true, None).in_lane("now"));
        assert!(project("now", true, None).is_closed());
    }

    #[test]
    fn closable_by() {
        let p = project("now", false, Some(7));
        assert!(p.closable_by(true, 3), "an owner or admin");
        assert!(p.closable_by(false, 7), "the project's owner");
        assert!(!p.closable_by(false, 3), "another member");
    }
}
