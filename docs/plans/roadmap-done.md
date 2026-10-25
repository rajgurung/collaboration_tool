# Roadmap: done projects and a progress donut

Status: Draft for staff review · One PR · Owner: Raj · 25 October 2026

Brief: `docs/briefs/roadmap-done.md` (approved). Mockup:
`docs/designs/roadmap-done.html` (approved). Section numbers below (§1 to §8)
are the mockup's. Visuals and copy are the mockup's. This plan covers what the
server, the templates and `frontend/app.ts` must do.

## Summary

A project can be marked done. Done is a closed state, not a lane. The project
keeps its lane and gets `closed_at` and `closed_by_id`. Closing marks every
open task done in one transaction and records each task's earlier status in a
new `tasks.status_before_close`. Reopening puts the project back in its lane
and puts back the tasks that nobody changed since.

What people see:

- The roadmap gets a fourth column, Done (§1). Phones get a fourth segment in
  the lane switcher (§2).
- "Mark project done" lives in the project's edit sheet, with a confirm step
  that shows the counts (§3). Done cards have Reopen instead of the pencil (§4).
- Home's "Roadmap: now" gets one donut of the tasks in open "now" projects,
  drawn on the server as SVG (§5).
- Each affected person gets one bell notice of a new kind, `closed` (§6).
- Claude's `list_projects` shows `closed` and `closed_at`. Claude can't add or
  move tasks into a closed project. Claude gets no close tool.

One migration: two nullable columns on `projects`, one on `tasks`. It rolls
back cleanly. No new crate. No chart library.

**The key safety rule.** Every place that reads a project's lane must also skip
closed projects. The plan adds one helper, `projects::Model::in_lane(lane)`,
which checks both. Every lane comparison goes through it, and a source test
fails on any other `.lane ==` outside `src/models/projects.rs`.

## What the code does today

Read before writing this plan:

- `src/models/projects.rs`: `LANES = ["now", "next", "later"]`, `LANE_RE`
  validates the form's lane, `Progress::of` counts done tasks, and
  `lock_with_tasks` locks a project, then its tasks, `FOR UPDATE`. Its comment
  relies on a task insert waiting on the project row through its foreign key
  check.
- `src/controllers/roadmap.rs`: `lane_data` builds three lanes by
  `p.lane == *key`. `destroy` needs `require_manager` and
  `require_own_membership`. Routes are under `/roadmap/projects/{id}`.
- `assets/views/roadmap/_lane.html`, `_form.html`, `index.html`: cards with a
  pencil that opens `#project-dialog`; the delete confirm uses `hx-confirm`.
- `src/controllers/dashboard.rs`: `roadmap_now` filters `p.lane == "now"`.
- `src/controllers/tasks.rs`: `task_form_data` builds `project_groups` from
  `LANES`. `board_data` makes one swimlane per project with tasks, and each
  cell has an add button with `project_id` set. Drag and drop only changes
  status.
- `src/models/tasks.rs`: `TaskParams::check` only checks the project is in the
  org. `before_save` sets and clears `done_at`. `create`, `update_from` and
  `set_status` are the only task writers (no `update_many` on tasks anywhere).
- `src/controllers/notifications.rs`: `send` saves and publishes;
  `task_saved` sends `assigned` and `project` notices per task. Bodies are
  "headline\nsecond line".
- `src/models/notifications.rs`: `kind` constants. `notify` skips the actor
  and duplicates.
- `src/mcp/mod.rs`: `INSTRUCTIONS` says "Projects sit in roadmap lanes: now,
  next or later." `list_projects_for` returns id, name, lane, status, summary,
  owner, progress. `create_task_for` and `update_task_for` build `TaskParams`
  and call the same model methods as the web, so a model-level refusal covers
  both. `From<ModelError> for Fail` turns a field error into
  `Refused("project_id: ...")`.

## What changes, file by file

### 1. Migration

`migration/src/m20261025_120000_project_closed.rs` (new), registered in
`migration/src/lib.rs`. Raw SQL, like `m20261013_145508_add_parent_id_to_messages.rs`,
because `add_reference` makes a non-null column named after the table.

```sql
ALTER TABLE projects
  ADD COLUMN closed_at timestamptz NULL,
  ADD COLUMN closed_by_id bigint NULL
    REFERENCES users (id) ON DELETE SET NULL ON UPDATE CASCADE;
ALTER TABLE tasks
  ADD COLUMN status_before_close varchar NULL;
```

`closed_by_id` uses the same type as `projects.owner_id` (the engineer checks
with `\d projects` first). No index: a team has tens of projects, and every
query already filters by `organisation_id`.

`down` drops the three columns.

The doc comment says what a rollback means: closed projects show again in
their old lanes, their tasks stay done, and who closed them and the saved
statuses are lost.

Then regenerate `src/models/_entities/projects.rs` and `tasks.rs` with
`cargo loco db entities`.

### 2. Projects model (`src/models/projects.rs`)

- `pub const DONE: &str = "done";` for the roadmap's fourth column. `LANES`
  and `LANE_RE` stay as they are, so no form or Claude call can set a "done"
  lane (brief decision 1).
- `pub const DONE_SHOWN: usize = 5;` for the Done column.
- `Model::is_closed(&self) -> bool`.
- `Model::in_lane(&self, lane: &str) -> bool`: `!self.is_closed() && self.lane == lane`.
  Every lane check in the app uses this.
- `Model::closable_by(&self, can_manage: bool, user_id: i64) -> bool`:
  `can_manage || self.owner_id == Some(user_id)`. Used for close, reopen, and
  to show the box and the Reopen button.
- `pub fn closed_refusal(name: &str) -> String`:
  `"{name} is done. Reopen the project first."` One string for the web form,
  the project edit refusal and Claude.

**`CloseSummary`**, a plain struct worked out from a slice of tasks, like
`Progress::of`:

```rust
pub struct CloseSummary { pub todo: usize, pub progress: usize, pub blocked: usize, pub done: usize }
```

With `open()`, `total()`, `sentence()` and `box_line()`. `sentence()` returns
the mockup's §3 table word for word:

| Case | `sentence()` |
|---|---|
| 4 open, 1 blocked | 4 open tasks will be marked done. 1 is blocked. |
| 3 open, 2 blocked | 3 open tasks will be marked done. 2 are blocked. |
| 1 open, blocked | 1 open task will be marked done. It is blocked. |
| 2 open, none blocked | 2 open tasks will be marked done. |
| all 9 done | All 9 tasks are already done. |
| no tasks | It has no tasks. |

`box_line()`: "Its 4 open tasks are marked done with it." ("Its 1 open task
is marked done with it."). With nothing open it returns `sentence()`.

**`Model::close(db, org_id, id, user_id) -> ModelResult<Closed>`**, one
transaction:

1. `lock_with_tasks(&txn, org_id, id)`. This is the existing helper. It finds
   the project in the org (else `EntityNotFound`) and locks it and its tasks
   `FOR UPDATE`.
2. Already closed: commit nothing, return `Closed { already: true, .. }`. A
   double tap is a no-op, not an error.
3. One statement marks the open tasks done and remembers their status:

   ```sql
   UPDATE tasks
      SET status_before_close = status, status = 'done',
          done_at = now(), updated_at = now()
    WHERE organisation_id = $1 AND project_id = $2 AND status <> 'done'
   RETURNING *;
   ```

   Written with `tasks::Entity::update_many()` and `col_expr`, run with
   `exec_with_returning`. In Postgres the right side of `SET` reads the old
   row, so `status_before_close` gets the status from before. This bypasses
   `before_save` on purpose, so it sets `done_at` itself.
4. Set `closed_at = now()` and `closed_by_id = user_id` on the project.
5. Commit. Return `Closed { project, tasks: <the returned rows>, already: false }`.

The toast and the notices use `tasks.len()`, the number this statement really
changed. If someone finished a task while the confirm was open, the toast
says 3, not 4 (mockup §8, question 4).

**`Model::reopen(db, org_id, id) -> ModelResult<Reopened>`**, one transaction:

1. `lock_with_tasks`. Not closed: return `Reopened { already: true, .. }`.
2. Put back the tasks the close marked done that nobody changed since:

   ```sql
   UPDATE tasks
      SET status = status_before_close, status_before_close = NULL,
          done_at = NULL, updated_at = now()
    WHERE organisation_id = $1 AND project_id = $2
      AND status = 'done' AND status_before_close IS NOT NULL
   RETURNING id;
   ```

3. Clear any other `status_before_close` left on this project's tasks (there
   should be none, see section 3; this keeps a reopen from ever leaving stale
   values).
4. Set `closed_at` and `closed_by_id` to NULL. The lane is untouched, so the
   project is back where it was.
5. Commit. Return `Reopened { project, restored: <count>, already: false }`.

Tasks that were done before the close have `status_before_close` NULL and
keep their own `done_at`. They stay done.

**`lock_open_for_tasks(txn, org_id, id) -> ModelResult<Model>`** (new, `pub(crate)`).
Reads the project `FOR SHARE` (`lock_shared()`) inside the task's
transaction. Closed: `field_error("project_id", &closed_refusal(&name))`.
Not in the org: `field_error("project_id", "Choose a project.")`, as today.

Why a lock and not a plain read: a plain read races with a close. The task
form checks "open", the close commits, then the insert lands in a closed
project. `FOR SHARE` waits for a close holding `FOR UPDATE`, and then reads
the committed row, so it sees `closed_at`. The other way round, a close waits
for the task's transaction, and its task lock then sees the new task and
closes it too. Lock order is project, then task, in both paths, the same as
`lock_with_tasks`, so the two can't deadlock.

**`update_from` for projects** refuses a closed project. The refusal is not
about one field, so it is not a field error. The controller checks
`is_closed()` before calling it and answers 409 (section 4). A stale edit
sheet can't change a closed project's lane or name.

**Done column query.** `Model::closed_for_org(db, org_id)`: closed projects,
`closed_at DESC, id DESC`. The roadmap takes the first `DONE_SHOWN` unless
asked for all.

### 3. Tasks model (`src/models/tasks.rs`)

- `TaskParams::check` keeps checking the project is in the org. It does not
  check closed, because that check must hold a lock.
- `create`: when `checked.project_id` is `Some`, call
  `projects::lock_open_for_tasks(&txn, org_id, id)` first thing in the
  transaction, before the sort-order read and the insert.
- `update_from`: when the project changes (`checked.project_id !=
  self.project_id`) and the new one is `Some`, the same call, first thing in
  the transaction. A task that stays in its closed project can still be edited
  and have its status changed (brief: "Someone can still set one of its tasks
  back to in progress").
- **Forgetting the saved status.** "Changed since" means the status moved off
  done, or the task moved to another project:
  - `before_save`: when `status` is set to anything but `"done"`, also set
    `status_before_close` to NULL. This covers `update_from`, `set_status`
    and Claude's `update_task`, which all save through it.
  - `update_from`: when the project changes, set `status_before_close` to
    NULL. Otherwise a task moved out of a closed project carries a stale
    value into another project, and that project's reopen would wrongly
    "restore" it.

  A title or due-date edit, or saving the form with status still done, keeps
  the saved status, so the task still goes back on reopen. Setting it to in
  progress and then back to done clears it, so it stays done.

### 4. Roadmap controller and templates

**Routes** (`src/controllers/roadmap.rs`):

```
POST /roadmap/projects/{id}/close
POST /roadmap/projects/{id}/reopen
```

Both handlers, in order:

1. `CurrentMember` (an approved member; pending people never get here).
2. `member.require_own_membership()?` (decision 2 below: a super admin acting
   can't close or reopen, like delete).
3. `find_in_org` (404 for another org's project), then
   `project.closable_by(member.can_manage(), member.user.id)` or 403 "Only
   owners, admins and the project's owner can do that."
4. `projects::Model::close` or `reopen`.
5. Close only: `notifications::project_closed(...)` (section 6), not when
   `already`.
6. `tracing::info!` with user, org, project, task count and `acting`, as
   `destroy` does.
7. Not HTMX: redirect to `/roadmap?lane=done` (close) or `/roadmap?lane={lane}`
   (reopen). HTMX: `HX-Trigger` with the toast, `roadmap-changed` and
   `close-dialogs`, plus `HX-Reswap: none`.

Toasts:

| Case | Toast |
|---|---|
| Close, 4 tasks changed | Website done. 4 tasks closed. |
| Close, 1 | Website done. 1 task closed. |
| Close, 0 | Website done. |
| Close, already closed | Website is already done. |
| Reopen, 3 restored | Website is back in Now. 3 tasks reopened. |
| Reopen, 1 | Website is back in Now. 1 task reopened. |
| Reopen, 0 | Website is back in Now. |
| Reopen, already open | Website is already open. |

The lane word is `Now`, `Next` or `Later`.

**`update` and `edit` on a closed project** answer 409 with an error toast
(`closed_refusal`) and `HX-Reswap: none`. There is no pencil on a done card, so
only a stale sheet gets here. `destroy` still works on a closed project.

**`lane_data`** takes the member (for Reopen) and a `show_all: bool`:

- `LaneQuery` gains `all: Option<String>`. `lane_or_default` also accepts
  `"done"`.
- The three lanes use `p.in_lane(key)`.
- A fourth entry, `key: "done"`, label "Done", hint "Finished projects, newest
  first", `count` = all closed projects, `cards` = the newest `DONE_SHOWN`
  unless `show_all`, `more` = count > shown, `show_all`.
- Done cards add: `closed_on` ("25 Oct" this year, "12 Dec 2025" for earlier
  years, in the org's zone via `time::local`), `closed_by` (name, or none if
  the user is gone), `done_count`, `open_count` (tasks not done, for the blue
  "1 open task" pill), `can_reopen` (`closable_by` and not acting), `lane_label`.

**`form_data`** (edit only, open project) adds:

- `summary`: `CloseSummary::of(id, &tasks)` with `sentence`, `box_line`, and
  per-status counts for the pills.
- `notified`: the names who would get a notice, from the open tasks'
  assignees plus the project owner, minus the viewer, kept to the team. The
  line reads "Maya and Tom get a notice.", "Maya gets a notice.",
  "Maya, Tom and 3 others get a notice." (5 or more), or "No one else gets a
  notice."
- `can_close`: `closable_by` and not acting.

**`roadmap/_lane.html`:**

- Switcher: `{{ l.label }}<span class="seg-count">{{ l.count }}</span>` for
  all four (§2).
- `hx-get` on `#roadmap-lane` carries `&all=1` when `show_all`, so a refresh
  keeps the long list.
- Done cards (§1, §4): no pencil, no progress bar, no status pill. The badge
  "Done 25 Oct" with the green tick. The footer has "By Ana" (`title` and
  screen-reader text "Marked done by Ana") and, when `can_reopen`, a
  `.reopen` text button:
  `hx-post=".../reopen" hx-disabled-elt="this" data-reopen data-name="Website"`.
  "12 tasks done" is plain text. The blue "1 open task" pill shows when
  `open_count > 0`. With no tasks, "No tasks".
- Under the 5th card: "Show all 14" links to `/roadmap?lane=done&all=1`
  (HTMX swaps `#roadmap-lane`); then "Show newest 5".
- Empty Done: "No projects here yet.", as the other lanes.

**`roadmap/_form.html`** (edit, open project, `can_close`): the "Mark project
done" box above the buttons (§3), with `box_line`. Tapping it hides the form
and shows a confirm step. The confirm step sits outside the `<form>`, as a
sibling in the sheet body, so its post sends no form fields. It has the
sentence, the status pills, the notice line, an error alert (hidden), Back,
and "Mark done", the amber primary:
`hx-post=".../close" hx-disabled-elt="find button" data-close-confirm`.

**CSS** (`frontend/styles/app.css`): `.roadmap` grid to 4 columns at the
desktop breakpoint. Done card, badge, `.reopen`, `.seg-count` and its under-360px
rule, the confirm step, and the spinner states ("Marking done…",
"Reopening…") use `.htmx-request`. Values come from the mockup's CSS.

**`frontend/app.ts`:**

- `[data-close-open]` hides the form step and shows the confirm step; Back
  does the reverse. Nothing typed is lost, because the form is only hidden.
- `htmx:responseError` and `htmx:sendError` on `[data-close-confirm]` show
  the inline alert: "Couldn't mark Website done. Nothing was changed. Try
  again." (403 shows the server's text instead.) Same pattern as
  `[data-org-new-form]`.
- The same events on `[data-reopen]` show the error toast "Couldn't reopen
  Website. Try again."

### 5. The task picker and refusals (`src/controllers/tasks.rs`)

- `task_form_data` gains `current_project: Option<i64>`. The groups use
  `p.in_lane(lane)`. If the task's current project is closed, a last group
  "Done" holds only that project, so editing a task in a closed project keeps
  it there. Without this the select would fall back to "No project" and
  quietly turn the task into a chore.
- `new` with `?project_id=` naming a closed project (an old board add button)
  clears it, so the picker starts blank.
- The refusal itself is in the model (section 3), so the web form gets the
  error under Project through the existing `field_errors` path, with a 422.

### 6. The board's project swimlanes

`board_data`, group "project":

- Open projects first, as today.
- A closed project gets a swimlane only when it has a task in view that is
  not done (someone set one back). That swimlane comes after the open ones,
  shows a "Done" badge, and has no add buttons. Its "All done tasks" link
  stays.
- A closed project with every task done gets no swimlane. Its tasks are still
  in the Done view and under Person and None.

This is decision 3 below.

### 7. Notifications

`src/models/notifications.rs`: `kind::CLOSED = "closed"`.

`src/controllers/notifications.rs`: `pub async fn project_closed(ctx, member,
project, closed: &[tasks::Model])`. It does not call `task_saved`.

1. Counts closed tasks per assignee, from `task_assignees` for those task
   ids, kept to the org's team (`memberships::Model::team`).
2. Each assignee gets one notice: "marked Website done\n2 of your tasks closed"
   ("1 of your tasks closed" for one, as the mockup).
3. The project owner, if on the team and not already told as an assignee:
   "marked Website done\n4 tasks closed" ("1 task closed"; with 0 tasks, only
   "marked Website done").
4. Link `/roadmap?lane=done`. `send` skips the actor and publishes to the hub.

Reopen sends the same kind of notice, one per person, with kind `reopened` (added after launch, PR "reopen notice").

`assets/views/notifications/index.html`: `closed` uses the `status-done` icon.
`app.css`: `.notice-closed { color: var(--done); }`.

### 8. Home donut (§5)

`src/views/donut.rs` (new, `pub mod donut` in `src/views/mod.rs`). Pure maths,
no database:

```rust
pub struct Donut { pub total: usize, pub done: usize, pub pct: usize,
                   pub arcs: Vec<Arc>, pub legend: Vec<Slice>, pub label: String }
pub fn of(done: usize, progress: usize, blocked: usize, todo: usize) -> Option<Donut>
```

- `None` when the total is 0. Home then shows no donut.
- `pct = done * 100 / total`, rounded down, like `Progress`. So "100%" only
  when every task is done. (The mockup rounds; 199 of 200 would then read
  100%.)
- Order clockwise from 12: done, in progress, blocked, to do. `r = 50`,
  `C = 2πr`, stroke 14, gap 2.
- Each status with count > 0 is one arc. `len = count / total * C`. With two
  or more arcs, `dash = max(len - 2, 0.5)`. With one, `dash = C` and no gap.
  `offset = -(sum of earlier len)`. Values are strings with 2 decimals.
- `legend` always has all four rows, with zeros.
- `label`: "Now: 62% done. 31 done, 9 in progress, 3 blocked, 7 to do."

`dashboard.rs`:

- `roadmap_now` uses `p.in_lane("now")`.
- The donut counts tasks whose project is one of those open "now" projects.
  Chores and closed projects are left out.
- Adds `donut` and `now_projects` (count) to the page.

`assets/views/dashboard/_donut.html` (new), included at the top of "Roadmap:
now" when `donut` is set. An SVG 120×120 with `role="img"` and
`aria-label="{{ donut.label }}"`. One `<circle>` per arc with
`class="donut-arc donut-{{ key }}"`, `stroke-dasharray`, `stroke-dashoffset`
and `transform="rotate(-90 60 60)"`. The centre "62%" and "done". Beside it,
"31 of 50 tasks done", "In 3 projects in Now" ("In 1 project in Now"), and
the legend list (`.zero` on rows with 0).

Colours come from classes in `app.css`
(`.donut-done { stroke: var(--done) }` and so on), not from a `stroke="var(...)"`
attribute. The mockup used the attribute; a class makes sure the tokens
switch with dark mode.

No "now" projects: today's empty card, unchanged. "Now" projects with no
tasks: no donut, cards as today.

### 9. Claude connector (`src/mcp/mod.rs`)

- `list_projects_for`: each project adds `"closed": p.is_closed()` and
  `"closed_at"` (org-local RFC 3339, or null). Lane stays as stored.
- `list_projects` description: "List every project with its lane, status,
  owner, progress (done/total tasks), and whether it is closed (marked done)."
- `INSTRUCTIONS` gains one sentence after the lanes sentence: "A project can
  be marked done in the web app; list_projects shows it as closed, and a
  closed project takes no new tasks."
- `create_task` and `update_task` need no change. The model refuses and `Fail`
  returns "project_id: Website is done. Reopen the project first."
- No close or reopen tool.

## Edge cases

- **A closed project's task set back to in progress.** Allowed. The project
  stays closed. Its card shows "1 open task". Its board swimlane appears
  (section 6). That task's saved status is cleared, so a reopen leaves it
  alone.
- **A task moved out of a closed project.** Allowed (only moves in are
  refused). Its saved status is cleared.
- **The owner closes their own project.** No notice to themselves.
- **The owner or an assignee has left the org.** Not on the team, so no
  notice. `closed_by_id` goes NULL if the user is deleted; the card then
  shows no "By" line.
- **Close with no open tasks.** Closes. Toast "Website done." Owner notice
  without a count.
- **Two people close at once.** The second waits on the lock, then sees
  `already` and gets "Website is already done."
- **Delete a closed project.** Only after a reopen in the web app (no edit
  sheet on done cards). The server route still accepts it. Claude's
  `delete_project` keeps its own rules.
- **`done_at` on bulk-closed tasks** is the close time. The Done view puts
  them all in this week.
- **The donut can drop when you close** (mockup §8). Closing takes the
  project's done tasks out of the count. Expected.
- **Free-text status "Done".** Hidden on done cards, so it never sits beside
  the badge. Untouched by close and reopen.

## Tests

Run against Postgres, `#[serial]`, with the existing helpers
(`sign_up_member`, `TestMember::at`, `as_member`, `super_admin_cookie`,
`project_in`, `tool` from `tests/requests/mcp.rs`). New file
`tests/requests/roadmap_done.rs`, registered in `tests/requests/mod.rs`.

### Unit tests (in the source files)

`src/views/donut.rs`:

- `typical_is_62_percent`: 31/9/3/7 gives pct 62, four arcs, offsets 0,
  -194.78, ... (each the running sum of `len`), each dash `len - 2`.
- `no_tasks_is_no_donut`: 0/0/0/0 gives `None`.
- `all_done_is_one_full_ring`: 19/0/0/0 gives pct 100, one arc, dash = C, no
  gap, legend with three zeros.
- `rounds_down`: 199/1/0/0 gives 99, not 100. 1/2/0/0 gives 33.
- `a_single_task_still_shows`: 1 blocked among 500 gives a dash of 0.5.
- `legend_keeps_its_shape`: always four rows, in order.
- `label_reads_well`: the screen-reader sentence, word for word.

`src/models/projects.rs`:

- `close_sentence_matches_the_mockup`: the six rows of the §3 table.
- `box_line_and_notice_line`: one, many, five-or-more names, none.
- `in_lane_skips_closed`.
- `closable_by`: manager yes, owner yes, other member no.

### Permissions per role (`roadmap_done.rs`)

- `owners_admins_and_the_project_owner_can_close` (three roles, each on a
  fresh project).
- `a_member_who_does_not_own_it_cannot_close_or_reopen`: Tom owns another
  project; 403 on both, nothing changed.
- `the_box_and_reopen_show_only_to_those_allowed`: render checks of the edit
  sheet and the done card for owner, admin, project owner and member.
- `a_super_admin_acting_cannot_close_or_reopen`: 403 "Leave admin mode",
  nothing changed (decision 2).
- `pending_members_are_sent_away`: the existing `CurrentMember` redirect.

### Transaction (`tests/models/projects.rs`)

- `close_marks_every_open_task_done`: todo, progress and blocked become done
  with `done_at` set and `status_before_close` recorded. A task done last week
  keeps its `done_at` and a NULL saved status. `closed_at` and
  `closed_by_id` are set.
- `close_twice_is_a_no_op`: second call returns `already`, changes nothing.
- `close_of_another_orgs_project_changes_nothing`: `EntityNotFound`, every
  row unchanged.
- `a_task_added_during_a_close_is_closed_too`: transaction A takes
  `lock_open_for_tasks` and inserts a task; a close started on another
  connection waits; after A commits, the close includes the new task. The
  reverse order: the close commits first and the insert is refused. If this
  is flaky on CI, it becomes a hand check and the plan says so in the PR.
- `the_toast_reports_the_real_count`: open the edit sheet (4 open), finish one
  task, close: toast "Website done. 3 tasks closed."

### Status restore on reopen (`tests/models/projects.rs` and `roadmap_done.rs`)

- `reopen_puts_each_task_back`: blocked, progress, todo come back as they were,
  `done_at` cleared. The task done before the close stays done with its old
  `done_at`. The project is back in its lane, `closed_at` NULL.
- `a_task_changed_since_is_left_alone`: after the close, one task set to
  progress stays progress; another set to progress then done stays done.
- `a_title_edit_does_not_count_as_a_change`: edit the title, keep done;
  reopen restores it.
- `a_task_moved_out_keeps_no_saved_status`: moved to project B after A
  closed; closing and reopening B never touches its status from A.
- `reopen_toast_counts_restored_tasks`: "Website is back in Now. 3 tasks
  reopened."

### Notification count

- `maya_with_two_tasks_gets_one_notice`: exactly one `closed` notice, body
  "marked Website done\n2 of your tasks closed", link `/roadmap?lane=done`.
- `the_owner_gets_the_total`: "4 tasks closed". An owner who is also an
  assignee gets only the assignee notice.
- `the_closer_gets_nothing_and_no_task_notices_are_sent`: no `assigned` or
  `project` notices from the close.
- `removed_members_get_nothing`.
- `reopen_sends_nothing`.
- `the_bell_shows_the_closed_icon`: the list renders `notice-closed`.

### Lane filters

- `a_closed_project_leaves_now_and_joins_done`: roadmap page, desktop columns
  and the phone switcher counts ("Done 1").
- `done_shows_the_newest_five_then_all`: 14 closed; 5 cards and "Show all 14";
  `all=1` shows 14 and "Show newest 5"; newest first.
- `home_now_skips_closed_projects`.
- `the_task_picker_skips_closed_projects`: not in the new-task form. In the
  edit form of a task already in it, it stays selected under "Done".
- `new_task_from_an_old_add_button_starts_blank`.
- `the_board_hides_a_fully_done_closed_project`: and shows its swimlane, with
  no add buttons, once a task is set back.
- `mcp_list_projects_shows_closed`: `closed: true` and `closed_at`, open ones
  `false` and null.
- `no_lane_compare_outside_projects` (source test, like
  `only_the_resolver_reads_path_org`): greps `src/` for `.lane ==` and
  `lane == "` outside `src/models/projects.rs` and fails on a hit.

### Task refusal, web and Claude

- `web_create_into_a_closed_project_is_refused`: 422, the form shows "Website
  is done. Reopen the project first." under Project, no task saved.
- `web_move_into_a_closed_project_is_refused`: 422, task unchanged.
- `web_status_change_inside_a_closed_project_is_allowed`: 200; card shows "1
  open task"; project still closed.
- `editing_a_closed_project_is_refused`: `POST /roadmap/projects/{id}` gives
  409 and nothing changes.
- `mcp_create_task_into_a_closed_project_is_refused`: tool error text
  contains the refusal; no task.
- `mcp_update_task_into_a_closed_project_is_refused`.
- `mcp_update_task_status_inside_a_closed_project_is_allowed`.

### Donut on Home

- `home_shows_the_donut_for_now_projects`: 31/9/3/7 across three projects,
  plus a chore and a closed project's tasks that must not count. The
  `aria-label` reads "Now: 62% done. 31 done, 9 in progress, 3 blocked, 7 to
  do."
- `no_donut_when_now_has_no_tasks`, and the old empty card when "now" has no
  projects.
- `all_done_shows_100`.

### Tenancy (`tests/requests/tenancy.rs`, plus `roadmap_done.rs`)

- `org_a_cannot_close_or_reopen_org_b`: 404 for both, Globex's project and
  tasks untouched. Added to `org_a_cannot_reach_org_b`'s route list too.
- `owning_a_project_in_a_does_not_let_you_close_bs`: same person in both orgs,
  owner of a project in A, member in B: 403 in B.
- `notices_stay_in_the_org`: every `closed` notice has the project's
  `organisation_id`, and only that org's members get one.
- `mcp_shows_only_this_orgs_closed_projects`.
- The acting test above.

`tests/requests/stale_tab.rs`: the two new POST routes join the walk, so the
claim check covers them.

**Not covered by `cargo test`.** No JS runner. The confirm step swap, Back,
the spinners, the inline alert and the reopen error toast are hand checks.

## The migration

- **What it does.** Adds `projects.closed_at` and `projects.closed_by_id`
  (nullable, key to users, set to NULL if the user is deleted), and
  `tasks.status_before_close` (nullable text). Every existing row gets NULL,
  so every project stays open and nothing on screen changes until someone
  closes one. No backfill. Instant on a table this size.
- **Can it roll back?** Yes. `down` drops the three columns. Closed projects
  show again in their old lanes, at 100%. Their tasks stay done. Who closed
  them, when, and the saved statuses are lost, so those tasks can no longer be
  put back by a reopen. `closed` notices stay in bells with the default icon.
- **Code-only revert** (keep the columns): also safe. Old code doesn't select
  the new columns. Closed projects reappear in their lanes.

## Commits, in order

One PR (Raj prefers fewer, bigger PRs). Each commit builds and passes tests.

1. Migration, entities, `projects` and `tasks` model changes (close, reopen,
   `in_lane`, `lock_open_for_tasks`, the saved-status rules), model tests.
2. Every lane filter goes through `in_lane`: roadmap, Home, task picker,
   board, Claude list. Plus the source test. (Nothing is closed yet, so
   nothing changes on screen.)
3. Task refusal on the web and through Claude, with tests.
4. Close and reopen routes, the edit sheet box and confirm step, the Done
   column and done cards, the phone switcher, CSS and `app.ts`, with tests.
5. The `closed` notice, with tests.
6. The Home donut, with unit and request tests.
7. Claude: `list_projects` fields, description and instructions, with tests.

## Out of scope

From the brief: closing or reopening from Claude; a "cancelled" status;
closing a lane or all of "now"; hiding old done projects; more charts;
tappable segments; closing chores; a changelog page. From the mockup's open
questions: a reopen notice, a project page, clearing the free-text status on
close.

## Risks

- **A missed lane filter** shows a closed project as current. `in_lane` and
  the source test make this hard to miss. Template-only lane checks would
  escape the grep; there are none today (`grep lane assets/views` finds only
  the roadmap and the board's own swimlanes).
- **The race between close and task writes.** Handled by `FOR SHARE` in the
  task transaction (section 2). The concurrency test may be fiddly on CI.
- **Deadlock.** Both paths lock project then task. A task update that keeps
  its project takes no project lock, so it can't form a cycle. Unverified
  under load; the engineer logs any `40P01` while running the tests.
- **SeaORM `update_many` with `RETURNING`** and `col_expr(Column::Status)` as
  a column reference: the API exists (`drop_notices` uses
  `exec_with_returning`), but the column-to-column `SET` is unverified in
  this SeaORM version. Fallback: the same SQL as a raw statement with bound
  values (`Statement::from_sql_and_values`).
- **Stale edit sheets.** A sheet opened before a close can still post. The
  project update is refused (409), and the confirm step's close is a no-op.
- **CSS variables in SVG.** Using classes, not presentation attributes, so
  dark mode works. Checked by the dark screenshots.
- **Notice volume.** One per person, so at most the team size per close.

## Rollback

Revert the PR, then roll back the migration if wanted (`cargo loco db down`
once). The revert alone is safe with the columns left in place. See "The
migration" for what each step loses.

## Checks

```sh
bin/build-assets
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

Also: `cargo loco db down` then `cargo loco db migrate` locally, to prove the
rollback runs.

By hand, locally, with realistic data: Juniper Tea Co from the mockup. Ana
(owner), Maya, Tom (member, owns Wholesale catalogue), Leo (member). Three
"now" projects (Website with 4 open tasks, 1 blocked, Maya on 2 of them),
plus some in next and later, and enough closed projects (14) to show "Show
all".

- Ana closes Website on a phone: box, confirm with the right sentence and
  names, "Marking done…", toast "Website done. 4 tasks closed.", switcher
  stays on Now, Done counts up.
- Maya's bell: one notice, the green tick, it opens the Done lane.
- Tom sees no box on Website and no Reopen on its done card.
- Ana reopens: the blocked task is blocked again; last week's done task is
  still done; toast counts restored tasks.
- Set a closed project's task back to in progress: the blue pill, the board
  swimlane without add buttons.
- Leo's task form: Website not in the picker. A form opened before the close
  shows the error under Project.
- Claude (connector): "what's done on the roadmap?" lists the closed
  projects; `create_task` into Website is refused with the message.
- Stop the server mid-confirm to see the inline alert; same for Reopen's
  error toast.
- Home: donut at 62%, 100% case, no-tasks case, empty case.

### Screenshots needed

From the local app with the data above, on the `pr-screenshots` branch under
`roadmap-done/`. Desktop 1440px and phone 390px, light and dark (colours and
layout change).

- Desktop roadmap with four columns, Done showing 5 of 14 and "Show all 14"
  (§1). The same after "Show all".
- Phone switcher on Now and on Done (§2), and at under 360px.
- Edit sheet with the box, the confirm step, the saving state, the error
  alert, and the toast after (§3), phone and desktop.
- Done cards: with Reopen, without (Tom), with "1 open task" (§4).
- The task form refusal under Project (§4).
- Home donut: 62%, 100%, and "now" with no tasks (§5), phone and desktop.
- Maya's bell with the closed notice (§6).
- Claude's `list_projects` result and the refused `create_task`, as a
  screenshot of the tool output.

## Decisions for Raj

Each with a recommendation. None block the build.

1. **Donut percentage rounds down**, unlike the mockup's sketch, so "100%"
   means everything is done. Recommend: round down, like project progress.
2. **A super admin acting can't close or reopen**, like delete. Closing
   changes other people's tasks and sends notices in their name, and they
   aren't on the team, so the notice would read "Someone". Recommend: refuse.
3. **The board hides a closed project's swimlane** unless one of its tasks is
   open again; then it shows last, with a Done badge and no add buttons.
   Recommend: yes. Showing every closed project would fill the board with
   finished work; hiding all would make a reopened task vanish from the
   project view.
4. **Editing a closed project is refused** (409 with a toast). The done card
   has no pencil, so only a stale sheet hits this. Recommend: refuse; reopen
   to edit.
5. **"Changed since" means the status moved off done, or the task moved to
   another project.** A title or date edit doesn't count. Recommend: yes, it
   matches what people mean by "someone changed it".
6. **Owner notice when nothing was open** reads only "Ana marked Website
   done". Recommend: still send it; it's their project.

## Amendments after review

These are binding and override anything above that says otherwise.

- **R1. Entities keep their hand edits.** `cargo loco db entities` would drop
  the hand-added `#[serde(skip)]` on `claude_grant_id` and its doc comment in
  `src/models/_entities/projects.rs` and `tasks.rs`. Add the three new fields
  by hand (or regenerate and restore). `git diff src/models/_entities` shows
  only the new fields.
- **R2. A refused task form keeps the project selected.** On a refused
  re-render (create and update), the posted `project_id` is passed as
  `current_project`, so it stays selected under "Done" and a second Save is
  refused again. Tested.
- **R3. Stale-tab 409s are skipped by the new handlers.** The
  `[data-close-confirm]` and `[data-reopen]` handlers in `app.ts` skip 409s,
  since the stale-tab 409 carries `HX-Redirect`. The closed-project 409 never
  carries `HX-Redirect`. Request test: toast in `HX-Trigger`, no `HX-Redirect`.
- **R4. More tests.** The forget-saved-status rule is tested via `set_status`
  (board drag), the web update and Claude's `update_task`. `GET
  /roadmap/projects/{id}/edit` on a closed project is 409.
- **R5. The closed-project edit refusal lives in the controller** (`update` and
  `edit`), like `destroy`'s checks. `update_from` does not refuse. There is a
  harmless race: an edit that reads the project just before a close commits
  can still save a name or lane change onto a project that is now closed. The
  lane is kept but unused while closed, so nothing shows wrongly.
- **SeaORM, no raw SQL.** Close uses `col_expr(Column::StatusBeforeClose,
  Expr::col(Column::Status))` and `exec_with_returning`. The raw-SQL fallback
  is dropped.
- **Stale-tab test.** The line about adding the routes to
  `tests/requests/stale_tab.rs` is removed; that test already walks
  `App::routes`.

Optional items taken on:

- The source test is broader: any `.lane` field read (`\.lane\b`) or
  `Column::Lane` outside `src/models/projects.rs` fails, with a small
  allowlist of display uses.
- No `closed_for_org`. The roadmap loads the org's projects once and filters
  closed ones in memory.
- `before_save` acts only when `status` is `ActiveValue::Set`.
- `lock_with_tasks`'s doc comment is updated to mention close and reopen.
- The concurrency test is deterministic: open transaction A and take
  `lock_open_for_tasks`, spawn the close, check it has not finished after
  about 200ms, insert and commit A, then check the close includes the task.
- Roadmap grid: four columns are checked at about 1024px with the sidebar.
  If tight, 2x2 until about 1200px.
- Donut arcs use an explicit `stroke-dasharray="{dash} {C}"` and butt caps,
  so the 2px gaps show.
- Claude's instructions say a closed project is not current work.

Rulings on the decisions for Raj: the donut rounds down (integer division); an
acting super admin can't close or reopen; a closed project has no swimlane
unless a task is open again; editing a closed project is 409; title and date
edits don't count as changed-since; the owner gets a notice even with nothing
open.

Follow-ups, not built here:

- A stale task form could reopen a task by posting its old status (a
  `status_was` check would catch it).
- Deleting a done project needs a reopen first in the web app.
- A notice on reopen.
