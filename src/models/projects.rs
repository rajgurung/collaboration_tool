use std::{collections::HashMap, sync::LazyLock};

use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};

pub use super::_entities::projects::{ActiveModel, Column, Entity, Model};
use super::{
    field_error, memberships,
    order::{self, Place, Slot},
    project_owners, task_assignees,
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

/// Whose open tasks a close marks done. The roadmap handler builds it once
/// per request from the closer's own membership, never from a form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseScope {
    /// An organisation owner or admin: every open task.
    Everything,
    /// A project owner who is not an owner or admin of the organisation
    /// (this user): only open tasks with no assignee but them. A task with
    /// anyone else on it, a removed member included, stays open.
    OwnWork(i64),
}

impl CloseScope {
    /// Whether a close marks an open task with these assignees done.
    #[must_use]
    pub fn closes(self, assignees: &[i64]) -> bool {
        match self {
            Self::Everything => true,
            Self::OwnWork(closer) => assignees.iter().all(|id| *id == closer),
        }
    }
}

/// What closing a project would do, worked out from its tasks for the
/// confirm step: how many of each status it would mark done, how many are
/// done already, and how many it leaves open (`kept`).
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
pub struct CloseSummary {
    pub todo: usize,
    pub progress: usize,
    pub blocked: usize,
    pub done: usize,
    pub kept: usize,
}

impl CloseSummary {
    /// Every task by status, as a manager's close sees them. For the Done
    /// card's counts.
    #[must_use]
    pub fn of(project_id: i64, all_tasks: &[tasks::Model]) -> Self {
        Self::for_close(
            project_id,
            all_tasks,
            &HashMap::new(),
            CloseScope::Everything,
        )
    }

    /// What a close in `scope` would do. `assignees` maps task ids to their
    /// assignees. [`Model::close`] applies the same rule.
    #[must_use]
    pub fn for_close(
        project_id: i64,
        all_tasks: &[tasks::Model],
        assignees: &HashMap<i64, Vec<i64>>,
        scope: CloseScope,
    ) -> Self {
        let mut summary = Self::default();
        for t in all_tasks
            .iter()
            .filter(|t| t.project_id == Some(project_id))
        {
            let on_it = assignees.get(&t.id).map_or(&[][..], Vec::as_slice);
            match t.status.as_str() {
                "done" => summary.done += 1,
                _ if !scope.closes(on_it) => summary.kept += 1,
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
        self.open() + self.done + self.kept
    }

    /// The confirm step's sentence: "4 open tasks will be marked done. 1 is
    /// blocked." `who` names the people on the tasks it leaves open, as
    /// "Ana and Maya"; unused when there are none.
    #[must_use]
    pub fn sentence(&self, who: &str) -> String {
        let open = self.open();
        if self.kept > 0 {
            let kept = self.kept_clause(who);
            return if open == 0 {
                format!("No tasks are marked done. {kept}")
            } else {
                format!("{} {kept}", self.marked_done(open))
            };
        }
        match (open, self.total()) {
            (0, 0) => "It has no tasks.".to_string(),
            (0, 1) => "Its 1 task is already done.".to_string(),
            (0, total) => format!("All {total} tasks are already done."),
            _ => self.marked_done(open),
        }
    }

    /// "4 open tasks will be marked done. 1 is blocked."
    fn marked_done(&self, open: usize) -> String {
        let lead = format!("{open} open {} will be marked done.", tasks_word(open));
        match (open, self.blocked) {
            (_, 0) => lead,
            (1, _) => format!("{lead} It is blocked."),
            (_, 1) => format!("{lead} 1 is blocked."),
            (_, n) => format!("{lead} {n} are blocked."),
        }
    }

    /// "3 assigned to Ana and Maya stay open."
    fn kept_clause(&self, who: &str) -> String {
        let stay = if self.kept == 1 { "stays" } else { "stay" };
        format!("{} assigned to {who} {stay} open.", self.kept)
    }

    /// The line in the "Mark project done" box, before anyone taps it.
    #[must_use]
    pub fn box_line(&self) -> String {
        if self.kept > 0 {
            let kept = self.kept_clause("others");
            return match self.open() {
                0 => format!("No tasks are marked done. {kept}"),
                1 => format!("1 open task is marked done with it. {kept}"),
                n => format!("{n} open tasks are marked done with it. {kept}"),
            };
        }
        match self.open() {
            0 => self.sentence(""),
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

/// People as a sentence names them: "Ana", "Ana and Maya", "Ana, Maya, Leo
/// and Sam", or "Ana, Maya and 3 others" for five or more. `unnamed` people
/// (off the team) add "and others", or count in the others; with no names
/// at all it is "others".
#[must_use]
pub fn names(named: &[String], unnamed: usize) -> String {
    match named {
        [] => "others".to_string(),
        [first, second, rest @ ..] if rest.len() >= 3 => {
            format!("{first}, {second} and {} others", rest.len() + unnamed)
        }
        _ if unnamed > 0 => format!("{} and others", named.join(", ")),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// Who a close would notify, as the confirm step says it: "Maya and Tom get
/// a notice.", or "Maya, Tom and 3 others get a notice." for five or more.
#[must_use]
pub fn notice_line(people: &[String]) -> String {
    match people {
        [] => "No one else gets a notice.".to_string(),
        [one] => format!("{one} gets a notice."),
        _ => format!("{} get a notice.", names(people, 0)),
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
/// the notices count what really changed. `left_open` are the open tasks
/// its [`CloseScope`] left alone. `already` means it was closed before, and
/// nothing changed.
#[derive(Debug)]
pub struct Closed {
    pub project: Model,
    pub tasks: Vec<tasks::Model>,
    pub left_open: Vec<tasks::Model>,
    pub already: bool,
}

/// Why a project move changed nothing.
#[derive(Debug)]
pub enum MoveRefusal {
    /// The card it was placed against isn't in that lane (any more, or
    /// ever, as for another organisation's project).
    Stale,
    /// It was marked done meanwhile; reopen it first.
    Closed(Box<Model>),
}

/// The number that puts a project at the bottom of any lane. Not under the
/// order lock, as for tasks ([`tasks::next_sort`]).
///
/// # Errors
/// On database errors.
pub async fn next_sort<C: ConnectionTrait>(db: &C, org_id: i64) -> ModelResult<i64> {
    let last = Entity::find()
        .in_tenant(org_id)
        .order_by_desc(Column::SortOrder)
        .one(db)
        .await?
        .map_or(0, |p| p.sort_order);
    Ok(last.saturating_add(order::STEP))
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

/// The most owners a project can have, so one post can't make a huge
/// query.
pub const MAX_OWNERS: u64 = 50;

/// The project form. `owner_ids` comes from repeated checkbox fields.
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
    #[validate(length(max = MAX_OWNERS, message = "Choose up to 50 owners."))]
    pub owner_ids: Vec<i64>,
    #[serde(default)]
    #[validate(length(max = 500, message = "Keep the summary under 500 characters."))]
    pub summary: String,
}

impl ProjectParams {
    /// The chosen owners, without repeats, in the order given. Each must be
    /// an approved member of the organisation; one that isn't refuses the
    /// whole list. Call it in the transaction that saves them: it reads the
    /// memberships `FOR SHARE`, so a removal committing now is waited for and
    /// seen, and one starting later waits, then takes them off.
    ///
    /// # Errors
    /// An `owner_ids` field error, or database errors.
    pub async fn owners<C: ConnectionTrait>(&self, db: &C, org_id: i64) -> ModelResult<Vec<i64>> {
        let refused = || field_error("owner_ids", "Choose people from the team.");
        let mut ids: Vec<i64> = Vec::new();
        for &id in &self.owner_ids {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        if ids.is_empty() {
            return Ok(ids);
        }
        let found = memberships::Entity::find()
            .in_tenant(org_id)
            .filter(memberships::Column::Status.eq(memberships::status::ACTIVE))
            .filter(memberships::Column::UserId.is_in(ids.clone()))
            .lock_shared()
            .all(db)
            .await?;
        if found.len() == ids.len() {
            Ok(ids)
        } else {
            Err(refused())
        }
    }
}

/// Who may change a project's owners in an edit. Anyone can edit a project;
/// owners decide who can close it, so only some people can change them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerRights {
    /// An organisation owner or admin, through their own membership.
    Any,
    /// A member: only while they own the project.
    IfOwner(i64),
    /// A super admin acting in an organisation that isn't theirs.
    No,
}

impl OwnerRights {
    #[must_use]
    pub fn allows(self, owners: &[i64]) -> bool {
        match self {
            Self::Any => true,
            Self::IfOwner(user_id) => owners.contains(&user_id),
            Self::No => false,
        }
    }
}

/// What an edit saved, with the owners before and after, for the notices.
#[derive(Debug)]
pub struct Saved {
    pub project: Model,
    pub owners_before: Vec<i64>,
    pub owners: Vec<i64>,
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
    pub async fn create(
        db: &DatabaseConnection,
        org_id: i64,
        params: &ProjectParams,
        claude_grant: Option<Uuid>,
    ) -> ModelResult<Self> {
        ValidatorTrait::validate(params)?;
        let txn = db.begin().await?;
        let owners = params.owners(&txn, org_id).await?;
        let mut project = ActiveModel {
            sort_order: ActiveValue::Set(next_sort(&txn, org_id).await?),
            claude_grant_id: ActiveValue::Set(claude_grant),
            ..Default::default()
        }
        .set_tenant(org_id)?;
        apply(&mut project, params);
        let project = project.insert(&txn).await?;
        project_owners::Model::replace(&txn, org_id, project.id, &owners).await?;
        txn.commit().await?;
        Ok(project)
    }

    /// Deletes a project with its tasks, their notes, assignees and
    /// reactions (cascading keys), and the notifications that link to those
    /// tasks. For owners and admins in the web app. Can't be undone.
    ///
    /// # Errors
    /// `EntityNotFound` when it is not in this organisation, or database
    /// errors, including a lock timeout ([`order::is_busy`]).
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
    /// errors, including a lock timeout ([`order::is_busy`]).
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

    /// Saves an edit and, when `rights` allow it, the owners, together. The
    /// project row is locked first, so the owners read here are the ones
    /// the rule is checked against. Without the right, the posted owners
    /// are ignored, not checked. Likewise the posted lane, unless
    /// `may_move` (managers only: planning lanes is theirs).
    ///
    /// # Errors
    /// Validation errors, `EntityNotFound` when it is gone, or database
    /// errors.
    pub async fn update_from(
        self,
        db: &DatabaseConnection,
        params: &ProjectParams,
        rights: OwnerRights,
        may_move: bool,
    ) -> ModelResult<Saved> {
        ValidatorTrait::validate(params)?;
        let org_id = self.organisation_id;
        let txn = db.begin().await?;
        let current = Entity::find_by_id(self.id)
            .in_tenant(org_id)
            .lock_exclusive()
            .one(&txn)
            .await?
            .ok_or(ModelError::EntityNotFound)?;
        let owners_before = project_owners::Model::for_project(&txn, org_id, current.id).await?;
        let lane = if may_move {
            params.lane.clone()
        } else {
            current.lane.clone()
        };
        // A new lane puts it at the bottom of that lane.
        let bottom = if current.lane == lane {
            None
        } else {
            Some(next_sort(&txn, org_id).await?)
        };
        let mut project = current.into_active_model();
        apply(&mut project, params);
        project.lane = ActiveValue::Set(lane);
        if let Some(n) = bottom {
            project.sort_order = ActiveValue::Set(n);
        }
        let project = project.update(&txn).await?;
        let owners = if rights.allows(&owners_before) {
            let posted = params.owners(&txn, org_id).await?;
            project_owners::Model::replace(&txn, org_id, project.id, &posted).await?;
            project_owners::Model::for_project(&txn, org_id, project.id).await?
        } else {
            owners_before.clone()
        };
        txn.commit().await?;
        Ok(Saved {
            project,
            owners_before,
            owners,
        })
    }

    /// Moves the project to the bottom of another lane, from a drag in a
    /// tab drawn before drops could land on a spot. The caller refuses a
    /// closed project first.
    ///
    /// # Errors
    /// A message when `lane` is not now, next or later, or database errors.
    pub async fn move_to_lane<C: ConnectionTrait>(self, db: &C, lane: &str) -> ModelResult<Self> {
        if !LANES.contains(&lane) {
            return Err(ModelError::msg("Choose now, next or later."));
        }
        let bottom = next_sort(db, self.organisation_id).await?;
        let mut project = self.into_active_model();
        project.lane = ActiveValue::Set(lane.to_string());
        project.sort_order = ActiveValue::Set(bottom);
        Ok(project.update(db).await?)
    }

    /// Puts an open project at `place` in `lane`, from a drag or the Reorder
    /// arrows. The lane is every open project in it. Moves in one
    /// organisation run one at a time ([`order::lock`]). Neither a move nor a
    /// renumber is an edit, so `updated_at` stays.
    ///
    /// # Errors
    /// A message for an unknown lane, or an anchor that is the card itself;
    /// `EntityNotFound` when it isn't in this organisation; database errors,
    /// including a lock timeout ([`order::is_busy`]).
    pub async fn move_to(
        db: &DatabaseConnection,
        org_id: i64,
        id: i64,
        lane: &str,
        place: Place,
    ) -> ModelResult<Result<Self, MoveRefusal>> {
        if !LANES.contains(&lane) {
            return Err(ModelError::msg("Choose now, next or later."));
        }
        if matches!(place, Place::Before(a) | Place::After(a) if a == id) {
            return Err(ModelError::msg("Choose another card to move it next to."));
        }
        let txn = db.begin().await?;
        order::lock(&txn, "projects", org_id).await?;
        let project = Entity::find_by_id(id)
            .in_tenant(org_id)
            .lock_exclusive()
            .one(&txn)
            .await?
            .ok_or(ModelError::EntityNotFound)?;
        // An early return drops `txn`, which rolls it back.
        if project.is_closed() {
            return Ok(Err(MoveRefusal::Closed(Box::new(project))));
        }
        let column: Vec<(i64, i64)> = Entity::find()
            .select_only()
            .column(Column::Id)
            .column(Column::SortOrder)
            .in_tenant(org_id)
            .filter(Column::Lane.eq(lane))
            .filter(Column::ClosedAt.is_null())
            .filter(Column::Id.ne(id))
            .order_by_asc(Column::SortOrder)
            .order_by_asc(Column::Id)
            .into_tuple()
            .all(&txn)
            .await?;
        let Some(slot) = order::slot(&column, id, place) else {
            return Ok(Err(MoveRefusal::Stale));
        };
        let number = match slot {
            Slot::At(n) => n,
            Slot::Renumber(rows) => {
                let (mine, others): (Vec<_>, Vec<_>) =
                    rows.into_iter().partition(|(r, _)| *r == id);
                order::write_renumber(&txn, org_id, &others, order::Cards::Lane(lane)).await?;
                mine.first().map_or(order::STEP, |(_, n)| *n)
            }
        };
        Entity::update_many()
            .col_expr(Column::Lane, Expr::value(lane))
            .col_expr(Column::SortOrder, Expr::value(number))
            .filter(Column::Id.eq(id))
            .filter(Column::OrganisationId.eq(org_id))
            .exec(&txn)
            .await?;
        txn.commit().await?;
        Ok(Ok(Model {
            lane: lane.to_string(),
            sort_order: number,
            ..project
        }))
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

    /// Who can close and reopen it: organisation owners, admins and its
    /// owners (`owners`, from `project_owners`). A close changes tasks and
    /// notifies people, so it is a step above editing. An owner who isn't a
    /// manager closes only their own work ([`CloseScope::OwnWork`]).
    #[must_use]
    pub fn closable_by(can_manage: bool, user_id: i64, owners: &[i64]) -> bool {
        can_manage || owners.contains(&user_id)
    }

    /// Marks the project done, and the tasks in it that `scope` covers
    /// ([`CloseScope::closes`]) and aren't done yet, in one transaction. Each
    /// such task remembers its status in `status_before_close` for a reopen.
    /// Tasks already done, and open tasks the scope leaves (`left_open`), are
    /// not touched. The project closes either way. Closing a closed project
    /// changes nothing.
    ///
    /// The assignees are read after the tasks are locked. A task edit locks
    /// its row before changing assignees, so they can't change between the
    /// check and the write.
    ///
    /// The tasks are written in one statement, past `before_save`, so it sets
    /// `done_at` itself. Postgres reads the old row on the right of `SET`, so
    /// `status_before_close` gets the status from before.
    ///
    /// `None` when `user_id` can't close it ([`Self::closable_by`]; only
    /// [`CloseScope::Everything`] counts as a manager), checked against the
    /// owners as they are once the project is locked, so an owner taken off
    /// meanwhile can't.
    ///
    /// # Errors
    /// `EntityNotFound` when it is not in this organisation, or database
    /// errors, including a lock timeout ([`order::is_busy`]).
    pub async fn close(
        db: &DatabaseConnection,
        org_id: i64,
        id: i64,
        user_id: i64,
        scope: CloseScope,
    ) -> ModelResult<Option<Closed>> {
        let txn = db.begin().await?;
        let (project, locked) = lock_with_tasks(&txn, org_id, id).await?;
        // An early return drops `txn`, which rolls it back.
        let owners = project_owners::Model::for_project(&txn, org_id, id).await?;
        if !Self::closable_by(scope == CloseScope::Everything, user_id, &owners) {
            return Ok(None);
        }
        if project.is_closed() {
            return Ok(Some(Closed {
                project,
                tasks: Vec::new(),
                left_open: Vec::new(),
                already: true,
            }));
        }
        let ids: Vec<i64> = locked.iter().map(|(id, _)| *id).collect();
        let assignees = task_assignees::Model::for_tasks(&txn, org_id, &ids).await?;
        let (closing, kept): (Vec<i64>, Vec<i64>) = ids
            .into_iter()
            .partition(|id| scope.closes(assignees.get(id).map_or(&[][..], Vec::as_slice)));
        let now: DateTimeWithTimeZone = chrono::Utc::now().into();
        // Always limited to `closing`: never every open task by default.
        let closed = if closing.is_empty() {
            Vec::new()
        } else {
            tasks::Entity::update_many()
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
                .filter(tasks::Column::Id.is_in(closing))
                .exec_with_returning(&txn)
                .await?
        };
        let left_open = if kept.is_empty() {
            Vec::new()
        } else {
            tasks::Entity::find()
                .filter(tasks::Column::OrganisationId.eq(org_id))
                .filter(tasks::Column::Id.is_in(kept))
                .filter(tasks::Column::Status.ne("done"))
                .order_by_asc(tasks::Column::Id)
                .all(&txn)
                .await?
        };
        let mut active = project.into_active_model();
        active.closed_at = ActiveValue::Set(Some(now));
        active.closed_by_id = ActiveValue::Set(Some(user_id));
        let project = active.update(&txn).await?;
        txn.commit().await?;
        Ok(Some(Closed {
            project,
            tasks: closed,
            left_open,
            already: false,
        }))
    }

    /// Puts a closed project back in its lane. Tasks the close marked done go
    /// back to their earlier status, unless someone changed them since (that
    /// cleared the saved status). Tasks done before the close stay done.
    /// Reopening an open project changes nothing.
    ///
    /// `None` when `user_id` can't reopen it, as for [`Self::close`].
    ///
    /// # Errors
    /// `EntityNotFound` when it is not in this organisation, or database
    /// errors, including a lock timeout ([`order::is_busy`]).
    pub async fn reopen(
        db: &DatabaseConnection,
        org_id: i64,
        id: i64,
        user_id: i64,
        can_manage: bool,
    ) -> ModelResult<Option<Reopened>> {
        let txn = db.begin().await?;
        let (project, _) = lock_with_tasks(&txn, org_id, id).await?;
        let owners = project_owners::Model::for_project(&txn, org_id, id).await?;
        if !Self::closable_by(can_manage, user_id, &owners) {
            return Ok(None);
        }
        if !project.is_closed() {
            return Ok(Some(Reopened {
                project,
                tasks: Vec::new(),
                already: true,
            }));
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
        Ok(Some(Reopened {
            project,
            tasks: restored,
            already: false,
        }))
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
///
/// It takes the tasks' order lock first, and the tasks in id order, so a
/// card move (which renumbers a whole status column) and a close queue up
/// instead of meeting on the same rows in a different order.
async fn lock_with_tasks<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: i64,
) -> ModelResult<(Model, Vec<(i64, Option<Uuid>)>)> {
    order::lock_then_unlimit(db, "tasks", org_id).await?;
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
        .order_by_asc(tasks::Column::Id)
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

fn apply(project: &mut ActiveModel, params: &ProjectParams) {
    project.name = ActiveValue::Set(params.name.trim().to_string());
    project.lane = ActiveValue::Set(params.lane.clone());
    project.status = ActiveValue::Set(params.status.trim().to_string());
    project.accent = ActiveValue::Set(params.accent.to_lowercase());
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
            kept: 0,
        }
    }

    fn kept(todo: usize, blocked: usize, kept: usize) -> CloseSummary {
        CloseSummary {
            todo,
            blocked,
            kept,
            ..CloseSummary::default()
        }
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|n| (*n).to_string()).collect()
    }

    fn project(lane: &str, closed: bool) -> Model {
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
            assert_eq!(s.sentence("Ana"), sentence, "no kept tasks, no names");
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

        let names = strings;
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
    fn a_member_closes_only_their_own_and_unassigned_tasks() {
        let me = CloseScope::OwnWork(7);
        assert!(me.closes(&[]), "unassigned");
        assert!(me.closes(&[7]), "only mine");
        assert!(!me.closes(&[3]), "someone else's");
        assert!(!me.closes(&[7, 3]), "shared");
        assert!(!me.closes(&[3, 7]), "shared, me second");
        assert!(CloseScope::Everything.closes(&[]));
        assert!(CloseScope::Everything.closes(&[3, 7]));
    }

    #[test]
    fn close_copy_with_tasks_left_open() {
        assert_eq!(
            kept(1, 0, 3).sentence("Ana and Maya"),
            "1 open task will be marked done. 3 assigned to Ana and Maya stay open."
        );
        assert_eq!(
            kept(1, 0, 3).box_line(),
            "1 open task is marked done with it. 3 assigned to others stay open."
        );
        assert_eq!(
            kept(0, 0, 3).sentence("Ana and Maya"),
            "No tasks are marked done. 3 assigned to Ana and Maya stay open."
        );
        assert_eq!(
            kept(0, 0, 3).box_line(),
            "No tasks are marked done. 3 assigned to others stay open."
        );
        assert_eq!(
            kept(1, 1, 1).sentence("Ana"),
            "2 open tasks will be marked done. 1 is blocked. 1 assigned to Ana stays open."
        );
        assert_eq!(
            kept(0, 1, 2).sentence("others"),
            "1 open task will be marked done. It is blocked. 2 assigned to others stay open."
        );
        assert_eq!(
            kept(2, 0, 1).box_line(),
            "2 open tasks are marked done with it. 1 assigned to others stays open."
        );
    }

    #[test]
    fn names_in_a_sentence() {
        assert_eq!(names(&[], 0), "others");
        assert_eq!(names(&[], 2), "others");
        assert_eq!(names(&strings(&["Ana"]), 0), "Ana");
        assert_eq!(names(&strings(&["Ana", "Maya"]), 0), "Ana and Maya");
        assert_eq!(
            names(&strings(&["Ana", "Maya", "Leo", "Sam"]), 0),
            "Ana, Maya, Leo and Sam"
        );
        assert_eq!(
            names(&strings(&["Ana", "Maya", "Leo", "Sam", "Tom"]), 0),
            "Ana, Maya and 3 others"
        );
        assert_eq!(names(&strings(&["Ana"]), 1), "Ana and others");
        assert_eq!(names(&strings(&["Ana", "Maya"]), 2), "Ana, Maya and others");
        assert_eq!(
            names(&strings(&["Ana", "Maya", "Leo", "Sam", "Tom"]), 1),
            "Ana, Maya and 4 others"
        );
    }

    #[test]
    fn in_lane_skips_closed() {
        assert!(project("now", false).in_lane("now"));
        assert!(!project("now", false).in_lane("next"));
        assert!(!project("now", true).in_lane("now"));
        assert!(project("now", true).is_closed());
    }

    #[test]
    fn closable_by() {
        assert!(Model::closable_by(true, 3, &[]), "an admin, no owners");
        assert!(Model::closable_by(true, 3, &[7]), "an admin");
        assert!(Model::closable_by(false, 7, &[7]), "the one owner");
        assert!(Model::closable_by(false, 8, &[7, 8, 9]), "one of several");
        assert!(!Model::closable_by(false, 3, &[7, 8]), "another member");
        assert!(!Model::closable_by(false, 3, &[]), "no owners");
    }

    #[test]
    fn owner_rights() {
        assert!(OwnerRights::Any.allows(&[]));
        assert!(OwnerRights::IfOwner(7).allows(&[3, 7]));
        assert!(!OwnerRights::IfOwner(7).allows(&[3]));
        assert!(
            !OwnerRights::IfOwner(7).allows(&[]),
            "no owners: managers only"
        );
        assert!(!OwnerRights::No.allows(&[7]));
    }
}
