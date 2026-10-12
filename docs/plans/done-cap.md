# Tasks: keep Done short

Status: Approved by Raj, 12 October 2026 · Small change (plan, build, review, PR)

## Why

The Done column grows without end. 33 tasks were finished in two days, so a
time window (e.g. 14 days) still leaves a wall of cards. Raj doesn't want a big
"show N older" either. (The 28 PR-tracking tasks have already been moved to a
"Changelog" project; that was a data change, not part of this PR.)

## What changes

1. **Record when a task was finished.** Migration adds
   `tasks.done_at timestamptz null`. Set it whenever a task's status becomes
   `done` (every path: status change, drag on the board, edit form, the Claude
   connector's `update_task`); clear it when the task leaves `done`. Put this in
   the tasks model (one place, e.g. a `before_save` or the status-change
   function the routes already share), not in each controller. Backfill:
   existing done tasks get `done_at = updated_at`. Down migration drops the
   column.
2. **Board: the Done cell shows the latest 5** per lane (and per lane in the
   desktop list layout), newest `done_at` first. When a lane has more, a quiet
   link under the cards: **"All done tasks"**, going to the Done view (3),
   filtered to the same project or person when the lane is one. No count in the
   link. Lane progress (`done`/`total`, `pct`) still counts every done task.
3. **Done view.** The existing list filter `done` (`/tasks?filter=done`)
   groups done tasks by when they were finished: **This week**, **Last week**,
   then one group per month (**September 2026**), newest first, in the
   organisation's time zone (`CurrentMember::tz()`). This replaces the single
   "Done" group for that filter only. The other filters don't show done tasks,
   as today. Search still works within it.
4. Home counts, roadmap progress, notifications and the Claude connector are
   unchanged (they read status, not this cap).

## Not in this change

Archiving or deleting old tasks; changing other columns; hiding Done on phones.

## Tests

- `done_at` set when a task moves to done by each route (status, board drag,
  form, MCP `update_task`), cleared when it moves back.
- Board lane with 7 done tasks renders 5, newest first, plus the link with the
  lane's project; a lane with 3 shows no link. Progress still counts 7.
- Done view groups tasks by this week, last week and month, in the org's time
  zone, and respects search.
- Migration backfill sets `done_at` from `updated_at` for existing done tasks.

## Checks

`bin/build-assets`, `cargo fmt`, `clippy -D warnings`, `cargo test`.
Screenshots: board with a capped Done column (1440px, light and dark) and the
Done view (1440px and 390px) with realistic data.
