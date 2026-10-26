# Member access: what a plain member can reach

Status: Draft, for review · One PR, after close-guard · Owner: Raj · 26 October 2026

Brief: `docs/briefs/member-access.md` (approved). This plan adopts the staff
review of the brief. Where they differ, the review wins: Shared is found by a
new `folders.kind` column, not by name, so there is now a small migration.

## Review changes

The security review changed these points. They win over the text below.

1. **Shared is made by the migration, not on first GET.** The migration
   renames any ordinary folder whose `lower(name) = 'shared'` to "Shared
   (old)", or "Shared (old 2)", "Shared (old 3)" and so on if that name is
   taken, so `idx-folders-org-name` can't fail. Then it inserts one Shared
   folder (`kind = 'shared'`) for every organisation. `create_starters`
   makes it for new organisations. At runtime `shared()` only looks it up by
   kind. If it is somehow missing, it makes it inside a transaction holding
   `pg_advisory_xact_lock(org)`, and looks again inside it first. It never
   renames anything. If the insert fails (an ordinary "Shared" holds the
   name), it logs the error and members see no Shared row. The Documents
   page never 500s because of it. Down drops the index and the column. The
   renames are not undone, and the Shared folders stay as ordinary folders.
2. **`unlock`.** A member who sends no `login_id` is refused. A login id
   from another organisation gets the same 403 as a login they can't see.
3. **`logins::show`.** The gate runs before `login_data`, on both the HTMX
   sheet and the full page. The edit `form` (folder names, people picker)
   is built only for managers.
4. **`rename_file`.** Keeps the `can_delete` check after the gate. Test:
   another person's file outside Shared gives a member 403.
5. **Refusals never name things.** The 403 page and toast never show the
   folder, file or login name. Tests cover folders and logins.
6. **`tasks::destroy`** also calls `require_own_membership`, so a super
   admin acting in an organisation can't delete tasks.

Also: members lose access to their own past uploads outside Shared. The PR
must say so. `copy` is already gated through `can_reveal`; it only needs a
test.

## Summary

Members (`CurrentMember::can_manage()` false) see two rows in Documents:
"Shared" and "Shared with you". Every other folder, file and login is owners
and admins only. In Shared, members open and add files, and rename or delete
only their own. Members see only their own email on Members. Only owners and
admins delete tasks, in the app and through Claude.

One gate for Documents: `folders::Model::open_to(can_manage)`, true for a
manager or for the Shared folder. Every folder and file route calls it.

## What the code does today

- `folders` has no kind. `is_logins` (`src/models/folders.rs:105`) matches
  the name "Logins". `chat_photos` (`:207`) finds or makes "Chat photos" and
  recovers from the unique name index `idx-folders-org-name` on
  `(organisation_id, lower(name))`. `create_starters` (`:113`) inserts
  `STARTERS` (`:24`): Company and Logins.
- `src/controllers/documents.rs`: `show_folder` (`:265`), `upload` (`:436`),
  `open_file` (`:480`) and `rename_file` (`:560`) check only
  `require_own_membership`. `delete_file` (`:582`) checks
  `documents::Model::can_delete` (manager or uploader). `edit_folder`
  (`:303`) and `delete_folder` (`:358`) are `require_manager`, with no
  built-in check, so Logins can be renamed today (and then `is_logins` stops
  matching).
- `folder_rows` (`:121`) lists every folder. It feeds `index`, `show_folder`
  and `logins::show`.
- Logins can move between folders through the edit form
  (`logins::Model::edit`, `src/models/logins.rs:336`), into any folder.
  `create` (`:257`) takes any folder.
- `src/controllers/logins.rs`: `show` (`:201`) has no access check: any
  member reads title, username, URL and notes. `reveal` and `copy` both go
  through `prepare_secret` (`:476`), which calls `can_reveal` (`:486`;
  manager or `login_accesses::is_named`, browser session only). So copy is
  already covered, like reveal; the review's item 5 only needs a test for
  it. `unlock` (`:385`) has no login check.
- `MemberRow::new` (`src/controllers/members.rs:41`) sets `email` for every
  row (`:57`). `_row.html:6` prints it. Pending rows are managers only
  (`:153`). No other page or JSON sends another person's email
  (`grep '\.email' src/controllers src/mcp src/views`: only auth, join and
  the super-admin page).
- `tasks::destroy` (`src/controllers/tasks.rs:934`) has no role check.
  `tasks/_sheet.html:44` always shows "Delete task".
- `delete_task_for` (`src/mcp/mod.rs:920`) lets any member's connection
  delete tasks it made in the last 24 hours. Description at `:622`.
- Claude has no documents or logins tools. `WRITE_TOOLS` and the tool list
  are projects, tasks, notes and members only, and
  `tests/requests/mcp.rs:307` fails if any tool mentions login, password,
  secret, document, folder or file. No change.
- Every file download goes through `send_stored` (`documents.rs:505`):
  `open_file` and chat's `open_photo` (`chat.rs:862`, `:869`). Nothing else
  calls `download_stream`.
- Chat's photo delete uses `file.can_delete` in `photo_file_allows`
  (`chat.rs:935`). That method stays as it is, so chat is unchanged.
- A forbidden full-page GET today returns loco's JSON error. There is no
  403 page. HTMX 4xx responses don't swap (`responseHandling` in
  `layouts/base.html:13`), but `HX-Trigger` toasts still fire, which is how
  `refuse` (422) works now.

## Migration

`migration/src/m20261027_090000_add_kind_to_folders.rs`, generated, then:

```sql
SET LOCAL lock_timeout = '5s';
ALTER TABLE folders ADD COLUMN kind text NULL;
CREATE UNIQUE INDEX "idx-folders-org-shared" ON folders (organisation_id)
  WHERE kind = 'shared';
```

Then `backfill_sql()`. Down: `SET LOCAL lock_timeout = '5s'; DROP INDEX
"idx-folders-org-shared"; ALTER TABLE folders DROP COLUMN kind;`

- **What it does.** Adds the `kind` column and the index. Re-marks a
  Shared left by an earlier down (named "Shared", with our note, at most
  one per org because of the name index), so members keep its files.
  Renames any other folder called "Shared" to "Shared (old)", or "Shared
  (old N)" if that is taken. Inserts one Shared per org that has none.
  Holds a lock on `folders` briefly; `lock_timeout` makes it give up
  after 5 seconds rather than queue behind a long query.
- **Rollback.** Reversible. Down drops the index and the column and keeps
  the data: each Shared stays as an ordinary folder called "Shared", which
  the old code shows to everyone (today's access), and renamed folders keep their new
  names. Running up again re-marks the same Shared folders.
- Regenerate `src/models/_entities/folders.rs` (`kind: Option<String>`).

## What changes, file by file

### 1. `src/models/folders.rs`

- `pub const SHARED: &str = "Shared";` and `pub mod kind { pub const
  SHARED: &str = "shared"; }`.
- `is_shared(&self) -> bool`: `self.kind.as_deref() == Some(kind::SHARED)`.
  No name rule. Renaming Company to "Shared" is already refused by the
  unique name index, and grants nothing anyway.
- `open_to(&self, can_manage: bool) -> bool`: `can_manage || is_shared()`.
  The one gate, with a doc comment.
- `is_built_in(&self) -> bool`: `is_shared() || is_logins()`.
- `shared(db, org_id) -> ModelResult<Self>`, like `chat_photos`: find by
  `kind = 'shared'`; else, inside a transaction holding an advisory lock
  for the org, look again and insert `{name: "Shared", note: SHARED_NOTE,
  kind: "shared"}` (`tracing::warn!`). It never renames. If an ordinary
  folder holds the name, the insert fails; `folder_rows` logs the error
  and members see no Shared row.
  `SHARED_NOTE`: "Photos and files for the whole team. Everyone can open
  them and add to them."
- `STARTERS` becomes `[(name, note, kind); 3]` with Shared added.
  `create_starters` sets `kind`.
- `edit`: when `is_built_in()` and the cleaned name differs from
  `self.name`, `field_error("name", "Built-in folders keep their name.")`.
  The note can still change.
- `delete_if_empty`: when `is_built_in()`, `ModelError::msg("The {name}
  folder is built in, so it can't be deleted.")`. `delete_folder` already
  maps `Message` to a toast.
- Module doc: members see only Shared.

### 2. `src/models/login_accesses.rs`

`logins_for(db, org_id, user_id) -> ModelResult<Vec<logins::Model>>`:
`logins::Entity::find().in_tenant(org_id)`, `Id.in_subquery(SELECT
login_id FROM login_accesses WHERE user_id = ?)`, by title. Feeds the
"Shared with you" row and page.

### 3. `src/models/logins.rs`

- `create` and `edit`: refuse a Shared target folder with
  `field_error("folder_id", "Logins can't go in Shared. Choose another
  folder.")`, as `upload` refuses files in Logins. In `edit`, check the
  folder found at `:338`.
- `can_see(&self, db, member) -> ModelResult<bool>`: same org, and manager
  or `login_accesses::is_named`. Unlike `can_reveal`, no session rule, so
  the page answers the same in every browser.

### 4. `src/controllers/documents.rs`

- **Gate helper.** `fn no_entry(v, member, headers, message) ->
  Result<Response>`: an HTMX request gets 403, `HX-Reswap: none` and an
  error toast. A page load gets 403 with `errors/forbidden.html`. Every
  refusal below goes through it.
- `show_folder`: after `find_in_org`, `if !folder.open_to(can_manage)`
  → "Only owners and admins can see this folder." Skip `login_form` when
  `is_shared()`.
- `upload`: same gate and copy, before reading the body. Logins check
  stays.
- `open_file`, `rename_file`, `delete_file`: load the file, then its
  folder, then the gate. In `delete_file` the folder load moves above the
  `can_delete` check.
- `rename_file` and `delete_file`: after the gate, `file.can_delete(user,
  can_manage)` (manager or uploader; a NULL uploader means managers only)
  → "Only the person who added a file, owners and admins can change it."
  So a member's delete outside Shared is 403, including a chat photo
  opened from Documents. Chat routes don't change.
- `edit_folder`, `delete_folder`: unchanged apart from the model checks.
- `folder_rows(ctx, member, selected: Selected)`, where `Selected` is
  `None | Folder(i64) | SharedWithYou`. Calls `folders::Model::shared`
  first (first use). Managers: every folder, as today. Members: only the
  Shared row, then a "Shared with you" row from `logins_for` ("2 logins",
  or "None yet"). `FolderRow` gains `href`; `_folders.html` uses it.
- New route `GET /documents/shared-with-you` (`shared_with_you`).
  Managers are redirected to `/documents`. Members get `folder.html` with
  a virtual folder `{name: "Shared with you", is_logins: true,
  shared_with_you: true}` and the `logins_for` rows. No upload, settings
  or login form render, because those are already behind `not
  is_logins` or `can_manage`.
- `files_data`: adds `is_shared`; `can_delete` on each row becomes
  `folder.open_to(..) && file.can_delete(..)`, and also gates the Rename
  button.

### 5. `src/controllers/logins.rs`

- `show`: `if !login.can_see(..)` → `no_entry`, "Only owners, admins and
  people named on this login can see it." For members, `login_data` sets
  `folder` to `{href: /documents/shared-with-you, name: "Shared with
  you"}` and selects `SharedWithYou` in the sidebar. `login.html` uses
  `folder.href`. `form` is built for managers only.
- `login_data`: `folders_list` leaves out Shared.
- `copy`: no change. It goes through `prepare_secret`, so it is
  manager-or-named already. Add a test.
- `unlock`: for a non-manager, the `login_id` must be a login they
  `can_see`, else 403 "Only owners, admins and people named on this login
  can see it." This runs before `begin_reauth`, so no password is checked
  and nothing is counted. The front end always sends `login_id`
  (`frontend/app.ts:2792`). Managers as today.
- `create`: the model refusal covers a crafted post into Shared.

### 6. `src/controllers/members.rs`

`MemberRow.email: Option<String>`. `new` sets it only when
`me.can_manage() || is_me`. `_row.html:6`: `{% if row.email %}`. Pending
rows are managers only already.

### 7. Tasks

- `tasks::destroy` (`tasks.rs:934`): `member.require_manager()` first.
  The JSON 403 carries "Only owners and admins can delete tasks." Add an
  `htmx:responseError` case in `frontend/app.ts` for `[data-task-delete]`
  that toasts `refusalText(xhr)`.
- `tasks/_sheet.html:44`: wrap the Delete block in `{% if me.can_manage
  %}` and add `data-task-delete`.
- `src/mcp/mod.rs`: `delete_task_for` (`:920`) refuses non-managers before
  `remove_made_by`: "Only owners and admins can delete tasks. Ask one of
  them, or mark it done instead." Description (`:622`): "Owners and admins
  only. Delete a task this connection created in the last 24 hours, …".
  Keep it free of the words banned by `tests/requests/mcp.rs:307`.

### 8. Templates and front end

- `errors/forbidden.html`, modelled on `errors/not_found.html`: "403",
  heading "You can't open this", the message, and "Back to Documents".
- `documents/index.html`: the member line becomes "Owners and admins look
  after the other folders. Add photos and files to Shared for everyone."
- `documents/_folder_form.html`: `readonly` name and the hint "Built-in
  folders keep their name." when built in. Hint for other folders: "Only
  owners and admins can see this folder." For Shared: "Everyone in {org}
  can see this folder and add to it."
- `documents/folder.html`: no Delete folder form for built-in folders. The
  Add sheet's line "Everyone in {org} can open what you add." shows only
  in Shared. Elsewhere it reads "Only owners and admins can open what you
  add here."
- `documents/_files.html`: empty states. Shared: "Nothing in Shared yet" /
  "Add receipts, photos and papers for the whole team. Everyone in {org}
  can open them." Shared with you: "No logins shared with you" / "When an
  owner or admin names you on a login, it shows here." Other folders: drop
  "Everyone in {org} can open it." The can't-delete line becomes "Only the
  person who added a file, owners and admins can rename or delete it."
- `documents/_login_form.html`: show `errors.folder_id`.
- `frontend/app.ts` `uploadFinished` (`:1881`): a 403 says "Only owners
  and admins can add to this folder."

## Copy for refusals

| Where | Text |
|---|---|
| Folder page or file link (page) | "Only owners and admins can see this folder." |
| Upload, stale tab (row) | "Only owners and admins can add to this folder." |
| Rename or delete someone else's file (toast) | "Only the person who added a file, owners and admins can change it." |
| Login page, sheet or unlock | "Only owners, admins and people named on this login can see it." |
| Rename a built-in folder (field) | "Built-in folders keep their name." |
| Delete a built-in folder (toast) | "The Shared folder is built in, so it can't be deleted." |
| Login into Shared (field) | "Logins can't go in Shared. Choose another folder." |
| Task delete (toast, Claude) | "Only owners and admins can delete tasks." |

## What members see in the nav

The Documents link in the sidebar (`layouts/app.html:20`) and More
(`more/index.html:33`) stays. It lands on `/documents`, which lists Shared
and "Shared with you". Login activity stays owners only. Managers see every
folder, now including Shared.

## Edge cases

- **Admin made a member mid-session.** The next request is gated. An open
  folder's rename or delete gets a 403 toast; a reload shows the 403 page.
- **NULL uploader in Shared** (a former member's file): managers only.
- **Chat photos**: hidden from members in Documents. Opened and deleted
  through chat as today.
- **Old "Shared" folder** in production: renamed "Shared (old)" by the
  migration. Before deploy, run `SELECT organisation_id, id FROM folders WHERE
  lower(name) = 'shared'`. Unverified: expected none.
- **Super admin acting**: `require_own_membership` first, as today.

- **Missing and unnamed logins.** `show`, `reveal` and `copy` return 404
  for an id that doesn't exist or is in another org, and 403 for a login
  in this org the member isn't named on. So a member can tell that an id
  exists in their own org. Accepted: ids are not secret, and nothing about
  the login is shown.

## Tests

### Model (`tests/models/folders.rs`, new)

- `shared` makes one folder; a second call returns it; two at once give
  one row.
- Backfill: an ordinary "Shared" becomes "Shared (old)" (or "Shared (old
  N)") with `kind` NULL; each org gets one Shared; running it twice
  changes nothing. After a down, it re-marks the same Shared folder.
- `create_starters` makes Company, Logins and Shared.
- `edit` keeps built-in names (note still changes); `delete_if_empty`
  refuses both built-in folders, even when empty.
- `logins::create` and `edit` into Shared: field error, nothing saved.
- `login_accesses::logins_for` returns only this org's logins naming the
  user.

### Requests

- `tests/requests/member_access.rs` (new): the route walk. Collect every
  `/documents` route from `App::routes(ctx)`, as `stale_tab.rs` does, and
  assert the table covers each one, so a new route fails the test until it
  is listed. For member, admin and owner, call each route against a Shared
  folder and file, a Company folder and file, a Logins login (named and not
  named), and the "Shared with you" page. Expected:

  | Route | Member | Admin | Owner |
  |---|---|---|---|
  | `GET /documents` | 200, two rows | 200, all | 200, all |
  | `GET /documents/shared-with-you` | 200 | 303 | 303 |
  | `GET /documents/activity` | 403 | 403 | 200 |
  | `POST /documents/folders` | 403 | 303 | 303 |
  | `GET /folders/{shared}` / `{company}` | 200 / 403 | 200 / 200 | 200 / 200 |
  | `POST /folders/{id}`, `/delete` | 403 | built-in: refused | built-in: refused |
  | `POST /folders/{shared}/files` / `{company}` | 200 / 403 | 200 / 200 | 200 / 200 |
  | `GET /files/{shared}` / `{company}` | 200 / 403 | 200 | 200 |
  | `POST /files/{own in shared}/rename`, `/delete` | 200 | 200 | 200 |
  | `POST /files/{other's in shared}/rename`, `/delete` | 403 | 200 | 200 |
  | `POST /files/{company}/rename`, `/delete` | 403 | 200 | 200 |
  | `POST /folders/{id}/logins` | 403 | 303 (not Shared) | 303 |
  | `GET /logins/{named}` / `{other}` | 200 / 403 | 200 | 200 |
  | `POST /logins/{id}`, `/delete` | 403 | 303 | 303 |
  | `POST /logins/unlock` with other's id | 403 | 200 | 200 |
  | `POST /logins/{other}/reveal`, `/copy` | 403 | 200 | 200 |
  | `POST /logins/{named}/reveal`, `/copy` | 200 | 200 | 200 |

  Plus: a member's 403 on deleting their own #general photo via
  `/documents/files/{id}/delete`, then a 200 deleting it through chat's
  `delete_photo`. A 403 page load renders "Only owners and admins can see
  this folder." and no file name.
- `tests/requests/documents.rs`: rewrite
  `a_member_adds_a_photo_and_everyone_can_open_it` for Shared. Update
  `new_organisations_start_with_company_and_logins_folders` and
  `only_the_uploader_owners_and_admins_delete_files`. Built-in folders
  can't be renamed or deleted.
- `tests/requests/logins.rs`: `only_owners_admins_and_named_members_can_see_secrets`
  now expects bob's page and unlock to be 403.
- `tests/requests/members.rs`: a member sees their own email only; an
  admin sees all.
- `tests/requests/tasks.rs`: a member gets 403 deleting and the sheet has
  no Delete; an admin deletes.
- `tests/requests/mcp_orgs.rs` `delete_task_rules`: a member's connection
  is refused with the new text; an admin's still deletes its own recent
  task.
- Check helpers that pick "the first non-Logins folder"
  (`bare_addresses.rs:145`, `links.rs:241`) still pick the folder they
  mean, now that Shared exists.

## Commits

1. Migration, entity, `folders` (`kind`, `shared`, built-ins), logins kept
   out of Shared, model tests.
2. Documents gate, `folder_rows`, "Shared with you", 403 page, templates,
   route walk.
3. Logins `show` and `unlock` gate.
4. Member emails.
5. Task delete: web, sheet, MCP.

## Risks

- **A missed route.** The route walk fails on any unlisted `/documents`
  route.
- **Merge with close-guard.** It edits `src/mcp/mod.rs` (makes
  `create_project_for` at `:714` and `delete_project_for` at `:943`
  managers only, with new descriptions), `notifications.rs`, `roadmap.rs`
  and `tests/requests/mcp*.rs`. This PR edits `delete_task_for` (`:920`)
  and its description (`:622`), next to `delete_project_for`, and adds
  tests to `mcp_orgs.rs`. Rebase onto main after close-guard merges, and
  use the same refusal style. No overlap in `notifications.rs` or
  `roadmap.rs`.
- **Shared fills up.** 25 MB cap per file. Unverified: R2 stays in the free
  tier.
- **403 toast on HTMX.** Relies on htmx firing `HX-Trigger` on a 403, as it
  does for the 422 `refuse`. Check by hand.
- **Rollback then roll forward** needs the manual `UPDATE` above.

## Checks

```sh
bin/build-assets
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

By hand, locally, Juniper Tea Co: Ana (owner), Maya (admin), Tom and Leo
(members). Shared holds Tom's receipt photo and Leo's PDF. Company holds a
lease. Ana names Tom on the courier login.

- Tom: Documents shows Shared and "Shared with you". He adds, renames and
  deletes his own file. Leo's file has no Rename or Delete. The Company
  link shows the 403 page. He reveals the courier login.
- Leo: Members shows his own email only.
- Maya: every folder, renames and deletes Leo's file in Shared, deletes a
  task. Shared and Logins can't be renamed or deleted.

### Screenshots needed

On the `pr-screenshots` branch under `member-access/`. Desktop 1440px and
phone 390px, light and dark (the Documents list changes).

- Tom's Documents (Shared and "Shared with you").
- Tom in Shared, with his file's menu (Rename, Delete) and Leo's (neither).
- Tom's "Shared with you" with the courier login, and its empty state.
- The 403 page for the Company folder.
- Leo's Members page without other emails.
- Tom's task sheet with no Delete.
- Maya's Documents and task sheet, unchanged apart from the Shared folder.
