# Card order and several project owners

Status: Reviewed, building · One PR · Owner: Raj · 26 October 2026

Brief: `docs/briefs/order-and-owners.md`. Mockup:
`docs/designs/order-and-owners.html` (sections §1 to §8 below are the
mockup's). Visuals and copy are the mockup's. This plan covers what the
database, the server, the templates and `frontend/app.ts` must do.

## Review changes

From the security and staff reviews. The sections below are edited to
match; this list is the short version.

- **S1. Who changes owners.** On edit, only managers (`can_manage()`) and
  the project's current owners may change the owner list. For anyone else
  `update` keeps the owners and ignores the posted field. A project with
  no owners: only managers may set them. A super admin acting can't change
  owners (`require_own_membership` rule). On create, the creator picks.
  The form shows the pills disabled, with a line saying who can change
  them, to people who can't.
- **S2. Moves can't hang.** `SET LOCAL lock_timeout = '5s'` and
  `statement_timeout = '10s'` before `pg_advisory_xact_lock`. A timeout
  answers 409 "Someone else is moving cards. Try again." (not 500). A
  renumber is one `UPDATE … FROM unnest(ids, orders)` with
  `organisation_id = $org`.
- **S3. Owner ids.** `ProjectParams::owners()` dedupes and checks them all
  with one `memberships::Model::team_among` query; a count mismatch is the
  field error. `Team::ids` in `src/mcp/mod.rs` does not dedupe (the old
  plan said it did); `owners()` covers both the web and Claude.
- **S4. Notices only to the team.** `task_saved`, `project_saved` and
  `tell_per_person` filter owners through the active team. A removed owner
  gets nothing.
- **Removal.** Removing someone from the organisation deletes their
  `project_owners` rows there (`memberships::Model::remove`).
- **Logging.** `tracing::info!` on owner changes: ids only (who, added,
  removed, acting).
- **Client text.** Names reach the DOM through `textContent`, never
  `|safe`.
- **Checked arithmetic** in `order::slot`; an overflow falls back to a
  renumber.
- **T1.** Every renumber write also filters on the column (`status =
  $status` for tasks; `lane = $lane AND closed_at IS NULL` for projects),
  so a concurrent status change isn't overwritten.
- **T2.** `projects::lock_with_tasks` takes the tasks order lock first,
  then the project row, then its tasks in id order. A close and a move
  can't deadlock; the risk is dropped.
- **T4.** Reorder mode survives the phone roadmap's lane switch: the
  switcher links get `reorder=1` added while the mode is on, and the
  server renders `body[data-reordering]` for it.
- **T5.** Stretched links: inner links are `draggable="false"`; arrow
  buttons sit above the link's `::after`; `a.list-row:hover` also covers
  the new `div.list-row`.
- **T6.** `COPY_OWNERS` ends `ON CONFLICT (project_id, user_id) DO NOTHING`.
- **Also.** The board refreshes once the move queue is empty, and a person
  swimlane board always refreshes then (a task can show twice). The
  `owner_id` source check exempts only the two lines that need it.
  `hashtextextended` is fine (Postgres 11+; local is 15), so no two-int
  fallback. Only a move between two cards with no gap renumbers.

Raj's calls, unchanged: roadmap order = project swimlane order; a drop on
another swimlane's cell is refused; a drop into Blocked still notifies; no
Reorder in the desktop list view; `list_projects` drops `owner` for
`owners`.

## Summary

Two features, one PR.

**Order.** People put task cards in order inside a board column, and project
cards in order inside a roadmap lane. One shared order. Desktop drags, with
an amber drop line between cards. Every screen gets a Reorder mode with up
and down arrows (§3, §4). A drop into another column or lane lands where it
was dropped, which fixes today's bug: the card jumps back after the refresh
because its `sort_order` never changes.

- **Two new routes:** `POST /o/{org}/tasks/{id}/move` and
  `POST /o/{org}/roadmap/projects/{id}/move`. The client sends the target
  status or lane, plus the id of the visible card it lands before (or
  after). Nothing else.
- **One small shared module,** `src/models/order.rs`. A pure function picks
  the new `sort_order`: the midpoint of the gap, or a renumber of that one
  column when there is no gap. Unit-tested without a database.
- **One transaction per move,** under a per-organisation advisory lock, so
  two people reordering at once apply one after the other.
- **Any other status or lane change** (the task sheet, the edit forms,
  Claude) puts the card at the bottom of its new column.

**Owners.** A project has zero or more owners in a new `project_owners`
table. Every owner can close and reopen, and gets the notices the single
owner gets today. The edit form gets the assignee-style people pills (§6).
Cards show up to three avatars, then "+N".

- **One migration:** creates `project_owners`, copies every
  `projects.owner_id` into it. `owner_id` stays. `down` writes each
  project's first owner back.
- **`owner_id` is never read again.** It is still written, as a mirror of
  the first owner, so a code-only revert keeps working (see "The
  migration"). A source test fails on any read outside the model.
- **Claude:** `create_project` takes `owners` (a list) and still takes
  `owner`. `list_projects` and the create reply return `owners`.

No new crate. No live (socket) update: boards refresh only in the tab that
made the change, as today.

## What the code does today

Read before writing this plan:

- **Order.** `tasks.sort_order` and `projects.sort_order` are `bigint`.
  `list_for_org` in `src/models/tasks.rs` (268) and `src/models/projects.rs`
  (262) sort by `sort_order`, then `id`. `create` sets the org's last number
  plus one (`tasks.rs` 309-325, `projects.rs` 297-306). Nothing else writes
  `sort_order`. The numbers are org-wide, not per column: a column is the
  list filtered by status (tasks) or by lane and open (projects).
- **Board.** `board_data` in `src/controllers/tasks.rs` (379) filters by
  scope ("Mine") and search, then builds swimlanes by project, person or
  none. Each cell is one status, in list order. Done cells sort by
  `done_at`, newest first, and keep 5. A task with two assignees shows in
  two person swimlanes. Project swimlanes follow the projects' `sort_order`.
- **Phone list.** `list_data` (294) groups by status (Blocked, In progress,
  To do) for Mine, All and Blocked. Done groups by week. `Within` narrows to
  one project or person.
- **Drag.** `frontend/app.ts` 631-703. `dragover` lights any
  `[data-drop-status]` cell (any swimlane) or `[data-drop-lane]` column.
  `drop` appends the card before `.cell-add` and posts
  `/tasks/{id}/status`. `roadmapDrop` posts `/roadmap/projects/{id}/lane`,
  or opens the close confirm for Done. Both refresh on `tasks-changed` /
  `roadmap-changed`.
- **Status and lane writers.** `tasks::Model::set_status` (429, used by the
  sheet and today's drag), `tasks::Model::update_from` (343, web form and
  Claude's `update_task`), `projects::Model::move_to_lane` (381),
  `projects::Model::update_from` (364). `before_save` on tasks sets
  `done_at` and clears `status_before_close`.
- **Owner.** `projects.owner_id`, nullable, `ON DELETE SET NULL`. Every hit
  of `grep -rn owner_id src assets/views` is in "Owner reads and writes"
  below.
- **Assignees,** the model to copy: `m20261008_134056_task_assignees.rs`
  (table, unique index, copy SQL), `m20261008_134252_remove_owner_id_from_tasks.rs`
  (the "first one back" `down`), `src/models/task_assignees.rs`
  (`by_task`, `for_task`, `replace`), checkboxes named `assignee_ids` in
  `assets/views/tasks/_form.html` 26-37, read with
  `axum_extra::extract::Form` (`tasks.rs` 776, 847). The roadmap's
  `create` and `update` use plain `Form`, which can't read repeated fields.
- **Rights.** `can_close` (`roadmap.rs` 79) is
  `!member.acting && project.closable_by(can_manage, user_id)`. `closable`
  (509) refuses with "Only owners, admins and the project's owner can do
  that." So a super admin acting can never close, owner or not. That
  answers the brief's unverified point.
- **Live updates.** None for boards. `NotifyHub` only refreshes bell
  badges; `ChatHub` is chat only. Other people see a new order on their
  next load or refresh.
- **Memberships** are never hard-deleted (a removal sets `removed`).
  `memberships` has a unique index on `(organisation_id, user_id)`
  (`m20261019_124040_memberships_one_per_org.rs`).

## Owner reads and writes

Every hit, and what it becomes. "Owners" means the project's rows in
`project_owners`, in the order they were added.

| Where | Today | Becomes |
|---|---|---|
| `src/models/projects.rs` 182-222 `ProjectParams.owner_id`, `owner()` | one id as text | `owner_ids: Vec<i64>`, plus legacy `owner_id` (below); `owners()` checks each is on the team |
| `projects.rs` 298, 311 `create` | sets `owner_id` | one transaction: insert, then `project_owners::replace`, mirror |
| `projects.rs` 370-372 `update_from` | sets `owner_id` | one transaction, returns `Saved { project, owners_before, owners }` |
| `projects.rs` 406 `closable_by` | `owner_id == Some(user)` | takes `owners: &[i64]` |
| `projects.rs` 605-611 `apply` | sets `owner_id` | drops the owner line; the mirror is set by `set_owners` |
| `projects.rs` 635-648 unit test helper | builds `owner_id` | unchanged field; `closable_by` tests pass owners |
| `src/controllers/roadmap.rs` 79 `can_close` | one owner | takes the project's owners |
| `roadmap.rs` 117-126 `Card`, 162-178 `DoneCard` | `owner`, `owner_color` | `owners: Vec<Person>` (first 3), `owners_more`, `owners_label` |
| `roadmap.rs` 233-245 `form_values` | `"owner_id"` | `"owner_ids": [..]` |
| `roadmap.rs` 268 `close_data` | `told.extend(owner_id)` | `told.extend(owners)` |
| `roadmap.rs` 398-401 `update` | `owner_before` | from `Saved` |
| `roadmap.rs` 509 `closable` | one owner | loads owners; new refusal copy |
| `src/controllers/notifications.rs` 147 `task_saved` | one owner | every owner not told yet, one `send` |
| `notifications.rs` 245-268 `project_saved` | new owner | each added owner on the team |
| `notifications.rs` 367-370 `tell_per_person` | one owner | every owner on the team not told yet |
| `src/models/memberships.rs` 786 `remove` | (none) | deletes the person's `project_owners` rows |
| `src/mcp/mod.rs` 110-111 `CreateProject.owner` | one name | adds `owners`, keeps `owner` |
| `mcp/mod.rs` 256 `tool_field` | `owner_id => owner` | adds `owner_ids => owners` |
| `mcp/mod.rs` 519, 530, 578 descriptions | "owner" | "owners" |
| `mcp/mod.rs` 693 `list_projects_for` | `"owner"` | `"owners": [names]` |
| `mcp/mod.rs` 712-736 `create_project_for` | `"owner"` | merged list, `"owners"` reply |
| `assets/views/roadmap/_form.html` 10, 23-30 | lead line, `<select name="owner_id">` | new lead line, pills row |
| `assets/views/roadmap/_lane.html` 20, 53 | one avatar in the title row | stack at the end of the pill row |
| `src/models/_entities/projects.rs` 21, 44 | column and relation | unchanged (column stays) |
| Tests | see "Tests" | updated |

No fixtures or seeds mention owners (`src/fixtures/users.yaml` only).
The Home page and the guide checklist don't read the owner.

## The migration

`migration/src/m20261026_120000_project_owners.rs`, registered in
`migration/src/lib.rs` above `// inject-above`.

Generate first, then edit (AGENTS.md rule 1):

```sh
cargo loco generate model project_owners organisation:references project:references user:references
```

That writes the migration, `src/models/_entities/project_owners.rs`,
`src/models/project_owners.rs` and `tests/models/project_owners.rs`, and
wires `mod.rs`. Rename the migration file to the timestamp above if the
generator's sorts before `m20261025_140000`. The generated `create_table`
gives three `NOT NULL` keys with `ON DELETE CASCADE`, like
`task_assignees`. Then add, in `up`, one `execute_unprepared`:

```sql
CREATE UNIQUE INDEX "idx-project_owners-project-user" ON project_owners (project_id, user_id);
CREATE INDEX "idx-project_owners-org-user" ON project_owners (organisation_id, user_id);

-- Org consistency in the database, not only in code: the project and the
-- person must both belong to the row's organisation.
CREATE UNIQUE INDEX "idx-projects-id-org" ON projects (id, organisation_id);
ALTER TABLE project_owners
  ADD CONSTRAINT "fk-project_owners-project-same-org"
  FOREIGN KEY (project_id, organisation_id) REFERENCES projects (id, organisation_id)
  ON DELETE CASCADE ON UPDATE CASCADE,
  ADD CONSTRAINT "fk-project_owners-member-same-org"
  FOREIGN KEY (organisation_id, user_id) REFERENCES memberships (organisation_id, user_id)
  ON DELETE CASCADE ON UPDATE CASCADE;

INSERT INTO project_owners (organisation_id, project_id, user_id, created_at, updated_at)
SELECT p.organisation_id, p.id, p.owner_id, NOW(), NOW()
  FROM projects p
 WHERE p.owner_id IS NOT NULL
   AND EXISTS (SELECT 1 FROM memberships m
                WHERE m.organisation_id = p.organisation_id AND m.user_id = p.owner_id)
ON CONFLICT (project_id, user_id) DO NOTHING;
```

The `EXISTS` keeps the insert from failing on an owner with no membership
in that organisation. The app never allowed one (`owner()` checks
`is_active_member`), so it should skip nothing. Before merging, run this
read-only count against production and expect 0:

```sql
SELECT count(*) FROM projects p
 WHERE owner_id IS NOT NULL
   AND NOT EXISTS (SELECT 1 FROM memberships m
                    WHERE m.organisation_id = p.organisation_id AND m.user_id = p.owner_id);
```

`down`:

```sql
UPDATE projects SET owner_id = (
  SELECT user_id FROM project_owners o WHERE o.project_id = projects.id ORDER BY o.id LIMIT 1
);
```

then `drop_table(m, "project_owners")`, then
`DROP INDEX IF EXISTS "idx-projects-id-org"`. The `UPDATE` sets `owner_id`
to NULL for projects with no owners, which is right: no owners means no
owner.

Keep the copy and the down SQL as `pub const COPY_OWNERS` and
`pub const FIRST_OWNER_BACK` in the migration file, so a test can run them
(see "Tests").

**What it does in production.** On deploy, Railway runs the migration
before the new code serves. It creates an empty table, two indexes on it,
one unique index on `projects (id, organisation_id)` (always unique, since
`id` is), and copies one row per project with an owner. Tens of rows; it
takes milliseconds and holds a lock on `projects` only for the index build.
Nothing on screen changes for an existing project: the same one person
shows and has the same rights.

**Can it roll back?** Yes. `cargo loco db down` once writes each project's
first owner back to `owner_id` and drops the table. Only second and later
owners are lost. Their "made you owner" notices stay in bells.

**Code-only revert** (leave the table): also safe. The new code keeps
`owner_id` equal to the first owner on every save (`set_owners`), so old
code reads a correct single owner. Without that mirror, a project whose
owners changed after the deploy would show its old owner after a revert.

Entities: add `src/models/_entities/project_owners.rs` from the generator.
Do not regenerate `projects.rs` or `tasks.rs` (amendment R1 of
`roadmap-done.md`: regeneration drops the hand-added `#[serde(skip)]`).
`git diff src/models/_entities` shows only the new file and its `mod.rs`
and `prelude.rs` lines.

## What changes, file by file

### 1. `src/models/order.rs` (new)

Pure, no database. Shared by tasks and projects.

```rust
/// Room left between neighbours after a renumber, and the step for "the bottom".
pub const STEP: i64 = 1024;

/// Where a card lands, relative to a card the person can see.
pub enum Place { Before(i64), After(i64), End }

pub enum Slot {
    /// Give the moved card this number. Nothing else changes.
    At(i64),
    /// No room: these `(id, sort_order)` pairs, the moved card included,
    /// are the column's new numbers. Only changed rows are written.
    Renumber(Vec<(i64, i64)>),
}

/// `column` is the target column without the moved card, as `(id, sort_order)`
/// in display order (`sort_order`, then `id`). `None` when the anchor isn't in it.
pub fn slot(column: &[(i64, i64)], moved: i64, place: Place) -> Option<Slot>
```

Rules:

- `Before(a)`: `hi` is `a`'s number, `lo` the card before it (if any).
  `After(a)`: `lo` is `a`'s, `hi` the card after it (if any). `End`: `lo`
  is the last card's (if any), no `hi`.
- Only `lo`: `lo + STEP`. Only `hi`: `hi - STEP` (it can go below zero;
  the column is `bigint`). Neither (empty column): `STEP`.
- Both, and `hi - lo >= 2`: `lo + (hi - lo) / 2`.
- Both, and `hi - lo < 2` (no room, or a tie left by two creates at once):
  `Renumber`, with the moved card inserted at its place and every card at
  `(index + 1) * STEP`.
- All arithmetic is checked (`checked_add`, `checked_sub`). An overflow
  falls back to `Renumber`.

Today's numbers are consecutive, so the first move between two existing
cards renumbers the column. A move to the top or bottom doesn't. After a
renumber most moves write one row.

Also here:

```rust
/// Serializes moves in one organisation's tasks (or projects) for the rest
/// of the transaction. Reads after it see every earlier move committed.
pub async fn lock<C: ConnectionTrait>(txn: &C, what: &str, org_id: i64) -> ModelResult<()>
```

It first runs `SET LOCAL lock_timeout = '5s'` and
`SET LOCAL statement_timeout = '10s'` (as `messages.rs` does for deletes),
then `SELECT pg_advisory_xact_lock(hashtextextended($1, 0))` with
`format!("{what}-order:{org_id}")`, through
`Statement::from_sql_and_values`. A transaction-level advisory lock is
released on commit or rollback. It blocks only other moves (and closes,
see T2) in the same organisation, never ordinary edits.

`order::is_busy(&ModelError)` spots a lock or statement timeout (SQLSTATE
`55P03` or `57014`). The move routes answer it with 409 and the toast
"Someone else is moving cards. Try again.", never a 500.

`create` and `next_sort` stay outside the lock. They only append at the
bottom; a tie between two appends is kept stable by the id tiebreak and
renumbered by the next move between them. Locking every create would make
adding a task wait on someone's drag.

Why not row locks: a move can renumber a whole column. Locking every task
in the org `FOR UPDATE` would block edits. The advisory lock orders moves.
`projects::lock_with_tasks` (delete, close, reopen) takes the same tasks
lock before its project row, and locks the project's tasks `ORDER BY id`,
so a close and a move queue up instead of meeting on rows in a different
order.

### 2. Tasks model (`src/models/tasks.rs`)

**`Model::move_to(db, org_id, id, status, place) -> ModelResult<Result<Moved, Stale>>`.**
`Stale` is a unit struct: the anchor isn't in that column any more.
`Moved { task, before: TaskBefore-like status, changed_status: bool }`.

1. `status` must be `todo`, `progress` or `blocked`. Done keeps its own
   order (brief 5), so `done` is `ModelError::msg("Done keeps its own order.")`.
2. `txn = db.begin()`, `order::lock(&txn, "tasks", org_id)`.
3. The moved task: `find_by_id(id).in_tenant(org_id).lock_exclusive()`.
   Missing: `EntityNotFound` (404).
4. An anchor equal to `id`: `ModelError::msg` (400).
5. The column: `select_only(id, sort_order)`, `in_tenant(org_id)`,
   `status = $status`, `id <> $id`, ordered by `sort_order`, `id`. Every
   task in the org with that status: no scope, search, swimlane or project
   filter. That is the "full list" the brief means.
6. `order::slot(...)`. `None` (anchor not in this org, gone, or now in
   another column): drop `txn`, return `Ok(Err(Stale))`. A foreign id and
   a moved id get the same answer, so the reply says nothing about other
   organisations.
7. `Renumber`: one statement for the changed rows,
   `UPDATE tasks t SET sort_order = v.s FROM unnest($ids, $orders) AS v(id, s)
   WHERE t.id = v.id AND t.organisation_id = $org AND t.status = $status`.
   The status filter means a task whose status changed meanwhile (the
   sheet, which doesn't take the lock) keeps the bottom place it got. It
   skips `before_save`, so `updated_at` doesn't move. A reorder isn't an
   edit, and `updated_at` is the done-time fallback in `Workspace::row`.
8. The moved task: when the status changes, an `ActiveModel` update setting
   `status` and `sort_order`, so `before_save` handles `done_at` and
   `status_before_close` as for any status change. When it doesn't, a
   `sort_order`-only `update_many`.
9. Commit.

**Bottom on any other status change (brief 4).** `set_status` (429) and
`update_from` (343) set `sort_order` to `next_sort(db, org_id)` when the
status really changes. `next_sort` is the org's highest `sort_order` plus
`order::STEP`, the same number `create` now uses (`last + STEP` in place of
`last + 1`). Claude's `update_task` goes through `update_from`, so it
follows. A close's and a reopen's `update_many` don't touch `sort_order`:
reopened tasks go back to their old places.

### 3. Projects model (`src/models/projects.rs`)

**`Model::move_to(db, org_id, id, lane, place) -> ModelResult<Result<Moved, MoveRefusal>>`**,
the same steps as tasks with `order::lock(&txn, "projects", org_id)`:

- `lane` must be in `LANES`. Done is not a lane; a drop on Done still opens
  the close confirm.
- The moved project is locked `FOR UPDATE` in the org. Closed:
  `MoveRefusal::Closed` (the controller answers with `closed_refused`).
- The column: open projects (`closed_at IS NULL`) in that lane, without the
  moved one. Anchor not there: `MoveRefusal::Stale`.
- The project is written with `update_many` (lane and `sort_order`), so
  `updated_at` doesn't move either.
- A renumber is the same one `UPDATE … FROM unnest` statement, filtered on
  `organisation_id`, `lane = $lane` and `closed_at IS NULL`.

`move_to_lane` (381) stays for the old `/lane` route and now sets
`sort_order = next_sort`. `update_from` does the same when the lane
changes. `create` uses `last + STEP`.

The roadmap order is also the board's project swimlane order and the task
form's project picker order, since all three read `list_for_org`. That is
intended: one order for the plan.

**Owners.**

- `ProjectParams`: `#[serde(default)] pub owner_ids: Vec<i64>` and the
  legacy `#[serde(default)] pub owner_id: String`. `owners(db, org_id)`
  returns the deduped list (first one wins): `owner_ids`, or, when that is
  empty and `owner_id` is a number, `[owner_id]`. One
  `memberships::Model::team_among` query checks them all; if it finds fewer
  people than the list has, `field_error("owner_ids", "Choose people from
  the team.")`. The legacy
  field exists for one reason: an edit sheet opened before the deploy posts
  `owner_id`. Without it, saving that sheet would silently clear the owner.
  It goes with the column drop.
- `create`: validate, `owners()`, then one transaction: insert the project,
  `set_owners`, commit. (Today it's one insert with no transaction.)
- `update_from(self, db: &DatabaseConnection, params, rights: OwnerRights) -> ModelResult<Saved>`:
  one transaction. Locks the project row, reads `owners_before`, then
  decides from `rights` whether this person may change owners:
  `OwnerRights::Any` (a manager), `IfOwner(user_id)` (only if in
  `owners_before`; never for a project with no owners), `No` (a super
  admin acting). Allowed: `owners()` checks the posted list, then
  `set_owners`. Not allowed: the posted owners are ignored and not
  checked. `Saved { project, owners_before: Vec<i64>, owners: Vec<i64> }`.
- `set_owners(txn, org_id, project_id, ids)` (private): calls
  `project_owners::Model::replace`, then sets `projects.owner_id` to the
  first of `ids` (or NULL) with `update_many`. That mirror is the only
  remaining write of `owner_id`, and it is never read.
- `closable_by(&self, can_manage: bool, user_id: i64, owners: &[i64]) -> bool`:
  `can_manage || owners.contains(&user_id)`. Its doc comment drops "the
  project's owner" for "its owners".

### 4. `src/models/project_owners.rs` (new, generated then edited)

A copy of `task_assignees.rs`, with `project_id`:

- `by_project(db, org_id) -> HashMap<i64, Vec<i64>>`, ordered by `id`.
- `for_project(db, org_id, project_id) -> Vec<i64>`, ordered by `id`.
- `replace(db, org_id, project_id, user_ids)`: keeps rows for people who
  stay, so the first owner stays first.
- `TenantEntity` impl, and the `updated_at` `before_save`.

### 5. Task routes (`src/controllers/tasks.rs`)

```rust
#[derive(Debug, Deserialize)]
struct MoveForm {
    status: String,
    #[serde(default)]
    before: Option<i64>,
    #[serde(default)]
    after: Option<i64>,
}
```

`POST /tasks/{id}/move`, handler `move_card`, added to `routes()`:

1. `CurrentMember` (approved member; a person waiting for approval never
   gets here). Same rule as `set_status`: any member, and a super admin
   acting may move cards too, since it sends no notice in their name
   unless the status changes, as today's drag.
2. Both `before` and `after`: 400. Neither: `Place::End`.
3. Before the move, `TaskBefore { assignees, status }`, as `set_status`
   does.
4. `tasks::Model::move_to`.
   - `Ok(Ok(moved))` with a changed status: `notifications::task_saved`
     with the `TaskBefore`, so a drop on Blocked still tells the owners, as
     the drag does today. A move inside a column sends nothing (brief 7).
   - `Ok(Err(Stale))`: 409, no `HX-Redirect`, `HX-Reswap: none`, body
     empty. The client toasts and refreshes.
   - `Err(e)` with `order::is_busy(&e)`: 409 with the busy toast in
     `HX-Trigger`. Logged at `warn`.
5. Without `HX-Request`: redirect to `/tasks`, like `set_status`.
6. Success: 200, `HX-Reswap: none`, empty body, no toast.

`set_status` stays for the sheet and for a drop on Done. Its toast is
unchanged.

**Rendering.**

- `Row` gets nothing new; order already comes from `ws.tasks`.
- Board columns: add `"done": key == "done"` to each column so the
  template can show "· Newest first" and skip arrows.
- `list_data`: add `"reorder": filter != "done"` so the phone list knows
  whether to show the Reorder button.

### 6. Roadmap routes (`src/controllers/roadmap.rs`)

**Move.** `POST /roadmap/projects/{id}/move`, handler `move_project`, form
`{ lane, before, after }`. Same steps as tasks. `MoveRefusal::Closed`
returns `closed_refused(&project)` (409 with its toast and a refresh).
`Stale` returns 409 with no body. No toast on success (mockup §1, §2: "No
toast after a drop"). No notices: a lane change never sent any.

`move_lane` stays as it is for old open tabs, now putting the project at
the bottom through `move_to_lane`.

**Owners.**

- `lane_data` loads `project_owners::Model::by_project` once, beside
  `team`. A helper builds the stack:

  ```rust
  struct Owners { shown: Vec<Person>, more: usize, label: String }
  fn owners_of(ids: &[i64], team: &[Person]) -> Owners
  ```

  Only people on the team, in the order added. `shown` is the first 3,
  `more` the rest. `label` is "Owners: Tom, Ana, Leo and Maya" (one name:
  "Owner: Leo"; none: empty). `Card` and `DoneCard` replace `owner` and
  `owner_color` with `owners: Owners`.
- `can_close(member, project, owners)` and `closable` pass the project's
  owners. `closable`'s refusal becomes "Only this project's owners and
  admins can do that."
- `close_data`: `told.extend(owners)`.
- `form_values`: `"owner_ids"` as a list of ids. From params:
  `p.owner_ids`, or the legacy one. From a project: `for_project`. New:
  `[]`.
- `create` and `update` read `axum_extra::extract::Form<ProjectParams>`, so
  repeated `owner_ids` fields parse (plain `Form` drops them).
- `update`: `rights` is `No` when `member.acting`, `Any` when
  `member.can_manage()`, else `IfOwner(member.user.id)`.
  `let saved = project.update_from(&ctx.db, &params, rights).await;`
  then `project_saved(&ctx, &member, &saved.owners_before, &saved.project, &saved.owners)`.
  `saved()` takes the project out of `Saved`. When the owners changed,
  `tracing::info!(org_id, project_id, user_id, acting, added = ?ids, removed = ?ids, "changed project owners")`.
- `create`: `project_saved(&ctx, &member, &[], &project, &owners)`, and the
  same log line when it has owners.
- `form_data` passes `can_change_owners` (the same rule, read from the
  project's owners; always true for a new project). When false the pills
  render `disabled` with "Only this project's owners and admins can change
  owners." under them.

### 7. Notifications (`src/controllers/notifications.rs`)

- **`project_saved(ctx, member, before: &[i64], project, owners: &[i64])`.**
  Sends `kind::OWNER` "made you owner of {name}" to `owners` not in
  `before`, in one `send`. `send` skips the actor, so Raj ticking himself
  gets nothing. People removed get nothing (brief 10).
- **`task_saved`** (147). Loads `project_owners::for_project`. For a new
  task or one just blocked, one `send` to every owner not in `told`.
- **`tell_per_person`** (367). Owners on the team and not already told get
  the total. An owner who also has tasks in it got their "2 of your tasks"
  notice in the loop and is in `told`, so each person gets one notice
  (brief 10). One `send` per distinct body, as now.

`notifications::Model::notify` already dedupes and skips the actor, so a
list with repeats can't double up. Owners are always filtered through
`memberships::Model::team` first in all three, so a removed owner whose
row is still there (a removal before this PR, say) is never told.

**Removal.** `memberships::Model::remove` deletes the person's
`project_owners` rows in that organisation, in its transaction. They stop
owning anything there; a later rejoin starts with no projects.

### 8. Claude (`src/mcp/mod.rs`)

- `CreateProject`: add
  `#[schemars(description = "Usernames of the project's owners. Leave out for none.")] pub owners: Option<Vec<String>>`.
  `owner` stays, described as "One owner's username. Kept for older
  clients; merged with owners."
- `create_project_for`: names = `owners` plus `owner`, blanks dropped, then
  `team.ids(&member, &names)`, which refuses an unknown name with
  `No one called "X" is on the team.` It does not dedupe; `owners()` does.
  `ProjectParams { owner_ids, owner_id: String::new(), .. }`.
  The reply has `"owners": [usernames]`, no `"owner"`.
- `list_projects_for`: `"owners"`, from `by_project`, names on the team
  only. No `"owner"`.
- Descriptions: `list_projects` "...its lane, status, owners, progress...";
  `create_project` "The owners, if any, are notified."; `create_task`
  "Assignees and the project's owners are notified."
- `tool_field`: `"owner_ids" => "owners"`.
- No reorder tool (brief 12). `list_projects` is already in roadmap order.

### 9. Templates

**`assets/views/components/icons.html`.** Three new names, Lucide paths as
in the mockup:

```
arrow-up       <path d="m5 12 7-7 7 7"/><path d="M12 19V5"/>
arrow-down     <path d="M12 5v14"/><path d="m19 12-7 7-7-7"/>
arrow-up-down  <path d="m21 16-4 4-4-4"/><path d="M17 20V4"/><path d="m3 8 4-4 4 4"/><path d="M7 4v16"/>
```

**A shared arrows component,** `assets/views/components/move.html` (or a
block in an existing components file, as the engineer finds the pattern):
two `<button type="button" class="move-btn" data-move="up|down" aria-label="Move {name} up|down">`.
The first card in a cell or group gets `is-none` on its up arrow (with
`tabindex="-1" aria-hidden="true"`), the last on its down arrow, so arrows
line up (§3). The client fixes these after each move.

**Task cards and rows become a wrapper plus a stretched link.** A button
inside an `<a>` is invalid HTML and screen readers read it badly. So in
`tasks/_board.html` the card becomes
`<div class="task-card" draggable="true" data-task-id data-status data-name>`
holding `<a class="task-card-link" href hx-get ...>` (title, with
`::after` covering the card) and the arrows. Same for `tasks/_list.html`:
`<div class="list-row" id="task-{id}" data-task-id data-status data-name>`
with `<a class="list-row-link">`. Clicks, keyboard and the sheet open as
before. Roadmap cards are `<article>` already.

The inner links get `draggable="false"`, so a drag always starts on the
card. The arrows are `position: relative; z-index: 2`, above the link's
`::after`. The `a.list-row:hover` rule in `app.css` also matches
`div.list-row`.

**Lane switch in reorder mode.** `roadmap/index` and `/roadmap/list` read
`reorder=1`. When set, `<body data-reordering="roadmap">` is rendered
(the partial puts it on `#roadmap-lane` as `data-reorder-on`, and the
client copies it). While the mode is on, a click on a phone lane switcher
link adds `reorder=1` to its address, so the next page opens in the mode.

**Board (`tasks/_board.html`, `tasks/_toolbar.html`).**

- Done header: `<span class="col-order">· Newest first</span>` after the
  count.
- Toolbar: the Reorder button (`arrow-up-down`, "Reorder") after the
  search, and the reorder bar (hidden): "Reordering" / "Move tasks with
  the arrows." / Done (§3). Both only in board layout, not in the desktop
  list view.
- Each open cell (`todo`, `progress`, `blocked`): arrows on each card
  (`move-sm`, 32px). Done cells: none.
- In reorder mode the card hides its avatars and note count, and the title
  may wrap to two lines (§8). CSS only.

**Phone list (`tasks/_list.html`, `tasks/index.html`).** The Reorder
button ends the chips row when `reorder` is true (not on Done). Arrows
(44px) on each row of the Blocked, In progress and To do groups. In
reorder mode the row hides the note count and avatars, and the title
wraps.

**Roadmap (`roadmap/_lane.html`, `roadmap/index.html`).**

- Owner stack moves from the title row to the end of the pill row
  (`margin-left: auto`), on open and done cards. Up to three `sm` avatars,
  then `<span class="avatar-more">+{{ more }}</span>` the same size. The
  stack has `role="img"` and `aria-label="{{ owners.label }}"`; the
  avatars inside are `label={false}`.
- Desktop: Reorder in the topbar `actions` block, before "Project".
- Phone: the hint row gets the Reorder button beside the hint (§4). The
  Done lane shows "Newest first" there instead.
- Reorder mode folds open cards to one line (accent dot, name, status) with
  arrows; CSS keyed on the body attribute, from markup already in the card
  (no second card template). Desktop arrows 32px, phone 44px.
- Done column: no arrows, no drop line.

**Project form (`roadmap/_form.html`).**

- Lead line: "A workstream on the roadmap. Owners can mark it done."
- The Lane / Status / Owner row becomes Lane / Status (two columns).
- New row after it: `<fieldset>` "Owners · tap to add or remove", the
  same `person-toggle` checkboxes as the task form, `name="owner_ids"`,
  checked when `person.id in form.owner_ids`. Error under it from
  `errors.owner_ids`.

**Copy changes,** all from the mockup:

1. Board Done header "· Newest first", and the phone roadmap Done lane's
   "Newest first" in place of Reorder. (The roadmap Done hint already says
   "Finished projects, newest first".)
2. Project form lead line, and "Owner" becoming "Owners".
3. The refusal on dropping on Done without the right: "Only this project's
   owners and admins can mark it done." (app.ts `roadmapDrop`, and the
   server's `closable` text above).

### 10. CSS (`frontend/styles/app.css`)

From the mockup's tokens and rules (lines 291-322): `.drop-line` (3px
amber line with a ring at its start), `.drop-note`, `.col-order`,
`.reorder-btn` (with the 44px hit area), `.reorder-bar`, `.move`,
`.move-btn`, `.move-sm`, `.move-btn.is-none { visibility: hidden }`,
`.is-moved` (amber tint, fades), `.avatar-more`. Today's blue
`.is-drop-target` outline becomes amber (§1: blue means in progress).

Reorder mode is `body[data-reordering]`, so it survives the board's and
roadmap's HTMX refreshes (they swap `#task-board`, `#task-list` and
`#roadmap-lane`, never `body`):

- arrows, the reorder bar and folded roadmap cards show;
- the Reorder button, filter chips, search and swimlane switcher hide on
  phones; avatars and note counts in cards hide;
- `.task-card-link`, `.list-row-link` and roadmap pencils don't open
  anything (see app.ts).

### 11. `frontend/app.ts`

Replace the block at 631-703. Keep the file-drop code at 1555+ separate
(it ignores card drags because they carry no `Files`).

**Shared move sender.**

```ts
type MoveKind = "task" | "project";
let moveQueue: Promise<void> = Promise.resolve();
function sendMove(kind: MoveKind, card: HTMLElement, target: string, anchor: Anchor, undo: () => void, refresh: boolean)
```

- Queued: each move waits for the one before, so fast taps reach the
  server in the order they were made. A failure clears the rest of the
  queue (they were computed on a DOM that is about to be refreshed).
- `fetch(orgUrl(url), { method: "POST", headers: { "HX-Request": "true", "Content-Type": "application/x-www-form-urlencoded", ...orgHeaders() }, body, credentials: "same-origin" })`,
  body `status=`/`lane=` plus `before=` or `after=`.
- `staleRefusal(res)` first. Then on success: if the column or lane
  changed, or the board is grouped by person (a task can show twice),
  mark a refresh as due. The refresh event
  (`"tasks-changed" | "roadmap-changed"`) fires once the queue is empty,
  so a refresh never redraws cards a queued move is about to send. A move
  inside a column on other boards doesn't refresh.
- Names in toasts and the live announcement go in through `textContent`
  (`showToast` already does). Nothing here uses `innerHTML` or `|safe`.
- On failure or a network error: `undo()` (put the node back where it
  was), then the error toast, then the refresh event. The toast is the
  `HX-Trigger` header's `toast.message` when there is one (the
  closed-project case), else:
  - board: "Couldn't move {name}. The board is back as it was."
  - phone list: "Couldn't move {name}. The list is back as it was."
  - roadmap: "Couldn't move {name}. The roadmap is back as it was."

**Anchor rule** (both drag and arrows): after placing the card in the
DOM, if a card of the same kind follows it in its cell, send
`before=<that id>`; else if one precedes it, `after=<that id>`; else
neither. Only visible cards are in the DOM, so with Mine, a search, a
person swimlane or a project swimlane, the card lands next to what the
person sees, and hidden cards keep their place (brief, filtered boards).

**Drag (desktop).**

- `dragstart`: as now, and remember the card's cell (or roadmap column),
  its next sibling (for `undo`) and its swimlane `section.lane`.
- `dragover`: the target is a `[data-drop-status]` cell in the same
  `section.lane` as the card (another swimlane is out of scope, brief), or
  a `[data-drop-lane]` column for projects. Over a card, the line goes
  above or below it by which half the pointer is in; over empty space, at
  the end. One `.drop-line` element is moved around, never one per card.
  No outline while the line shows. Over a Done cell: no line, the cell
  gets `is-drop-target` and a `.drop-note` "Lands at the top". Over the
  roadmap Done column: no line and no outline (§5).
- `dragleave` out of the cell and `dragend`: remove the line.
- `drop` on an open cell: if the line sits where the card already is, do
  nothing. Else move the node to the line, update `data-status`, and
  `sendMove`. Drop on a Done cell: today's path (`/tasks/{id}/status`,
  card to the top of the cell). Roadmap drop on Done: today's close
  confirm path, unchanged apart from the toast copy.

**Reorder mode.**

- Reorder button: `document.body.dataset.reordering = "tasks" | "roadmap"`,
  focus the first arrow. Done (in the bar): delete the attribute, focus the
  Reorder button. Leaving the page or following a filter link ends it on
  its own (a new page load). Escape also ends it.
- One capture-phase `click` listener on `document`: while
  `body[data-reordering]`, a click inside a card or row that isn't a
  `[data-move]` button is cancelled (`preventDefault` and
  `stopPropagation`), so nothing opens and HTMX never sees it. A
  `[data-move]` click moves the card:
  - up: insert before the previous sibling card in the same cell or
    group; down: after the next one. Nothing there: do nothing (the arrow
    is hidden anyway).
  - re-mark `is-none` on the first and last arrows of that cell;
  - add `is-moved` for about a second;
  - put focus back on the same arrow, or the other one if this one is now
    hidden (moving the node can drop focus);
  - write "{name}, {n} of {total} in {column}" to one
    `<div id="reorder-status" class="sr-only" aria-live="polite">` in
    `layouts/app.html` (outside every swap);
  - `sendMove` with the anchor rule. No spinner (§3).
- After an HTMX swap (`htmx:afterSwap` on `#task-board`, `#task-list`,
  `#roadmap-lane`) the markup already has its arrows and `is-none` marks
  from the server; nothing to redo.

Build with `bin/build-assets` (Tailwind and esbuild into
`assets/static/`, which is git-ignored). Run it before the request tests
that read rendered pages and before taking screenshots.

## Edge cases

- **A card dropped back where it was:** no request.
- **Two people move cards in the same column at once:** the advisory lock
  makes the second wait, then it reads the first one's numbers and places
  its card against them. Both moves hold. If the second's anchor moved to
  another column meanwhile, it gets 409, a toast and a refresh. A wait of
  more than 5 seconds gives 409 "Someone else is moving cards. Try again."
- **A status change through the sheet during a renumber:** the renumber
  filters on the old status, so it skips that task, which keeps its new
  bottom place.
- **Two creates at once** can still get the same `sort_order` (as today).
  The id tiebreak keeps the order stable, and the next move in that column
  renumbers it (`hi - lo < 2`).
- **Anchor deleted, or in another organisation:** 409, nothing written.
- **Moving a task in a closed project's swimlane** (shown while one of its
  tasks is open again): allowed. It writes only the task.
- **Moving a closed project:** 409 with the closed refusal; can't happen
  from the UI (done cards have no arrows and don't drag).
- **A task in two person swimlanes:** one place in the list. Moving it in
  Ana's swimlane moves it in Tom's too, and in the project view if the
  neighbours are in the same project (brief 3).
- **Reorder mode and a refresh:** the mode lives on `body`, the arrows in
  the markup, so a refresh after a cross-column drop keeps the mode.
- **Owners:** zero is allowed and shows no avatars. Removing a member
  deletes their `project_owners` rows there. Any left over drop out of
  names, notices (team filter) and rights (they can't sign in to the
  organisation). A deleted user's rows cascade.
- **Tom (member, not an owner) edits Website:** saves name, lane and so on;
  his posted owners are ignored, and the pills show disabled.
- **An old edit sheet** posts `owner_id`; the legacy field keeps its owner.
- **A super admin acting:** can reorder; can't close or reopen (unchanged).

## Tests

Before each commit: `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`,
`cargo test`. The Juniper Tea Co helper in `tests/requests/juniper.rs`
(Ana owner, Maya admin, Tom and Leo members) is the team for the new
request tests.

### Unit tests (`src/models/order.rs`)

- `Before` the first card: `hi - STEP`. `After` the last, and `End`:
  `lo + STEP`. Empty column: `STEP`.
- A gap of 2 or more: the midpoint. A gap of 1 and a tie: `Renumber` with
  the card in the right place and numbers `1024, 2048, ...`.
- An anchor not in the column: `None`.
- A long run of moves into the same gap never produces two equal numbers
  (renumbers when it must).

### Unit tests (`src/models/projects.rs`, `src/controllers/roadmap.rs`)

- `closable_by` with no owners, one, several; admin always.
- `owners_of`: 0, 1, 3 and 4 owners (3 shown, "+1"); a removed member is
  skipped; labels "Owner: Leo", "Owners: Ana and Leo",
  "Owners: Tom, Ana, Leo and Maya".

### `tests/models/tasks.rs` and `tests/models/projects.rs`

- `move_to` inside a column writes the new order: reload `list_for_org`
  and check the column's ids.
- First move renumbers; the second move into a fresh gap writes one row
  (count rows whose `sort_order` changed).
- `move_to` doesn't change `updated_at` of any task.
- Cross-column move: the status and place both change; `done_at` and
  `status_before_close` behave as for `set_status`.
- `move_to` with status `done`: refused.
- `set_status`, `update_from` with a new status, and a project's
  `update_from` with a new lane and `move_to_lane`: card at the bottom of
  its new column. The same calls without a change: place kept.
- Concurrency, deterministic, with `tests/locks.rs`: transaction A on its
  own connection takes `order::lock` for the org; spawn a `move_to`; wait
  until `waiting_locks` is above 0 (it is blocked, not slow); move a card
  in A and commit; the spawned move then finishes against A's numbers.
- Busy: A holds the lock past 5 seconds; `move_to` returns a busy error
  (`order::is_busy`), and the route answers 409 with the busy toast.
- A renumber skips a row whose status changed under it (T1).
- A close waits for a move holding the order lock (T2).
- Projects: owners `replace` keeps order; `set_owners` mirrors the first
  owner into `owner_id`, and NULL with none.

### `tests/models/project_owners.rs` (generated, then extended)

- The entity query test the generator writes.
- The database refuses a row whose project is in another organisation, and
  one whose user has no membership in the row's organisation (the two
  composite keys).
- Duplicate `(project_id, user_id)`: refused by the unique index.

### Migration (`tests/models/project_owners.rs`)

Run the migration's own SQL against the test database:

- Make two projects with `owner_id` set (one owner a member, one not) and
  one without. Delete their `project_owners` rows. Run `COPY_OWNERS`. The
  first gets one row, the second and third none.
- Add a second owner to the first. Run `FIRST_OWNER_BACK`. `owner_id` is
  the first-added owner; the project with no owners gets NULL.

And by hand, locally: `cargo loco db down` then `cargo loco db migrate`,
with a project that has two owners, to prove both directions run.

### New `tests/requests/order.rs`

- Ana moves "Order tins" before the first card in To do. 200, no
  `HX-Trigger` toast. `GET /tasks/board` shows it first; Tom's
  `GET /tasks/list?filter=all` shows it first too.
- `after=` the last card: it ends up last.
- Cross-column (the bug): drop "Book the courier" into In progress
  `before=` the second card. Status is `progress`, and after a fresh
  `GET /tasks/board` it sits between the two, not at its old place.
- Cross-column into Blocked tells the project's owners once; a move inside
  To do sends no notice to anyone.
- Filtered board: with Mine on, Leo's visible cards are A and C, with B
  (not his) between them. He moves C `before=A`. Full order: C, A, B.
  Hidden B kept its place relative to A.
- Project swimlane: move a card `before=` a card in the same project with a
  card of another project between them in the full list. The other
  project's card keeps its place relative to the anchor.
- Done: `status=done` is 400. `before` and `after` both set: 400. Anchor
  equal to the card: 400.
- Stale: anchor moved to Blocked first, then a move `before=` it into To
  do: 409, no `HX-Redirect`, nothing changed.
- Projects: Leo moves "Spring blend" `before=` "Gift box redesign" in Next.
  Roadmap, Home's "Roadmap: now" (for a now lane case), the task form's
  project list and Claude's `list_projects` all show the new order.
- Projects cross-lane with `before=`: lane and place both change.
- Closed project move: 409 with the closed refusal toast.
- The old routes: `/tasks/{id}/status` and `/roadmap/projects/{id}/lane`
  put the card at the bottom of its new column.
- A pending member gets the usual refusal on both routes.

### `tests/requests/tenancy.rs`

- Moving another organisation's task or project: 404.
- An anchor from another organisation (a real id there): 409, nothing
  changed in either organisation.
- `owner_ids` naming a user from another organisation: the field error,
  nothing saved (update the existing `owner_id` cases at 61, 191, 261, 359
  to `owner_ids`, and keep one legacy `owner_id` case).

### Owners: `tests/requests/roadmap.rs`, `roadmap_done.rs`, `notifications.rs`

- Raj makes Ana and Leo owners of Website: two rows; Ana and Leo each get
  one "made you owner of Website"; Raj gets none. Saving again with no
  change sends none. Removing Ana sends nothing. Adding Tom tells only Tom.
- An old form posting `owner_id=<leo>`: Leo is the owner (legacy field).
- A form with no owner boxes ticked: no owners.
- S1: Tom (not an owner) posts `owner_ids=<tom>` on Website: saved, owners
  unchanged, and his close is still 403. Leo (an owner) can change them.
  Tom on a project with no owners can't set them; Maya (admin) can. A
  super admin acting can't change them. Tom's sheet shows the pills
  disabled.
- S3: duplicate ids post one row and one notice; a list with one unknown
  id is refused whole.
- S4: a removed owner (row left in place) gets no task, close or owner
  notice. Removing a member deletes their `project_owners` rows.
- Leo (owner, member) closes Website: allowed. Ana gets one close notice,
  even with two of her tasks in it ("2 of your tasks closed"). Tom (not an
  owner) gets 403 with the new text, and his edit sheet has no "Mark
  project done" box. Leo reopens: allowed.
- A new task in Website and one marked blocked: both owners get one notice
  each; the actor none.
- Cards: three avatars and "+1" for four owners, the label text.
- Update existing tests that write `owner_id` directly: `roadmap.rs` 292,
  `roadmap_done.rs` 58 (`owned_by` inserts a `project_owners` row), and
  the assertions at `roadmap.rs` 68 and 183. Update the `ProjectParams`
  literals (add `owner_ids`) in `tests/models/projects.rs` 72,
  `tests/requests/tasks.rs` 23, `mcp_orgs.rs` 1085,
  `dashboard_numbers.rs` 55, `tenancy.rs` 61 and 191,
  `roadmap_done.rs` 1218, and the posted forms in `links.rs` 283,
  `roadmap.rs` 24, `notifications.rs` 405, `mcp_orgs.rs` 466,
  `roadmap_done.rs` 844.

### Claude: `tests/requests/mcp.rs`

- `create_project` with `owners: ["Ana", "Leo"]`: two owners, two
  notices, reply `owners` lists both.
- With `owner: "Bob"` only (the old shape, the test at 344): still works,
  reply has `owners: ["bob"]`.
- With both, overlapping: merged, no duplicate rows or notices.
- An unknown name: refused with `No one called "Zed" is on the team.`
- `list_projects`: `owners` per project in roadmap order; no `owner` key.
- Update the direct `owner_id` writes at 382 and 424 and the assertion at
  356.

### Source check (in `tests/requests/order.rs`)

Like the lane check in `roadmap_done.rs` 303: no `owner_id` read
(`\.owner_id\b` or `Column::OwnerId`) in `src/` outside
`src/models/_entities/`, except two known lines in
`src/models/projects.rs`: the mirror write (`Column::OwnerId` in
`set_owners`) and the legacy param (`self.owner_id` in `owners()`). Each
is matched by its exact text, so a new read in that file fails too.

Also: the phone roadmap with `reorder=1` renders `data-reordering`, and its
lane switcher links are there to carry it (T4).

### Not covered by `cargo test`

No JS runner. The drop line, the reorder bar, arrows, focus, the live
announcement, rollback on failure and the queue are hand checks (below).

## Commits, in order

One PR (Raj prefers fewer, bigger PRs). Each commit builds and passes tests.

1. Migration, `project_owners` entity and model, migration and model tests.
2. Owners everywhere: params, create and update in transactions, the
   mirror, `closable_by`, roadmap cards and form, notices, Claude, tests,
   the `owner_id` source check.
3. `order.rs` with unit tests; `move_to` on tasks and projects; bottom on
   other status and lane changes; model tests.
4. The two move routes, request and tenancy tests.
5. Templates: icons, stretched links, arrows, Reorder button and bar,
   "Newest first", owner stacks. CSS. `app.ts`: drop line, drag to a spot,
   reorder mode, queue, rollback.
6. Copy changes and screenshots.

## Out of scope

From the brief: touch drag; a personal order; sorting by due date or
priority; reordering done cards, swimlanes or columns; dragging to another
swimlane; owner roles; notices on removal; reorder or owner edits through
Claude beyond `create_project`; an owner filter. Also: the desktop list
view (`view=list`) shows the new order but gets no Reorder button (the
mockup doesn't draw it). Dropping `projects.owner_id` is a later PR.

## Risks

- **A missed `owner_id` reader** silently drops a right or a notice. The
  table above lists every hit, and the source check fails on a new one.
- **Stretched-link markup change** touches every task card and phone row.
  View tests that match `<a ... class="task-card"` must be updated, and
  the sheet must still open from a click, Enter and a notification's
  `?open=`. Checked by hand on both screens.
- **Advisory lock key** is a 64-bit hash of a string. A collision only
  makes two organisations' moves wait for each other. Harmless.
- **Migration foreign keys.** If production has an owner with no
  membership in that organisation, the `EXISTS` skips that copy and the
  project loses its owner. The pre-merge count query says whether any
  exist (expected 0).
- **Old open tabs after the deploy** run old JS for a while: their drag
  posts `/status` and `/lane`, which still work (card at the bottom), and
  their edit sheet posts `owner_id`, which the legacy field keeps.
- **`hashtextextended`** is in Postgres 11+. Local is 15; Railway's
  Postgres image is newer than 11. No fallback.
- **Unverified:** the 32px arrows fit a 265px desktop card at 1440px with
  the sidebar (the mockup's §8 says yes); checked in the screenshots.

## Rollback

Revert the PR. The revert alone is safe with the table left in place:
`owner_id` holds each project's first owner. Then, if wanted, roll back the
migration (`cargo loco db down` once). See "The migration" for what each
step loses. Orders people made stay; old code reads the same
`sort_order`.

## Checks

```sh
bin/build-assets
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

Also `cargo loco db down` then `cargo loco db migrate` locally.

By hand, locally, with realistic data, not test leftovers. Make Juniper Tea
Co as in the mockup: Ana (owner), Maya (admin), Tom and Leo (members), Raj
(owner). Projects Website (Ana, Leo), Wholesale catalogue (Tom, Ana, Leo,
Maya), Instagram shop (Maya) in Now; Tea tasting evenings (Leo), Gift box
redesign (Maya, Ana), Spring blend (Leo, Maya) in Next; Ship to Europe (no
owner), Subscription box (Leo) in Later; two done. Website's To do with
"Order tins", "Book the courier" and three more.

- Ana drags "Order tins" to the top of To do: the amber line, the faded
  card, the drop, no toast. Reload: still first. Tom on a phone: first.
- Maya drags a card into In progress between two cards; after the refresh
  it is still there.
- With Mine on and with a search, drag and arrows place the card next to
  the visible ones.
- A drop on Done: the whole cell lights, "Lands at the top".
- Stop the server and move a card: it goes back, the error toast shows,
  the column refreshes once the server is back.
- Phone 390px, board list: Reorder, arrows, top row has no up arrow, tap up
  on "Order tins", the tint, VoiceOver reads "Order tins, 2 of 7 in To
  do", Done ends it. The Done filter has no Reorder.
- Phone roadmap: Leo taps Reorder in Next, folded cards, moves "Spring
  blend" up, switches to Now still in the mode, taps Done. Home's
  "Roadmap: now" shows the same order.
- Raj makes Ana and Leo owners of Website: one notice each, none for Raj.
  Leo marks Website done; Tom has no "Mark project done" and gets the new
  toast when dropping Website on Done.
- Claude (local connector): "who owns Website?" names Ana and Leo.

### Screenshots needed

From the local app with the data above, on the `pr-screenshots` branch
under `order-and-owners/`. Desktop 1440px and phone 390px; dark mode too,
since colours (amber line, tints) and layout (folded cards, owner stacks)
change.

- Desktop board: dragging within To do with the drop line (§1), and into
  In progress between two cards. Light and dark.
- Desktop board dragging over Done, with "Lands at the top" and the "·
  Newest first" header (§5).
- Desktop board in reorder mode, 32px arrows, a just-moved card (§3).
- Desktop roadmap: dragging "Spring blend" in Next with the line, owner
  stacks including "+1" (§2). Light and dark.
- Desktop roadmap in reorder mode with folded cards (§4).
- Phone task list: before, reordering with a moved row, and the failed
  save toast (§3). Light and dark.
- Phone roadmap Next: before and reordering (§4); the Done lane with
  "Newest first" (§5).
- Edit project sheet with the Owners pills, desktop and phone (§6), and
  Tom's sheet without the close box (§7).
- Cards with no owner, one, two and four (§6).
- Ana's bell with "Raj made you owner of Website".
- Claude's `list_projects` output with `owners`, as a screenshot of the
  tool result.

## Decisions

Each recommendation below was taken as the decision (Raj asked for fewer
questions). Any can be revisited at PR time.

1. **The roadmap order is also the board's project swimlane order,**
   because both read the same `sort_order`. Recommend: yes, one plan order.
   The alternative needs a second column.
2. **A drop on another swimlane's cell is refused** (no line, nothing
   happens). Today it changes the status and the card snaps back to its own
   swimlane. Recommend: refuse; moving between swimlanes is out of scope,
   and half-doing it reads as a bug.
3. **A drop into Blocked still sends the "marked blocked" notice** to the
   owners, as today's drag does. Only moves inside a column are silent.
   Recommend: keep; it is a status change, not a reorder.
4. **The desktop list view gets no Reorder button** in this version; it
   shows the order. Recommend: leave it until someone asks.
5. **Claude's `list_projects` and create reply drop `owner`** for
   `owners`, as the brief says. Recommend: drop it; Claude reads the
   description.

## Where this differs from the brief or mockup

- **`owner_id` is still written** (the first owner), though never read, so
  a code-only revert is correct. The brief says it "stays in place,
  unread"; this keeps that and adds the mirror.
- **The form accepts the old `owner_id` field** until the column drop, for
  sheets opened before the deploy.
- **Roadmap drops show no toast** (mockup §1, §2). Today's "moved to Next."
  toast goes from the drag; the old `/lane` route keeps it.
- **Task cards and phone rows become a `div` with a stretched link**, so
  the arrow buttons aren't inside a link. Looks the same.
- **New items get the last number plus 1024**, not plus 1, so a later move
  usually has room.
- **The close refusal on the server** also changes to "Only this project's
  owners and admins can do that.", to match the mockup's toast.
- **Decisions 1 to 4 above** are not in the brief; each is a consequence
  of the shared order or of today's behaviour.
