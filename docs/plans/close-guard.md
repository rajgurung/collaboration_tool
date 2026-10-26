# Close guard: a project close only finishes the closer's own work

Status: Draft, for review · One PR · Owner: Raj · 26 October 2026

Brief: `docs/briefs/close-guard.md`. No mockup: the change is copy in the
existing close box, confirm, toast and notice. This plan covers the model,
the controllers, the notices and the tests.

## Summary

A project owner who is not a manager (`CurrentMember::can_manage()` is
false) can still close a project. The close marks done only open tasks
whose assignees are empty or just the closer. A task with anyone else on it
stays open. Managers close as today.

One rule, one function: `projects::Model::closes`. The model's `close`, the
box and confirm counts (`close_data`) and the toast all go through it, so
the preview and the write can't drift.

No migration. No change to who can close, reopen, edit tasks or pick owners.

## What the code does today

- `projects::Model::close` (`src/models/projects.rs`) locks the project and
  its tasks (`lock_with_tasks`), rechecks owners (`closable_by`), then one
  `update_many` sets every non-done task to done, saving the old status in
  `status_before_close`. Returns `Closed { project, tasks, already }`.
- `reopen` restores only tasks with `status = 'done'` and
  `status_before_close IS NOT NULL`, then clears any leftovers. A task the
  close didn't touch never gets `status_before_close`, so reopen already
  leaves it alone. No reopen change needed.
- Callers of `close`: `close` in `src/controllers/roadmap.rs` (the confirm
  button posts here). Dropping a card on Done (`roadmapDone` in
  `frontend/app.ts`) only opens the edit sheet and clicks "Mark project
  done", so it uses the same box, confirm and route. Claude has no close
  tool (`src/mcp/mod.rs` only lists `closed`). Tests call the model
  directly with `can_manage = true`.
- `close_data` (`src/controllers/roadmap.rs`) builds the box line, the
  confirm sentence, the pills and the "who will be told" line from
  `CloseSummary::of`, which counts every task in the project.
- `project_closed` → `tell_per_person` (`src/controllers/notifications.rs`)
  sends one `kind::CLOSED` notice per assignee of the closed tasks, then the
  total to owners not told yet. Team members only; the actor is skipped by
  `notifications::Model::notify`.
- The Done card's "N open tasks" pill (`open_count`, from
  `CloseSummary::of(...).open()`) already counts every non-done task. It
  works for tasks left open with no change.

## Why the check is safe inside the transaction

Assignees change only through `task_assignees::Model::replace`, called from
`tasks::Model::create` and `update_from`. `update_from` writes the task row
first (title and the rest are always `Set`), then replaces assignees, in one
transaction. `lock_with_tasks` holds every task row `FOR UPDATE`. So an edit
either commits before the close takes its locks, and the close's next
statement sees it (read committed), or it waits until the close commits. A
new task waits on the project row (`lock_open_for_tasks`). Removing a member
doesn't touch `task_assignees`.

## What changes, file by file

### 1. `src/models/task_assignees.rs`

Add `for_tasks(db, org_id, task_ids: &[i64]) -> HashMap<i64, Vec<i64>>`,
like `by_task` but filtered with `TaskId.is_in(ids)`. Returns an empty map
for no ids without a query.

### 2. `src/models/projects.rs`

- **The rule.**
  ```rust
  /// Whether a close by `closer` marks this open task done. Managers close
  /// every task; anyone else only tasks with no assignee but themselves.
  pub fn closes(can_manage: bool, closer: i64, assignees: &[i64]) -> bool {
      can_manage || assignees.iter().all(|id| *id == closer)
  }
  ```
  Any other assignee counts, including a removed member still on the task.
  That keeps the task open (safe side), and no notice goes out for it.
- **`CloseSummary`** gets `pub kept: usize` (open tasks a close leaves).
  `of` is unchanged (kept 0), for the Done card. New
  `CloseSummary::for_close(project_id, all_tasks, assignees, closer,
  can_manage)` puts each open task in `todo`/`progress`/`blocked` when
  `closes(...)`, else in `kept`. `open()` is now "tasks the close marks
  done", which is what it already means.
- **Copy.** When `kept == 0`, every string is today's (existing unit tests
  stay as they are). Otherwise:
  - `sentence(&self, who: &str)`: `who` is the kept tasks' people, e.g.
    "Ana and Maya". `open > 0`: today's lead and blocked clause, then
    "3 assigned to Ana and Maya stay open." (`1 … stays open.`).
    `open == 0`: "No tasks are marked done. 3 assigned to Ana and Maya stay
    open."
  - `box_line()`: "1 open task is marked done with it. 3 assigned to others
    stay open." / "2 open tasks are marked done with it. …". `open == 0`:
    "No tasks are marked done. 3 assigned to others stay open."
  - Pull the name joining out of `notice_line` into `fn names(&[String]) ->
    String` ("Ana", "Ana and Maya", "Ana, Maya and 3 others" past four), used
    by both. No names on the team (all removed): "others".
- **`Closed`** gets `left_open: Vec<tasks::Model>`.
- **`close`.** After the owner check and the `already` return:
  - `can_manage`: today's `update_many`, `left_open` empty.
  - Otherwise: load `task_assignees::Model::for_tasks(&txn, org_id, &ids)`
    for the ids `lock_with_tasks` returned. Split ids with `closes`. Run the
    same `update_many` with `.filter(tasks::Column::Id.is_in(closing))`
    (skip it when `closing` is empty). Read `left_open` as the `kept` ids
    with `status != 'done'` (rows already locked).
  - The project is closed either way (brief, decision 4).
- Doc comments on `close`, `CloseSummary::open` and `closable_by` say who
  closes what.

### 3. `src/controllers/roadmap.rs`

- **`close_data`**: one `task_assignees::Model::by_task` (already loaded),
  then `CloseSummary::for_close(project.id, &all_tasks, &assignees,
  member.user.id, member.can_manage())`. `who` = team names of the kept
  tasks' assignees, minus the viewer, in first-met order, through `names`.
  The "who will be told" set stays open tasks' assignees plus owners, minus
  the viewer. That already includes the people whose tasks stay open; build
  it from the closing and kept tasks so the intent is plain, and test it.
  Pass `summary.sentence(&who)` and `summary.box_line()`.
- **`close` handler** toast, from what the close did:
  - `left_open` empty: today's.
  - else `(n closed, k kept)`: "Spring clean done. 1 task closed, 3 left
    open." / "… 2 tasks closed, 1 left open." / `n == 0`: "Spring clean
    done. 3 tasks left open."
  - `tracing::info!` adds `left_open = closed.left_open.len()`.
  - Pass `&closed.left_open` to `project_closed`.
- **`DoneCard.open_count`** doc comment: "Open tasks: left open by the
  close, or set back since." No logic change.

### 4. `src/controllers/notifications.rs`

- `project_closed(ctx, member, project, closed, left_open)`.
  `project_reopened` passes `&[]`.
- `tell_per_person` takes `left_open: &[tasks::Model]`. Reuse
  `kind::CLOSED` (same icon and link to Done; the kind column is free text,
  so a new kind would only add a template branch for no gain).
  - Pull the per-person counting loop into a small helper used twice.
  - First loop: closed tasks, "N of your tasks closed" (today).
  - Second loop: kept tasks, body "marked Spring clean done\n2 of your
    tasks left open", skipping anyone already told. In a member close the
    two sets can't overlap (closed tasks have no assignee but the closer);
    in a manager close `left_open` is empty.
  - Same team filter and actor skip as today.
  - Owners not told: today's body, plus ", 3 left open" when `left_open` is
    non-empty: "marked Spring clean done\n1 task closed, 3 left open"; with
    none closed, "…\n3 tasks left open".
- Doc comment on `project_closed` updated.

### 5. Templates

`assets/views/roadmap/_form.html`: no structural change. The box and confirm
already print `close.box_line` and `close.sentence`; the pills read
`close.summary.*`, which now count only the tasks being closed. Done card
(`_lane.html`): no change.

Claude (`src/mcp/mod.rs`): no change.

## Edge cases

- **Closer added to someone's task**: `[tom, ana]` fails `closes`; stays open.
- **Unassigned task**: closed by a member owner.
- **Closer alone on a task**: closed.
- **Removed member still assigned**: task stays open; no notice to them.
- **All open tasks kept**: the project still closes; nothing marked done;
  toast says how many were left open.
- **Counts change between confirm and tap**: the toast reports what the
  close did, as now.
- **Role changes mid-request**: `can_manage` comes from the request, as
  today. Only the owner list is rechecked in the transaction.
- **Ana marks a kept task done later**: it has no `status_before_close`, so
  a reopen leaves it done (today's rule for changes after a close).
- **Ana moves a kept task out**: allowed today (moving out of a closed
  project is fine).

## Tests

### Unit (`src/models/projects.rs`)

- `closes`: manager true for anything; member true for `[]` and `[me]`,
  false for `[ana]` and `[me, ana]`.
- `sentence` / `box_line` with `kept`: the brief's four strings exactly,
  blocked clause with kept, `1 … stays open`, and every existing case
  unchanged with `kept == 0`.
- `names`: one, two, four, five-plus.

### Model (`tests/models/projects.rs`)

- Member owner closes a project with: his task, an unassigned task, Ana's
  task, a task on `[tom, ana]`, one done task. His and the unassigned one
  are done with `status_before_close` set; Ana's and the shared one keep
  their status and have no saved status. `left_open` holds the two.
  Project is closed.
- Same project closed with `can_manage = true`: everything done, `left_open`
  empty (today).
- All kept: project closed, no task changed.
- Reopen after the member close: only the two closed tasks go back; the
  kept ones are untouched (`updated_at` unchanged).
- Existing callers that pass `true` stay green.

### Requests (`tests/requests/roadmap_done.rs`)

- Tom (member) owns Spring clean, 1 open task of his, 3 of Ana and Maya's.
  The edit sheet's box reads "1 open task is marked done with it. 3
  assigned to others stay open." The confirm reads "1 open task will be
  marked done. 3 assigned to Ana and Maya stay open." The notice line names
  Ana and Maya. Close: toast "Spring clean done. 1 task closed, 3 left
  open."
- Notices after that close: Ana one "…\n2 of your tasks left open", Maya one
  "…\n1 of your tasks left open", a co-owner not assigned gets "…\n1 task
  closed, 3 left open", Tom none. All `kind == "closed"`.
- Raj (admin) closes Website with Ana's tasks: today's copy, all done,
  today's notices.
- Leo owns Tins, all open tasks his or unassigned: today's box, confirm and
  toast.
- None of Tom's: "No tasks are marked done. 2 assigned to others stay open."
  and toast "Spring clean done. 2 tasks left open."
- Done card for Spring clean shows "1 task done" and "3 open tasks".
- A super admin acting still gets 403 (existing test stays).

## Commits

1. Model: `closes`, `for_tasks`, `close` split, `CloseSummary` copy, unit
   and model tests.
2. Controllers: `close_data`, toast, notices, request tests.

## Risks

- **Preview drift.** The confirm reads assignees outside the transaction; the
  close reads them inside. Both call `closes`, so only a real edit in
  between can differ, and the toast reports the truth.
- **Notices read assignees after commit** (as today), so a reassignment
  right after the close could shift who hears. Same race as now.
- **Longer confirm at 390px.** Unverified; check in the screenshots.
- **A co-owner with kept tasks** gets the "left open" notice only, not the
  closed total. Accepted: the Done card shows both counts.

## Rollback

No migration. Revert the PR to restore today's close. Closes made in
between stay as they are; reopen works for them either way.

## Checks

```sh
bin/build-assets
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

By hand, locally, Juniper Tea Co with Raj (admin), Tom, Ana, Maya and Leo
(members). Spring clean owned by Tom with "Sweep the shop" (Tom), "Order
tins" and "Price list" (Ana), "Shelf labels" (Maya, Tom). Tins owned by Leo
with his tasks and one unassigned. Website with Ana's tasks.

- Tom opens Spring clean, then drops it on Done: box, confirm, toast as
  above. Ana's bell shows the "left open" notice.
- Tom reopens: only "Sweep the shop" goes back.
- Raj closes Website; Leo closes Tins: today's copy.

### Screenshots needed

On the `pr-screenshots` branch under `close-guard/`. Desktop 1440px and
phone 390px. No colour or layout change, so light mode only.

- Tom's edit sheet with the new box line (desktop, phone).
- Tom's confirm step (desktop, phone; the 390px one checks wrapping).
- The toast after Tom's close.
- The Done lane with Spring clean showing "3 open tasks".
- Ana's bell with "Tom marked Spring clean done · 2 of your tasks left
  open".
- Raj's confirm for Website, unchanged.

## Review changes

Recorded after the security and staff reviews. These override the sections
above where they differ.

Security:

- **S1. No unfiltered fallback.** The member close's `update_many` keeps the
  `ProjectId` and `Status != "done"` filters and adds `Id.is_in(closing)`.
  When `closing` is empty the statement is skipped. Tests: all kept, no task
  changed; mixed, kept tasks still have `status_before_close` NULL.
- **S2. Read inside the lock.** Assignees are read on `&txn` after
  `lock_with_tasks`, with `task_assignees::Model::for_tasks`, which uses
  `.in_tenant(org_id)` like `by_task`. `left_open` is read on `&txn` from the
  locked ids, `status != done`. `close_data`'s map is not reused.
- **S3. An enum, not a bool.** `can_manage: bool` in `close`, `closes` and
  `CloseSummary::for_close` becomes `CloseScope::Everything |
  OwnWork(user_id)`, built once in the roadmap handler from the real
  membership, with a doc comment. The order stays `closable` then
  `can_close` (a super admin acting gets 403), and its test stays.

Staff:

- **T1. Tell people taken off a task.** `task_saved` sends a notice to
  people removed from a task's assignees: "Tom took you off “Order tins”".
  Same team filter as other notices, never the actor. Deleting a task sends
  nothing (it doesn't go through `task_saved`). Tests: Tom swaps Ana for
  himself and Ana is told; Tom unassigns Ana from one task then closes, and
  only that task closes, Ana's other task stays open, and Ana was told about
  the removal. This is the accepted residual: a member can still take
  someone off a task and then close it, but not silently.
- **T2. Removed members.** Test: a kept task whose only assignee is a
  removed member stays open, is in `left_open`, no notice goes to them, and
  the confirm reads "… assigned to others stay open." When the kept tasks
  mix team members and removed members, the names read "Ana and others".
- **T3. Co-owner.** Test: Ana co-owns and has a kept task, and gets exactly
  one notice (the left-open one), not the owner total. Left-open notices go
  into `told` before the owners step.
- **T4. Singular copy.** Toast with nothing closed: "Spring clean done. 1
  task left open." Owner notice with nothing closed: "…\n1 task left open".
  Both one: "…\n1 task closed, 1 left open". One test covers these.
- **T5. Correction.** "Tests call the model directly with `can_manage =
  true`" was wrong: `only_an_owner_or_admin_closes_or_reopens` passes
  `false`. Model tests now pass a `CloseScope`.
- **Logging.** The close's `tracing::info!` logs the kept and closed task
  ids, not titles.

Roadmap rights (extra scope from Raj, brief decision 9), in separate
commits after the close guard:

- **Who.** Managers (`can_manage`) as today. A member who owns the project
  (`project_owners`) may open its edit sheet and save name, status, colour
  and summary; owners by `OwnerRights`; close and reopen with
  `CloseScope::OwnWork`. Everyone else, and every other write, managers
  only: `new`/`create`, `delete`, `move` (reorder), `lane`, and the MCP
  `create_project` and `delete_project`.
- **Lane.** Owners who aren't managers can't change it. The edit sheet shows
  the lane select disabled with the current lane in a hidden field, and
  `update_from` keeps the locked row's lane when the editor may not plan,
  as it ignores `owner_ids`.
- **Refusals.** 403 through `forbidden`, which the frontend shows as a
  toast: "Only admins can add projects.", "Only admins and this project's
  owners can change it.", "Only admins can move projects between lanes."
  Delete keeps `require_manager`.
- **UI.** "+ Project" and both Reorder buttons, and `draggable` on lane
  cards, only for managers; `?reorder=1` only turns on Reorder for them. The
  edit pencil only for managers and the project's owners.
- **Tasks.** Unchanged.
- **Tests.** Non-owner member: 403 on new, create, edit, update, delete,
  move, lane. Member owner: edits and closes their own project; a posted
  lane is ignored; 403 on move, lane, delete. Manager unchanged. MCP
  `create_project` refused for a member. Roadmap page hides the controls.
