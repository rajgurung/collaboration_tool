# Roadmap: done projects and a progress donut

Status: Draft (Raj away; calls made, see the end) · Owner: Raj · 25 October 2026

## Summary

A project can be marked done. Its open tasks are marked done with it, and it
moves to a new Done column on the roadmap. Reopening puts it back, and its
tasks come back as they were. Home gets one donut: how much of the "now" work
is done.

## Who and why

Raj asked: "What's missing is the done part: the things that have been
completed. We need to make sure the roadmap can be closed, and once that
roadmap is closed, the following tasks beneath that get closed as well." And:
"On the dashboard itself, in the roadmap section, do you reckon we could have
a donut chart to show where we are at?"

The team is four people: Ana, Maya, Tom and Leo. Most use phones.

What the code does today:

- A project has a lane: now, next or later (`LANES` in `src/models/projects.rs`).
  Nothing marks a project finished. A finished project stays in "now" at 100%,
  or someone deletes it, which also deletes its tasks and notes for good.
- A project also has a free-text status such as "Active" or "Planned". People
  may already type "Done" there. That changes nothing else.
- Tasks are todo, progress, blocked or done. A task already records when it
  was finished (`done_at`, set in `tasks.rs` `before_save`).
- Any approved member can edit a project. Only owners and admins can delete one
  (`destroy` in `src/controllers/roadmap.rs`).
- Home shows "Roadmap: now" as cards with a progress bar each. There is no
  overall picture.
- Claude's connector can list, create and delete projects, and update tasks.
  It describes lanes as "now, next or later" (`INSTRUCTIONS` in `src/mcp/mod.rs`).

## Decisions

1. Done is a closed state, not a fourth lane. A project gets `closed_at` and
   `closed_by`, and keeps its lane. A "done" lane could be picked in the edit
   form or by Claude's `create_project` without closing any tasks. The closed
   state can only be set by the close action, which always handles the tasks.
2. Closing marks every open task done, blocked ones included. The confirm sheet
   says what will happen first: "4 open tasks will be marked done. 1 is
   blocked." There is no "cancelled" status in this version.
3. Who can close and reopen: owners, admins and the project's owner. It
   changes other people's tasks in bulk, so it is a step above editing.
4. One notice per person, not one per task. Each assignee of a task that was
   closed gets "Raj marked Website done · 3 of your tasks closed". The project
   owner gets one too. The person closing gets nothing.
5. A closed project can be reopened. It goes back to its old lane. Tasks the
   close marked done go back to their earlier status, unless someone changed
   them since. Tasks that were already done stay done. This makes a mistaken
   close fully undoable, which a delete never is.
6. The roadmap gets a fourth column, Done, after Later. Newest first, showing
   the closed date and "12 tasks done". It shows the latest 5 with "Show all"
   below, like done tasks on the board (`DONE_SHOWN`). Nothing is hidden by age.
7. The donut shows tasks in "now" projects by status: done, in progress,
   blocked, to do. The centre reads "62% done". Closed projects and chores are
   left out, so it measures current work. One donut for the whole organisation.

## What people can do (first version)

Closing:

1. Open a project's edit sheet and tap "Mark project done".
2. See a confirm with the count of open and blocked tasks. Confirm.
3. The card moves to Done. A toast says "Website done. 4 tasks closed."

On a phone the lane switcher becomes Now, Next, Later, Done. Done projects have
no edit pencil, only "Reopen" for those allowed. Their tasks still open and
read as normal.

A closed project is read-mostly:

- It is left out of the task form's project picker. The server refuses a new
  task, or a task moved, into a closed project: "Reopen the project first."
- Someone can still set one of its tasks back to "in progress". The project
  stays closed and its card shows "1 open task".
- It does not show in Home's "Roadmap: now".

The donut sits at the top of "Roadmap: now" on Home, about 120px, with a legend
and counts beside it. It is drawn on the server as SVG, with no chart library.
It uses the existing status colours. It has a text label for screen readers. With
no tasks in "now", the current empty card shows instead.

Claude's connector: `list_projects` adds `closed` and `closed_at`. The
instructions add one sentence: a project can be closed, and closed projects
take no new tasks. `create_task` and `update_task` follow the same refusal as
the web. Claude gets no tool to close or reopen.

Someone waiting for approval sees none of this. `CurrentMember` already
requires an approved member for the roadmap and Home.

## Not in this version

- Closing or reopening from Claude.
- A "cancelled" or "won't do" status for tasks.
- Closing a single lane, or all of "now" at once.
- Archiving or hiding old done projects by age.
- More charts: per-project donuts, trends over time, burn-down.
- Tapping a donut segment to filter tasks.
- Closing chores (tasks with no project).
- A changelog or "what we shipped" page.

## Done when

- Raj opens "Website" on his phone, taps "Mark project done", reads "3 open
  tasks will be marked done. 1 is blocked." and confirms. The card is in Done
  with today's date. All its tasks show as done.
- Maya had 2 of those tasks. Maya gets one bell notice, not two.
- Tom, a member who is not the project owner, sees no "Mark project done".
- Raj taps Reopen. Website is back in "now". The blocked task is blocked again.
  The task done last week is still done.
- Leo tries to add a task to Website while it is closed. It is not in the
  picker, and Claude's `create_task` is refused with a clear reason.
- Home shows a donut, say "62% done", with done, in progress, blocked and to do
  counts that add up to the "now" projects' tasks. It reads well at 390px and
  in dark mode.
- Asked "what's done on the roadmap?", Claude lists the closed projects.

How we will know it worked: within a month the "now" lane holds only live
work, and no project has been deleted just to get it off the roadmap.

## Risks and open questions

Risks:

- Every place that filters by lane must also skip closed projects: the roadmap
  lanes, Home's "now" cards and donut, the task picker (`project_groups` in
  `src/controllers/tasks.rs`), the board's project swimlanes and Claude's list.
  Miss one and a closed project shows as current. The plan should find them
  all with a search for `lane`.
- Bulk change. Closing writes many tasks at once. Do it in one transaction,
  locking the project and its tasks like `lock_with_tasks` already does.
- Reopen depends on remembering each task's earlier status. Assumption: a
  nullable `status_before_close` on tasks, cleared on any later status change.
  The plan can choose another shape.
- Notifications. The bulk close must not call `task_saved` per task. It needs
  its own rule and a new notice kind in `src/models/notifications.rs`.
- Tenancy. Close and reopen take a project ID. Both must check the project is
  in the member's organisation, with a test like `tests/requests/tenancy.rs`.
- Migration. Two nullable columns on projects, one on tasks. Rolling back is
  safe: closed projects reappear in their old lanes, and their tasks stay done.
- Wording. "Done" on the roadmap and the free-text status "Done" can clash.
  The designer should make the closed badge look different from the status pill.
- Unverified: four segments fit the phone lane switcher at 390px.

Calls Raj may want to change (none block the work):

1. Who can close: owners, admins and the project owner. Recommend keeping it.
   Any member would be simpler but lets anyone close everyone's tasks.
2. Blocked tasks are closed too. Recommend keeping it, with the warning in the
   confirm. Refusing to close would leave projects stuck in "now".
3. The donut covers "now" only. Recommend keeping it. Counting next and later
   would make the team look less done than it is.
