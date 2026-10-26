# Member access: what a plain member can reach

Status: Approved by Raj, 26 October 2026 (decisions below) · Owner: Raj · 26 October 2026

## Summary

Owners and admins run the workspace. Members do their own work. In Documents,
members see one built-in "Shared" folder they can upload to, and the logins
they are named on. The rest is owners and admins only. Members stop seeing
teammates' emails, and only owners and admins can delete a task. No migration.
One PR, after close-guard.

## Who and why

Raj asked: "member can't access documents and other sensitive thing which i
can't think of". Raj wants it to work like Jira. Owners and admins (`can_manage`
in `src/extractors/current_member.rs`) run things. Members do their own work.

Close-guard (branch `close-guard`) covers the roadmap. It also makes Claude's
`create_project` managers-only. This brief does not touch either.

Juniper Tea Co: Ana (owner), Maya (admin), Tom and Leo (members). Most use
phones.

What a member can do today, area by area:

| Area | Member today | Risk | Call |
|---|---|---|---|
| Documents files | Sees every folder. Opens and downloads every file. Uploads, renames any file, deletes own (`documents.rs`). | Company papers open to all | Owners and admins only, except the Shared folder |
| Logins | Sees every login's title, username, URL and notes. Shows the password only if named on it (`can_reveal` in `models/logins.rs`). | Usernames and notes leak, and notes are plain text | Members see only logins they are named on |
| Login activity, security emails | Owners only (`require_owner`, `SecurityMailer`) | None | Keep |
| Members page | Sees every member's email and role (`MemberRow` in `members.rs`, `_row.html`). Join link, waiting list and time zone are already hidden. | Everyone's email | Hide emails, except your own |
| Join link | `/join/{slug}`. The slug is in every app address, so every member already knows it. A join is only a request. | Low | Keep |
| Chat | Only conversations you are in (`find_for_member`). Admins can't read other people's DMs either. | None found | Keep |
| #general photos | Saved as files in the Documents "Chat photos" folder (`create_photo`), but opened through chat | Would break if the Documents lock were applied to them | Keep: chat stays the way in |
| Meetings | Anyone adds. Everyone reads all. There is no edit or delete (`meetings.rs`). | A private meeting is seen by all | Keep |
| Tasks | Edit any task. Delete any task, for good, with no check at all (`destroy` in `tasks.rs`) | One tap wipes a teammate's task and its notes | Edit stays open. Delete becomes owners and admins only |
| Claude connector | A member's token can read projects, tasks, notes and members (username and role, no emails). It can create tasks, update any task, and delete only what it made in the last 24 hours. It cannot reach documents, logins, chat or meetings (`src/mcp/mod.rs`). | `create_project` had no role check; close-guard fixes it | Keep, plus close-guard's fix |
| Claude tokens | Each person sees and revokes only their own. Removing a member revokes theirs. | Low | Keep |
| Org settings | Only the time zone exists, and it is manager-only. There is no billing, rename or delete. | None | Keep |
| Platform admin | Super admin only. Anyone else gets a 404 (`admin.rs`). | None | Keep |
| Export | None exists | None | Keep |
| Bell, saved views, guide, Home | Your own data only | None | Keep |

Someone waiting for approval reaches none of this. `CurrentMember` only lets
in approved members.

## Decisions (Raj, 26 October 2026)

1. Members see only the logins they are named on, in a "Shared with you"
   list. They reveal them as today.
2. Only owners and admins delete tasks. Members can still edit tasks and mark
   them done.
3. Members get one shared area: "only a shared area, they can upload it."
   The design:
   - Every organisation gets one built-in folder named "Shared". It is made
     the first time anyone opens Documents, the same way "Chat photos" is
     made (`folders::Model::chat_photos`). New organisations get it with the
     starter folders. No migration and no new column.
   - Members see only this folder. They can open its files and add photos and
     files to it (25 MB, same types as today).
   - Members rename and delete only files they added. Owners and admins can
     rename and delete any file there, as in other folders.
   - Nobody can rename or delete the Shared folder. No other folder can be
     renamed to "Shared". The Logins folder can't be renamed either.
   - Rejected: a "shared with members" switch on each folder. It is more
     flexible, but it needs a migration and a setting admins must remember.
     One folder covers what Raj asked for.
4. Everything else in Documents is owners and admins only: other folders,
   their files, Chat photos, login lists and Login activity.
5. Chat photos keep working. Members open and delete them through chat
   (`open_photo` and `delete_photo` in `chat.rs`), as today. Those routes
   check conversation membership, not the Documents rule.

## Role matrix (first version)

| | Owner | Admin | Member |
|---|---|---|---|
| Documents: Shared folder, open and add files | Yes | Yes | Yes |
| Documents: Shared folder, rename or delete a file | Any | Any | Own only |
| Documents: every other folder and file | Yes | Yes | No |
| Logins: see and reveal | Any | Any | Only ones they are named on |
| Login activity | Yes | No | No |
| Members: names and roles | Yes | Yes | Yes |
| Members: emails | Yes | Yes | Own only |
| Approve, remove, time zone, join link | Yes | Yes | No |
| Change roles | Yes | No | No |
| Tasks: create, edit, move, note | Yes | Yes | Yes |
| Tasks: delete | Yes | Yes | No |
| Chat and meetings | As today | As today | As today |

## What people can do (first version)

1. Tom opens Documents from More on his phone. He sees two rows: "Shared" and
   "Shared with you".
2. He adds a photo of a receipt to Shared. Ana sees it in Shared too.
3. He renames his own receipt. Leo's file in Shared has no rename or delete
   for him.
4. A saved link to the Company folder shows "Only owners and admins can see
   this folder."
5. Ana names Tom on the courier login. It shows under "Shared with you". He
   reveals it after re-entering his password. Ana gets the reveal email.
6. Leo opens Members. He sees names and roles, and his own email only.
7. Tom opens a task. There is no Delete button. Maya can delete it.
8. Tom posts a photo in #general. Everyone in #general sees it, as today.

## Not in this version

- Sharing any other folder, or a single file, with members.
- More than one shared folder, or a shared folder for some members only.
- Members creating folders or moving files between folders.
- Private meetings, or editing and deleting meetings.
- Owners seeing or revoking other people's Claude connections.
- A custom role, or a setting for each permission.

## Done when

- Tom, on a 390px phone, sees only Shared and "Shared with you" in Documents.
  Every other `/documents` address returns 403 for him.
- Tom adds, opens, renames and deletes his own file in Shared. He gets 403
  renaming or deleting Leo's file there.
- Maya renames and deletes any file in Shared.
- Nobody can rename or delete Shared, or rename Company to "Shared".
- Tom reveals the courier login he is named on. He can't open any other login.
- Leo's Members page shows no email except his own.
- Tom gets 403 deleting a task. Maya deletes it.
- Tom opens a #general photo and deletes his own, from chat.
- Ana and Maya see no change, except the task delete rule and the new folder.

## Risks and open questions

- Every Documents route needs the gate, not just the menu. One check (for
  example "manager, or this is the Shared folder"), used by every folder and
  file route. Add a test that walks every route, like
  `tests/requests/stale_tab.rs`.
- Folders are found by name. Before the deploy, the plan checks production
  for a folder already called "Shared". If one exists, members would see its
  files. The plan must decide how to handle it, for example renaming the old
  one to "Shared (old)" first.
- Members can now upload, so the Shared folder can fill up. Uploads already
  have a 25 MB cap. Unverified: R2 cost stays in the free tier.
- When an admin becomes a member, they lose the rest of Documents on their
  next request. `can_reveal` checks the role every time. Unverified: open
  pages show a clean refusal.
- Unverified: no other page shows emails, such as task or meeting pickers.
  The plan should search for `email` in `assets/views/`.
- No migration. Rolling back restores today's access. The Shared folder and
  its files stay, as an ordinary folder.

Out of scope here: close-guard makes Claude's `create_project`
managers-only.
