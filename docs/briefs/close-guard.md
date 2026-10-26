# Close guard: a project close only finishes the closer's own work

Status: Draft (calls made, see the end) · Owner: Raj · 26 October 2026

## Summary

A project owner who is not an organisation owner or admin can close a project.
The close then marks done only open tasks that are theirs or nobody's. Tasks
assigned to anyone else stay open, and the confirm says so first. Organisation
owners and admins keep today's full close. One small PR.

## Who and why

The security review of PR #62 found a three-step abuse open to any approved
member. The steps work today and pre-date #62.

1. Tom creates "Spring clean" and picks himself as owner. The maker picks the
   owners (`create` and `form_data` in `src/controllers/roadmap.rs`).
2. Tom moves Ana's and Maya's open tasks into it. Any member can edit any
   task, project and assignees included (`update_from` in
   `src/models/tasks.rs` only checks that the new project is open).
3. Tom marks the project done. `close` in `src/models/projects.rs` marks every
   open task done in one statement. Ana and Maya get "Tom marked Spring clean
   done · 3 of your tasks closed" (`project_closed` in
   `src/controllers/notifications.rs`).

Reopen undoes it, but only Tom or an admin can reopen. Ana can only fix her
tasks one by one. Claude cannot close a project, but it can do steps 1 and 2
(`create_project` with `owners`, `update_task` with `project_id` in
`src/mcp/mod.rs`).

Note what is not new. Any member can already mark any single task done
(`set_status` in `src/controllers/tasks.rs`), silently. That openness is by
design. The problem is the bulk change, sent in the closer's name, of work the
closer has no claim to.

## Decisions

1. Organisation owners and admins close as today: every open task is marked
   done. They already have full power over the organisation.
2. A project owner who is not a manager marks done only open tasks with no
   assignee other than themselves: their own, and unassigned ones. A task
   with Ana on it stays open, even if Tom is also on it. Otherwise Tom could
   add himself to Ana's task and close it.
3. Unassigned tasks are still closed. Nobody's work changes, and any member can
   already mark them done one at a time. This keeps a normal close useful.
4. The close still goes ahead. The project moves to Done, and its card shows
   "3 open tasks", which already works.
5. Each person whose task stayed open gets one notice: "Tom marked Spring clean
   done · 2 of your tasks left open". It is true, and it tells Ana where her
   work went. She can move it out (moving out of a closed project is allowed)
   or finish it.
6. The rule is read inside the close's locked transaction, after
   `lock_with_tasks`. A task edit locks the task row, so assignees can't
   change between the check and the write.
7. Reopen is unchanged. It only restores tasks the close changed.
8. No change to who may create projects, pick owners or move tasks. No notice
   when a task is moved into a project.

Rejected:

- Only managers may close a project with others' tasks in it. This blocks the
  normal case: Leo owns Website, and Ana and Maya do the tasks.
- Notify assignees when their task is moved into a project. It warns but does
  not block, and adds noise to every normal move.
- Stop members owning a project they create. Most projects are run by members.

## What people can see

For a manager, nothing changes.

For a member owner, when some open tasks belong to others:

- Box line before tapping: "1 open task is marked done with it. 3 assigned to
  others stay open."
- Confirm: "1 open task will be marked done. 3 assigned to Ana and Maya stay
  open." Today's blocked clause still applies to the tasks being closed.
- When none are theirs: "No tasks are marked done. 3 assigned to others stay
  open."
- Toast: "Spring clean done. 1 task closed, 3 left open."
- The "who will be told" line includes the people whose tasks stay open.

When every open task is theirs or unassigned, the copy is today's.

Claude: no change. It still has no close tool. `list_projects` already shows
a closed project, and the open counts follow from the tasks.

Someone waiting for approval sees none of this. `CurrentMember` already
requires an approved member.

## Not in this version

- Limiting who can edit a task, move it, or change its assignees.
- Limiting who can pick owners when making a project.
- A notice when a task is moved into a project.
- Letting assignees reopen, or close a single task out of a closed project.
- A close or reopen tool for Claude.
- An audit log of closes.

## Done when

- Tom, a member, owns Spring clean. It holds his 1 open task and 3 of Ana's
  and Maya's. His confirm on a 390px phone reads "1 open task will be marked
  done. 3 assigned to Ana and Maya stay open." He confirms.
- His task is done. Ana's and Maya's tasks keep their status. Ana gets one
  notice saying 2 of her tasks were left open.
- Raj, an admin, closes Website with Ana's tasks in it. All are marked done,
  as today, with today's copy.
- Leo owns Tins and every open task is his or unassigned. His close looks and
  works exactly as before.
- Tom reopens Spring clean. Only his task goes back to its old status.

## Risks and open questions

- The confirm counts are read before the tap. The toast must report what the
  close actually did, as it does now.
- `CloseSummary` needs the closer, their role and the assignees. All places
  that build it (`close_data`, the toast, tests) must pass the same rule as
  `close`. One shared function avoids drift.
- No migration. Rolling back restores today's behaviour.
- Tests: member owner with mixed tasks, a shared task with the closer on it,
  unassigned tasks, an admin close, and a super admin acting (still refused).
- Unverified: the longer confirm sentence fits at 390px without wrapping badly.

Calls Raj may want to change (none block the work):

1. Unassigned tasks are closed by a member owner. Recommend keeping it, or a
   normal close leaves loose ends.
2. People with tasks left open get a notice. Recommend keeping it, so no task
   is stranded in a closed project unseen.
