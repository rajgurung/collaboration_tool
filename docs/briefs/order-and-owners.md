# Card order and several project owners

Status: Draft (calls made, see the end) · Owner: Raj · 26 October 2026

## Summary

People can put task cards in the order they want inside a board column, and
project cards in the order they want inside a roadmap lane. The order is the
same for everyone. A roadmap project can have several owners, and each one has
the rights and notices a single owner has today. One PR.

## Who and why

Raj asked: "is there a way you could order a task and roadmap in the list
(swim lane)" and "can we please ensure that roadmap can have multiple owners".

The team at Juniper Tea Co is four people: Ana, Maya, Tom and Leo. Most use
phones.

What the code does today:

- Tasks and projects both have `sort_order`. Lists sort by it, then by id
  (`list_for_org` in `src/models/tasks.rs` and `src/models/projects.rs`). A new
  item gets the last number plus one. Nothing lets a person change it, so cards
  sit in the order they were made.
- Drag only moves a card to another column or lane (`frontend/app.ts`, the
  `drop` handler and `roadmapDrop`). The card is put at the bottom on screen,
  but its `sort_order` is unchanged, so after the refresh it jumps back to its
  old place among the others.
- HTML5 drag does not work with touch. On a phone today you open a task to
  change its status. A project can only change lane through its edit sheet.
- Any approved member can change a task's status (`set_status` in
  `src/controllers/tasks.rs`) or move a project's lane (`move_lane` in
  `src/controllers/roadmap.rs`).
- The board's Done column sorts by finish time and shows the newest 5. The
  roadmap's Done column sorts newest closed first.
- A project has one `owner_id`. It lets that person close and reopen the
  project (`closable_by`), gets them the "made you owner" notice
  (`project_saved`), task added and blocked notices (`task_saved`), and close
  and reopen notices (`tell_per_person`), all in
  `src/controllers/notifications.rs`. It shows as one avatar on the card.
- Claude's `create_project` takes one `owner`, and `list_projects` returns one
  `owner` (`src/mcp/mod.rs`).
- Tasks already have many assignees through `task_assignees`, shown as checkboxes
  in the task form and an avatar stack on the card. Owners copy this.

## Decisions

Order:

1. Anyone who can move the card can reorder it: any approved member, the same
   rule as changing status or lane. Ordering is low risk and easy to undo.
2. One shared order for everyone. It is a team plan, not a personal view.
3. A task has one place in the whole list. The board shows it in that order
   whether swimlanes are by project, person or none. Moving Ana's card above
   Tom's in the "Person" view also moves it in the "Project" view if both are
   in the same project and column.
4. A card dropped on another column or lane lands where it was dropped. A
   status or lane changed any other way (the sheet, the edit form, Claude) puts
   the card at the bottom of its new column, like a new item.
5. Done columns keep their own order (newest finished first) and can't be
   reordered. Cards can't be dropped between done cards.
6. Phones get a Reorder button in the board and roadmap toolbars. It shows up
   and down arrows on each card in place, and "Done" ends it. This also gives
   keyboards a way in. Desktop keeps drag, now with a drop marker between cards.
7. Reordering sends no notices and shows no toast. A failed save shows an error
   toast and the column refreshes.

Owners:

8. A project has zero or more owners. No cap: the picker lists the approved
   team, as task assignees do. Cards show up to 3 avatars, then "+2".
9. Every project owner can do what the single owner can today: close and
   reopen. Editing stays open to every approved member, as now.
10. Notices go to every owner except the person acting, and once per person.
    People added as owners get "made you owner of Website". People removed get
    nothing. A close tells each owner once, even if they also have tasks in it.
11. The migration copies each `owner_id` into a new `project_owners` table.
    `owner_id` stays in place, unread, and is dropped in a later PR.
12. Claude: `create_project` takes `owners`, a list of usernames, and still
    accepts the single `owner` (both are merged). `list_projects` and the
    create reply return `owners`, a list. Claude gets no tool to reorder.

## What people can do (first version)

Ordering tasks, desktop:

1. On the board, drag "Order tins" above "Book the courier" in To do.
2. A line shows where it will land. Drop. It stays there after a refresh, and
   Maya sees the same order on her phone.
3. Drag a card from To do into In progress, between two cards. It lands there.

Ordering on a phone:

1. On the board or a roadmap lane, tap Reorder. Each card gets up and down
   arrows. The top card has no up arrow; the last has no down arrow.
2. Tap up on "Spring blend" in Next. It moves above the card before it.
3. Tap Done. The arrows go and the cards open as normal again.

Roadmap order shows wherever projects are listed: the lanes, Home's "Roadmap:
now" cards, the task form's project picker and Claude's `list_projects`.

Owners:

1. Open a project's edit sheet. "Owner" becomes "Owners", a checkbox list of
   the team like a task's assignees. Tick Ana and Leo. Save.
2. Ana and Leo each get "made you owner of Website". The card shows both
   avatars.
3. Leo can now mark Website done. Tom, a member who is not an owner, cannot.

Someone waiting for approval sees none of this. `CurrentMember` already
requires an approved member, and the owner picker only lists approved people.

## Not in this version

- Touch drag on phones. The arrows come first.
- A personal order per person, or sorting by due date or priority.
- Reordering done cards, swimlanes, or the order of columns.
- Moving a card to another swimlane by drag (another project or person).
- A "lead owner" or owner roles. All owners are equal.
- Notices when someone is removed as owner.
- Reorder or owner changes through Claude, beyond `create_project`.
- An owner filter on the board or roadmap.

## Done when

- Ana drags "Order tins" to the top of To do. Tom refreshes on his phone and
  sees it at the top.
- Maya drops a card into In progress between two others. It is still there
  after the board refreshes.
- On a 390px phone, Leo taps Reorder on the roadmap, moves "Spring blend" up
  one, taps Done. Home's "Roadmap: now" shows the same order.
- Raj makes Ana and Leo owners of Website. Each gets one notice. Raj gets none.
- Leo marks Website done. Ana gets one close notice. Tom does not see the
  "Mark project done" button.
- Asked "who owns Website?", Claude names Ana and Leo.
- Every project that had an owner before the deploy still has that owner.

## Risks and open questions

Risks:

- Tenancy. The reorder calls take card ids. The server must check the moved
  card and every neighbour id are in the member's organisation, and that the
  neighbours share its status or lane. Same for owner ids. Tests like
  `tests/requests/tenancy.rs`.
- Order numbers. Putting a card between two others may need room that whole
  numbers don't have. Assumption: the plan renumbers the organisation's list in
  one transaction when there is no gap. Two people reordering at once must not
  leave two cards with the same number in a way that flips on refresh; the id
  tiebreak already keeps it stable.
- Filtered boards. With "Mine" or a search on, a drop places the card next to
  the visible cards only. Hidden cards keep their place. That is fine, but
  tests should cover it.
- Every reader of `owner_id` must move to the new table: `closable_by`,
  `close_data`, the three notice rules, both card types, the form and Claude's
  tools. A missed one silently drops a notice or a right. The plan should list
  each hit of a search for `owner_id`.
- Migration. Adds `project_owners` and copies rows. Rolling back drops the
  table after writing each project's first owner back to `owner_id`, so only
  extra owners are lost.
- A removed member stays in `project_owners`. Names and notices already filter
  by the approved team; rights do too, since a removed person can't sign in to
  the organisation. Unverified for the super admin acting case.
- Unverified: up and down arrows fit on a task card at 390px without hiding the
  title.

Calls Raj may want to change (none block the work):

1. Any approved member can reorder. Recommend keeping it. Limiting it to
   owners and admins would stop most of the team from planning their own work.
2. Phones use arrows in a Reorder mode, not touch drag. Recommend it for now.
   Touch drag is more work and easy to trigger by mistake while scrolling.
3. No cap on owners. Recommend keeping it, as with task assignees.
