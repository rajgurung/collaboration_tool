# Claude connector for people in more than one organisation

Status: Draft, for Raj and the reviewers · One PR · Owner: Raj · 22 October 2026

No brief. This plan follows an incident. Raj's Claude used the Collab MCP
connector and created 9 projects and 37 tasks for "Softly" inside Himalayan
Ritual. The token was bound to Himalayan Ritual at consent, and nothing in
any tool result said so. There are no delete tools and no project delete in
the app, so the cleanup was a hand-run SQL delete on production.

## Summary

Four changes, in priority order. All in one PR.

1. **Show the org.** The server's `instructions` start with "Connected to
   Himalayan Ritual (himalayan-ritual) as raj." Every tool result carries
   `"organisation": {"name", "slug"}`, list tools included.
2. **Writes name their target.** Every write tool takes a required
   `organisation` slug. A mismatch is refused before anything is read or
   written: "This connection is Himalayan Ritual (himalayan-ritual), not
   softly." Each write tool's description names the connected slug.
3. **Undo without Railway.** Projects and tasks record the connection that
   made them (`claude_grant_id`, one migration). Two new tools,
   `delete_task` and `delete_project`, delete only what the same connection
   made in the last 24 hours. Owners and admins can delete a project in the
   web app.
4. **Smaller wins.** Recommended in: the organisation picker on the Allow
   page (already designed in `docs/designs/org-in-the-address.html` §6), a
   rate brake on bulk creates, a "Made with Claude" line, docs for one named
   connection per org. Recommended later: holding notifications.

One migration, additive and reversible. No new crate.

## What the code does today

- **The token is bound to one org at consent.** `authorize_page`
  (`src/controllers/oauth.rs:285`) uses `PinnedMember`, which picks the last
  used org. The comment at `:289-290` says "PR 3 adds a picker". The page
  pins that org in a hidden `org_id` (`:335`). `authorize` (`:343`) refuses
  if the org changed (`:366`) and issues the code for `member.org.id`
  (`:369-380`).
- **The bearer gate loads one member.** `require_bearer`
  (`src/extractors/bearer.rs:19`) finds the token row (`:53`), checks the
  membership in the token's org (`:58-62`), and puts a `CurrentMember` in the
  request (`:33`). Nothing about the token row itself reaches the tools.
- **The server says nothing about the org.** `INSTRUCTIONS`
  (`src/mcp/mod.rs:30-35`) is a fixed string. `get_info` (`:586-595`) takes
  no request. Tool descriptions are fixed in the `#[tool]` attributes
  (`:287-379`). Results are bare: `{"projects": [...]}` (`:403`), task objects
  (`:249-258`).
- **The org is never named in errors.** A missing id says "Not found in your
  organisation." (`src/mcp/mod.rs:168`).
- **Eight tools, four of them writes:** `create_project` (`:302`),
  `create_task` (`:349`), `update_task` (`:361`), `add_task_note` (`:373`).
  No delete. `docs/notes/claude-connector.md:27` says "and no delete".
- **Access tokens rotate every hour.** A refresh revokes the old row and
  inserts a new one with the same `grant_id` (`src/models/access_tokens.rs:295-345`,
  `issue` at `:567`). Revoked rows are deleted 30 days later (`prune`,
  `:529`). A personal token gets its own `grant_id` (`:182`). A "connection"
  on the settings page is a grant, not a row (`:85-86`).
- **The server is stateless and never streams.** GET answers 405
  (`src/controllers/mcp.rs:1-3`). So it can't send
  `notifications/tools/list_changed`. Clients see a new tool list when they
  next list tools.
- **Claude Code shows the server's `instructions` to the model.** The
  current text appears verbatim in this session's system prompt under
  "MCP Server Instructions" for `collab`. So a changed `instructions` string
  is a real signal, not a hidden field.
- **Deleting a project deletes its tasks.** `tasks.project_id` was created
  NOT NULL, so Loco gave it `ON DELETE CASCADE`
  (`loco-rs` 1.2.0 `schema.rs:698`). Making it nullable later
  (`m20261008_165224`) kept the cascade. Checked on the dev database:
  `fk-tasks-project_id-to-projects ... ON DELETE CASCADE`. Task notes, task
  assignees and note reactions cascade from tasks and notes. Nothing else
  references `projects`, `tasks` or `task_notes`.
- **Notifications point at items by link text only.** Task notices store
  `/tasks?open={id}` (`src/controllers/notifications.rs:105`, `:176`).
  Deleting a task leaves those notices with a dead link.
- **Any member can delete any task in the web app.** `destroy`
  (`src/controllers/tasks.rs:835`) has no role check. There is no project
  delete (`src/controllers/roadmap.rs:293-301`).
- **Org slugs never change.** Only `organisations::create` sets one
  (`src/models/organisations.rs:292`). Names can be 2 to 80 characters
  (`:70-75`).

## What changes, file by file

### 1. Show the org

**The member and the connection reach every handler.**
`src/extractors/bearer.rs`: next to the `CurrentMember` (`:33`), insert a
new `McpConnection { grant_id: Uuid }` extension, taken from the token row.
New small struct in `src/extractors/bearer.rs`. It never goes into a tool
result.

**Per-request `instructions`.** `src/mcp/mod.rs`. rmcp 3.5.1 passes
`RequestContext` to `initialize` and `discover` (`rmcp/src/handler/server.rs:317`,
`:392`), and the HTTP transport puts the request `Parts` in
`context.extensions` (`streamable_http_server/tower.rs:1544`). So:

- Override `initialize`: `context.peer.set_peer_info(request.clone())`, then
  `negotiate_initialize`, then replace `instructions` with the member's
  version. Override `discover` the same way.
- New `fn instructions(member) -> String`:
  "Connected to {org name} ({slug}) as {username}. Every tool reads and
  writes only there. Write tools need `organisation: \"{slug}\"`. If the user
  means another organisation, stop and tell them this connection can't reach
  it; they can add a second connection for it." Then today's text.
- `get_info` keeps the fixed text as the fallback when no member is present
  (it never is on `/mcp`, but the trait needs it).
- First step for the engineer: a test that `initialize` sees the `Parts`.
  If rmcp's stateless path doesn't attach them for `initialize`, stop and
  report. Don't work around it.

**`organisation` in every result.** Change `finish(outcome)` (`:191`) to
`finish(parts, outcome)`. On success, when the value is a JSON object, it
inserts `"organisation": {"name": ..., "slug": ...}` from the member. All
eight tools (and the two new ones) go through `finish`, so one change covers
them. Arrays are always wrapped in an object today (`{"projects": ...}`,
`{"tasks": ..., "more": ...}`, `{"members": ...}`), so nothing is missed.

**Errors name the org.** The not-found text (`:168`) becomes "Not found in
{org name}." `From<ModelError>` has no member, so `finish` rewrites that one
message. Other tool errors stay as they are.

### 2. Writes name their target

**The parameter.** Each write input struct (`CreateProject` `:66`,
`CreateTask` `:102`, `UpdateTask` `:118`, `AddTaskNote` `:140`, and the new
`DeleteTask`, `DeleteProject`) gets:

```rust
#[schemars(required, description = "Slug of the organisation this change is for. It must be the connected one, named in this tool's description.")]
pub organisation: Option<String>,
```

`Option` in Rust, required in the schema (schemars 1.2.2 supports
`#[schemars(required)]`). So a client with the new tool list must send it,
and a client with an old cached list gets a clear tool error instead of a
protocol error (see "Existing connections" below).

**The check.** New `fn check_org(member, given: Option<&str>) -> Result<(), Fail>`
in `src/mcp/mod.rs`, called first in every write `*_for`, before
`Team::load` or any id lookup:

- Missing or blank: `Fail::Refused("Say which organisation this is for.
  This connection is {name} ({slug}). Check with the user that {name} is
  where they mean, then repeat the call with organisation: \"{slug}\".")`
- Given, after `trim()` and ASCII lowercase, equal to `member.org.slug`: ok.
- Anything else: `Fail::Refused("This connection is {name} ({slug}), not
  {given}. Nothing was changed. To work in {given}, the user needs a
  connection for that organisation.")`. `{given}` is cut to 60 characters and
  stripped of control characters.

The check can only refuse. It never picks an org. The token's org stays the
only org any tool can reach.

**Slug in the descriptions.** Override `list_tools` on `CollabServer`
(`rmcp-macros` `tool_handler.rs:64` skips generating it when the impl has
one). It takes `self.tool_router.list_all()`, reads the member from
`context.extensions`, and for each write tool appends " Connected to {name}
({slug}): pass organisation \"{slug}\"." to the description. Read tools are
unchanged. `call_tool` and `get_tool` stay generated.

**Existing connections.**

- Raj's Claude Code and a friend's connection keep working. Tokens don't
  change.
- Claude Code lists tools when a session starts or the server reconnects.
  Our server can't push a list change. A session already open when we deploy
  keeps the old schema until it restarts or runs `/mcp` reconnect. claude.ai's
  refresh timing is unverified; assume per conversation.
- In that window, an old-schema write without `organisation` gets the
  "Say which organisation" tool error. Claude reads it and retries with the
  slug. Nothing is written by the first call.
- Honest limit: Claude can copy the slug from the error or description
  without asking. The parameter is a speed bump that puts the org name in
  front of the model at the moment it writes. The real fixes are the
  picker (section 4a) and one connection per org (4d).

### 3. Undo without Railway

#### 3a. Migration: `claude_grant_id` on projects and tasks

New `migration/src/m20261023_090000_add_claude_grant_to_projects_and_tasks.rs`,
registered in `migration/src/lib.rs`. Same shape as
`m20261022_120000_add_photo_to_messages.rs`:

```sql
SET LOCAL lock_timeout = '5s';
ALTER TABLE projects ADD COLUMN claude_grant_id uuid NULL;
ALTER TABLE tasks ADD COLUMN claude_grant_id uuid NULL;
CREATE INDEX "idx-projects-claude_grant" ON projects (claude_grant_id, created_at) WHERE claude_grant_id IS NOT NULL;
CREATE INDEX "idx-tasks-claude_grant" ON tasks (claude_grant_id, created_at) WHERE claude_grant_id IS NOT NULL;
```

Down: `ALTER TABLE projects DROP COLUMN claude_grant_id; ALTER TABLE tasks
DROP COLUMN claude_grant_id;` (the indexes go with the columns).

**Why the grant, not the access token id.** An OAuth access token row lasts
an hour; the next refresh makes a new row (`access_tokens.rs:567`). Keyed on
the row id, "delete what this connection made" would stop working after an
hour. The grant id stays the same for the life of the connection, across
refreshes. It also covers personal tokens (one grant each).

**Why no foreign key.** Token rows are deleted 30 days after they are revoked
(`prune`, `access_tokens.rs:529`). A key would either cascade (deleting
Claude-made projects) or fail the prune. A bare uuid outlives the token,
which is what a "made with Claude" record should do.

**No separate `source` column.** A non-null `claude_grant_id` means "made
with Claude". Web rows stay NULL. Existing rows stay NULL: we can't tell
which old rows Claude made.

**What it does in production.** Two `ADD COLUMN ... NULL` with no default:
catalog-only, no rewrite, a brief `ACCESS EXCLUSIVE` lock on each table. Two
partial indexes over zero matching rows: near instant, but `CREATE INDEX`
blocks writes to the table while it builds. `lock_timeout` makes the
migration fail fast if a long query holds the table; the deploy fails
and can be retried. Rolls back cleanly (see "Rollback").

**Entities.** Regenerate `src/models/_entities/projects.rs` and
`src/models/_entities/tasks.rs` (`cargo loco db entities`), or add the one
`Option<Uuid>` field by hand to match.

#### 3b. Recording it

- `projects::Model::create` (`src/models/projects.rs:136`) and
  `tasks::Model::create` (`src/models/tasks.rs:154`) take a new last argument,
  `claude_grant: Option<Uuid>`, and set the column. An argument, not a field
  on `ProjectParams` or `TaskParams`: those are deserialized from web forms,
  and a form must never be able to set it.
- Callers: `src/controllers/roadmap.rs:209` and `src/controllers/tasks.rs:744`
  pass `None`. `src/mcp/mod.rs:425` and `:528` pass the `McpConnection`
  grant.
- `update_task` doesn't touch it. A human-made task edited by Claude is
  still human-made.

#### 3c. `delete_task` and `delete_project` tools

Both: `annotations(destructive_hint = true, open_world_hint = false)`, the
`organisation` parameter, and the slug in the description.

**`delete_task { organisation, task_id }`.** Description: "Delete a task
this connection created in the last 24 hours, to undo a mistake. Its notes
and assignees go with it. Anything else must be deleted in Collab Tool."

New `tasks::Model::remove_made_by(db, org_id, task_id, grant, author_id) ->
ModelResult<Removed>` in `src/models/tasks.rs`. One transaction:

1. `find_in_org` with `FOR UPDATE`. Not found: "Not found in {org}."
2. Refuse unless `claude_grant_id == grant` and `created_at > now() - 24
   hours`: "Only tasks this connection created in the last 24 hours can be
   deleted from Claude. Ask the user to delete it in Collab Tool."
3. Refuse when the task has a note by anyone other than the connected
   user: "Someone else has added notes to this task, so it can't be deleted
   from Claude." Their words are not Claude's to throw away.
4. Delete notifications in the org with `link = '/tasks?open={id}'`.
5. Delete the task. Notes, assignees and reactions cascade.

Returns `{"deleted": {"id", "title"}, "organisation": ...}`.

**`delete_project { organisation, project_id }`.** Description: "Delete a
project this connection created in the last 24 hours, with its tasks, to
undo a mistake. Refused if anyone else has added tasks or notes to it."

New `projects::Model::remove_made_by(...)` in `src/models/projects.rs`. One
transaction:

1. `find_in_org` with `FOR UPDATE`. This lock matters: inserting a task into
   the project, or moving one in, takes `FOR KEY SHARE` on the project row
   for the foreign key check. So no one can add a task between our check and
   our delete.
2. Refuse unless the project's `claude_grant_id == grant` and it is under
   24 hours old.
3. Refuse when any task in it has a different (or NULL) `claude_grant_id`:
   "Someone else has tasks in this project, so it can't be deleted from
   Claude. Ask the user to delete it in Collab Tool."
4. Refuse when any of its tasks has a note by someone else (as above).
5. Delete notifications for each of its tasks (`link IN (...)`).
6. Delete the project. Tasks, notes, assignees and reactions cascade.

Returns `{"deleted": {"id", "name", "tasks": n}, "organisation": ...}`.

No notifications are sent for either delete. Nothing like it exists today
for task deletes in the web app.

**Why these limits.** The tools exist to undo Claude's own mistake right
after it happens. Same connection: a second Claude (or a friend's) can't
undo the first's work. 24 hours: covers "I just noticed"; anything older is
a human decision in the web app. No human content: a delete from Claude
must never remove what a person wrote.

#### 3d. Delete a project in the web app

**Hard delete, owners and admins, with a confirm that names the count.**

- Route: `POST /roadmap/projects/{id}/delete` in
  `src/controllers/roadmap.rs` `routes()` (`:293`). One line, like the photo
  route, so the address grep sees it.
- Handler `destroy`: `CurrentMember`, `member.require_manager()?`
  (`src/extractors/current_member.rs:138`), `find_in_org`, then
  `projects::Model::remove(db)` (new): one transaction that deletes the
  notifications for its tasks, then the project. HTMX gets the same trigger
  shape as task delete (`src/controllers/tasks.rs:847-851`): toast "Project
  deleted", `roadmap-changed`, `close-dialogs`. No HTMX: redirect to
  `member.url("/roadmap")`.
- `assets/views/roadmap/_form.html`: when `project_id` is set and
  `can_manage`, a `btn-text-danger` "Delete project" left of Cancel
  (`:47`), with `hx-confirm="Delete “{name}” and its {n} tasks? Their notes
  go with them. This can't be undone."` (`n` = 0 reads "and its 0 tasks";
  the engineer may drop the clause for 0). Same pattern as the task sheet
  (`assets/views/tasks/_sheet.html:43`). `form_data` (`:151`) adds
  `task_count` and `can_manage`.
- What goes: the project, its tasks, their notes, assignees, note reactions,
  and the notifications that link to those tasks. Project-owner notices link
  to a lane (`/roadmap?lane=...`), still valid, and stay.
- Who: owners and admins (`can_manage`). A super admin acting in the org has
  the owner role, as for every other manager action.

**Why not a soft delete with a bin.** A bin means a `deleted_at` on projects
and tasks, a filter on every project and task query (roadmap, tasks, board,
dashboard numbers, MCP lists, notifications), a bin page and a restore. That
is its own PR. The hard delete matches task delete, which already exists
and has no bin. Moving tasks to chores instead of deleting them is the other
small option; it would have left 37 stray chores after the Softly incident.

### 4. Smaller wins

#### 4a. Organisation picker on the Allow page (recommend: in)

The mockup is done and approved: `docs/designs/org-in-the-address.html` §6
(cards with initial, name, "Owner · as raj", starts on last used, only
active orgs, an error at the top if the picked org is gone). It is the root
fix: the incident happened because consent silently took the last-used org.

- `authorize_page` (`src/controllers/oauth.rs:285`): keep `PinnedMember` for
  signed-in and acting checks. Add `memberships::Model::for_user_with_orgs`
  (`src/models/memberships.rs:206`), active only, last used first. One org:
  today's page. Two or more: the radio cards, `name="org_id"`, the pinned org
  checked. The hidden `org_id` field (`:335`) goes when there is a picker.
- `authorize` (`:343`): `org_id` is now a choice, not a pin. Parse it, then
  `memberships::Model::active_in(db, org_id, user.id)` (`:879`). Not active
  (or not a number): re-render the page with "You can't connect Claude to
  that organisation any more." and the list without it. No code is issued.
  Acting super admins still get `acting.html`, as today (`:354`). The code
  is issued for the chosen org (`:373`).
- `exchange_code` already re-checks the membership `FOR SHARE`
  (`access_tokens.rs:218-224`), so a removal between Allow and the exchange
  still wins.
- `assets/views/oauth/authorize.html`: the cards, per the mockup. "Allow"
  and "Deny" stay as they are (mockup question, answered "Allow only").
- `SWITCHED_ORG` (`current_member.rs`) stays for the one-org page.

#### 4b. Rate brake on bulk creates (recommend: in)

In `create_project_for` and `create_task_for`, after `check_org`:

- Count rows with this `claude_grant_id` and `created_at > now() - 60
  seconds` (uses the new partial index).
- 5 projects or 20 tasks already: refuse with "Paused: this connection has
  made 5 projects in {name} ({slug}) in the last minute. Confirm with the
  user that this is the right organisation and that they want more, then
  try again in a minute."
- Constants `PROJECTS_PER_MINUTE = 5`, `TASKS_PER_MINUTE = 20` in
  `src/mcp/mod.rs`.
- Soft: two parallel calls can both pass at the limit. It is a brake for
  mistakes, not a security control. Per connection, so a token refresh
  doesn't reset it.
- The Softly run (9 projects) would have stopped at the sixth with the org
  name in the message.

#### 4c. "Made with Claude" line (recommend: in, two places)

- Task sheet (`assets/views/tasks/_sheet.html`): a muted line "Made with
  Claude" under the details when `task.claude_grant_id` is set.
- Project edit dialog (`assets/views/roadmap/_form.html`): the same line.
- Not on cards, the board or lists. Later if wanted.
- It says how, not who. We don't record which person's Claude, and the
  token row that knows is pruned 30 days after it is revoked.

#### 4d. Docs: one connection per org (recommend: in)

- `README.md:43-44`: for Claude Code, one named connection per org:
  `claude mcp add --transport http collab-softly https://collab.zen-scale.io/mcp`,
  pick Softly on the Allow page; then `collab-himalayan` and pick Himalayan
  Ritual. For claude.ai, one custom connector per org, each named after it.
- Unverified: that Claude Code keeps a separate OAuth token per server name
  for the same URL. The engineer checks by hand with two names and two orgs,
  and writes down what happened.
- `docs/notes/claude-connector.md`: the new tools, the `organisation`
  parameter, the limits on deletes, the brake, the picker. Remove "and no
  delete" (`:27`).

#### 4e. Hold notifications for Claude-made items (recommend: later)

Needs a delay queue (a worker or scheduler job that sends held notices), a
place to keep them, and cancelling on delete. The brake and the deletes
cover the incident. If it comes back, the column from 3a is what the job
would key on.

## Access rules and tenancy

| Action | Who | Scope |
|---|---|---|
| Any tool | Active member of the token's org (unchanged) | Token's org only |
| Write tools | Same, plus `organisation` equals the token's slug | Token's org only |
| `delete_task` | Same | Tasks made by this connection in the last 24 h, no one else's notes |
| `delete_project` | Same | Projects made by this connection in the last 24 h, all tasks by this connection, no one else's notes |
| Web project delete | Owners and admins | `find_in_org`, `CurrentMember` claim check |
| Picker | Signed in, not acting | Only orgs where the person is active; re-checked on Allow and at code exchange |

Every lookup stays `find_in_org(member.org.id, ...)` or `in_tenant`. No tool
takes an org id. The `organisation` slug is compared, never looked up.

## Edge cases

- **Old-schema client, write without `organisation`.** Tool error, nothing
  written. Covered above.
- **`organisation: "Himalayan-Ritual "`.** Trimmed and lowercased: accepted.
- **Org renamed.** The slug doesn't change, so a saved slug keeps working.
  The name in messages follows the rename.
- **A task Claude made, then moved by a person into a human project.**
  `delete_task` still allows it within 24 hours (it is Claude's task). Its
  move is lost with it. Acceptable.
- **A person reassigns or edits a Claude-made task.** `delete_task` still
  allows it. Only notes block it, since notes are a person's own words.
- **A person adds a task to a Claude-made project while Claude deletes it.**
  The `FOR UPDATE` on the project makes one wait. Either the delete sees the
  task and refuses, or the insert fails its key check with not-found.
- **A notification already emailed.** Only mentions are emailed, after 15
  unread minutes (`src/mailers/mentions.rs:65`). The email's link then opens
  Tasks with nothing to open.
- **Two personal tokens for the same person.** Two grants. Each can only
  delete its own. Deliberate.
- **Disconnecting Claude.** The grant is revoked, so no tool works. Rows keep
  their `claude_grant_id` and the "Made with Claude" line.

## Tests

Full suite green. `cargo fmt`, `clippy -D warnings`, `cargo test`.

**Existing tests that change.**

- `tests/requests/mcp.rs`: every write call gains `"organisation": "acme"`
  (or its org). `initialize_and_list_the_eight_tools` (`:260`) becomes ten
  tools, checks the delete tools have `destructiveHint: true`, and that
  `instructions` contains "Connected to Acme (acme)". The forbidden-words
  check (`:300-304`) stays.
- `tests/requests/oauth.rs`: `the_allow_page_pins_the_org` (`:814`) and
  `allow_after_a_switch_is_refused` (`:833`) change meaning for people in two
  orgs (the posted org is a choice). Keep them for one-org people; add the
  picker tests below.

**New: `tests/requests/mcp_orgs.rs`** (add to `tests/requests/mod.rs`).
Setup: Raj active in Himalayan Ritual and Softly, Kreepa only in Himalayan
Ritual. Helper `token_in(ctx, email, slug)` (like `token_for`, `mcp.rs:22`,
but for a named org).

- `initialize_names_the_org`: instructions start with "Connected to
  Himalayan Ritual (himalayan-ritual) as raj".
- `write_tools_name_the_slug_and_require_organisation`: from `tools/list`,
  each write tool's description contains `himalayan-ritual` and its
  `inputSchema.required` contains `organisation`. Read tools don't.
- `every_result_names_the_org`: all ten tools' success results carry
  `organisation == {"name": "Himalayan Ritual", "slug": "himalayan-ritual"}`.
- `a_write_for_another_org_is_refused`: each write tool with
  `organisation: "softly"` is a tool error with exactly "This connection is
  Himalayan Ritual (himalayan-ritual), not softly." at the start. Row counts
  of projects, tasks, notes and notifications unchanged.
- `a_write_without_organisation_is_a_tool_error_not_a_protocol_error`:
  `isError: true`, names the org, nothing written.
- `a_long_or_odd_organisation_is_cut_in_the_message`: 500 characters with a
  newline: the echo is at most 60 characters, no control characters.
- `rows_record_the_connection`: MCP-made project and task have the token's
  `grant_id`; web-made ones (`POST /o/.../roadmap/projects`, `/tasks`) have
  NULL.
- `the_record_survives_a_token_refresh`: OAuth flow (reuse the helpers in
  `tests/requests/oauth.rs`), create a task, refresh, `delete_task` works
  with the new access token.
- `delete_task_rules`: refused for a second personal token's task, a
  web-made task, a task backdated 25 hours, a task with Kreepa's note.
  Allowed with Raj's own note. Success removes notes, assignees, reactions
  and the `/tasks?open={id}` notifications; other notifications stay.
- `delete_project_rules`: refused with one web-made task inside, with a task
  from another connection, with Kreepa's note on a task, after 24 hours.
  Success removes the project and all its tasks, and returns the count.
- `delete_tools_stay_in_their_org`: ids from another org are "Not found in
  Himalayan Ritual." and nothing is deleted.
- `the_brake_stops_the_sixth_project_and_twenty_first_task`: the sixth
  `create_project` in a minute is refused with "Confirm with the user";
  after backdating the five by 61 seconds it works. Same for 21 tasks. A
  second connection has its own count.

**`tests/requests/roadmap.rs`.**

- `owners_and_admins_delete_a_project_with_its_tasks`: owner and admin both;
  tasks, notes, assignees, reactions and task notifications gone; other
  projects untouched; HTMX trigger and non-HTMX redirect.
- `members_cannot_delete_projects`: 403, nothing deleted; the button isn't
  rendered for them.
- `another_orgs_project_is_not_found`: 404, nothing deleted.
- The new route is picked up by the route walk in
  `tests/requests/org_address.rs` (`a_strangers_org_and_a_made_up_slug_look_identical`).
  Confirm it is listed.

**`tests/requests/oauth.rs`.**

- `the_picker_lists_only_active_orgs_last_used_first`: pending, removed and
  declined orgs absent; roles and usernames shown.
- `one_org_shows_no_picker`.
- `allow_binds_the_chosen_org`: cookie on Himalayan Ritual, pick Softly, the
  token works in Softly and `initialize` says Softly.
- `allow_for_an_org_you_left_is_refused`: removed between page and Allow:
  the page again with the error, no code issued.
- `allow_for_an_org_you_never_joined_is_refused`: a forged `org_id`: same
  page, no code, and the response doesn't say whether the org exists.
- `a_super_admin_acting_elsewhere_is_asked_to_leave_first` (`:628`) still
  passes with two orgs.

**Render tests** (`tests/views/render.rs`): the "Made with Claude" line shows
only when the column is set.

## Risks

- **The org name goes into the model's context.** An owner can set an org
  name like "Ignore earlier instructions". The same is already true of task
  titles and notes in tool results. Names are 2 to 80 characters. The
  instructions quote it as a name, in one sentence.
- **Claude may copy the slug without asking.** The parameter is a speed
  bump. The picker and one connection per org are the real guard.
- **A hard delete can't be undone.** Owners and admins only, with the task
  count in the confirm. Claude's deletes are narrow by design.
- **`initialize` override.** If rmcp changes how it negotiates, our override
  could drift. We call its own `negotiate_initialize`, as its docs show.
- **Migration lock.** Covered by `lock_timeout`.

## Rollback

- **Code only (preferred).** Revert the PR without running the down
  migration. The old code doesn't select `claude_grant_id`, and the columns
  are nullable, so old inserts work. The picker goes back to last-used.
- **Schema too.** Then `cargo loco db down` once: drops both columns and
  their indexes. Loses only the "made with Claude" marks. Safe.
- **Deleted data doesn't come back** in either case. That is the point of
  the confirm and the narrow tool rules.

## What the security reviewer must check

1. `check_org` runs first in every write tool, before any lookup, and can
   only refuse. No path takes an org from the input.
2. `claude_grant_id` can't be set from a web form or a tool argument. Only
   from the bearer's `McpConnection`.
3. The grant id never appears in a tool result, page or log line at info
   level.
4. Both delete tools: tenant-scoped, grant-scoped, 24 hours, the notes rule,
   one transaction, and the `FOR UPDATE` that stops a race with a person
   adding a task.
5. Web project delete: `require_manager`, `find_in_org`, POST only, claim
   checked by `CurrentMember`, same response for a stranger's org and a
   made-up slug.
6. The picker: lists only active memberships; `authorize` re-checks the
   posted org with `active_in`; acting super admins are still refused; a
   forged `org_id` gives no hint that an org exists; `exchange_code` still
   re-checks `FOR SHARE`.
7. The org name in `instructions` and descriptions: length-capped, no way
   to break out of the sentence into tool schema JSON (it is a JSON string).
8. The echoed `organisation` value in errors is cut and cleaned.
9. The brake counts per grant and can't be reset by refreshing.

## Checks

```sh
bin/build-assets
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

Before merging, on production (read only):

```sql
SELECT count(*) FROM projects;
SELECT count(*) FROM tasks;
SELECT conname, pg_get_constraintdef(oid) FROM pg_constraint
 WHERE conname = 'fk-tasks-project_id-to-projects';
```

The last must say `ON DELETE CASCADE`. If it doesn't, stop: the project
delete must then delete tasks itself.

By hand, locally, with realistic data (Raj in Himalayan Ritual and Softly,
Kreepa in Himalayan Ritual):

- Claude Code with `collab-himalayan` and `collab-softly`. In each, ask
  "which organisation are you connected to?" and create a task. Each lands
  in its own org. Record whether the two names kept separate tokens.
- Ask the Himalayan connection to create a project "for Softly". It should
  stop and say it is connected to Himalayan Ritual. Save the output for the
  PR screenshot.
- Six projects in a minute: the brake message.
- `delete_task` and `delete_project` on what it just made; then on a task
  Kreepa noted on: refused.
- A Claude Code session opened before the branch started: a write without
  `organisation` gets the clear error; `/mcp` reconnect fixes it.
- Web: owner deletes a project with tasks and notes; a member sees no button.
- The Allow page with one org and with two, phone and desktop, light and
  dark.

## Screenshots needed

Desktop (1440) and phone (390), light and dark where colours or layout
change:

- The Allow page picker (two orgs) and the one-org page.
- The project edit dialog with "Delete project" and "Made with Claude", and
  the confirm.
- The task sheet with "Made with Claude".
- Claude Code output: the mismatch refusal and the "Connected to ..." answer.

## Out of scope (later)

- Holding notifications for Claude-made items (4e).
- A bin with restore for projects and tasks.
- "Made with Claude" on cards, the board and lists.
- Recording which person's Claude made a row.
- Optional `organisation` on read tools.
- A role check on web task delete (any member can delete any task today).

## Steps for the engineer

1. Test that `initialize`, `discover` and `tools/list` see the member through
   `Parts`. Stop and report if not.
2. `McpConnection` in the bearer gate. `instructions`, `organisation` in
   results, not-found text. Tests.
3. `organisation` parameter, `check_org`, `list_tools` override. Update
   existing MCP tests. New tests.
4. Migration, entities, `claude_grant` argument on both `create`s. Tests.
5. Delete tools. Tests.
6. Web project delete. Tests.
7. Brake. Tests.
8. Picker. Tests.
9. "Made with Claude" lines. Render tests.
10. Docs: README, `docs/notes/claude-connector.md`.
11. Checks, the hand run, screenshots.

## Questions for Raj

Each with a recommendation.

1. **Picker in this PR?** It is the root fix and the mockup is approved.
   Recommend: yes.
2. **`organisation` required in the schema but a soft tool error when
   missing?** A hard schema error would hit old sessions with a protocol
   error. Recommend: yes, as planned.
3. **Record the grant, not the access token row?** Access token rows rotate
   hourly and are pruned. Recommend: the grant id, no foreign key.
4. **Web project delete: hard delete with its tasks, owners and admins only,
   confirm with the task count?** Alternatives: move tasks to chores, or a
   bin with restore (its own PR). Recommend: hard delete as planned.
5. **Claude's deletes: same connection, 24 hours, nothing a person wrote?**
   Recommend: yes. Anything wider is a web-app job.
6. **Rate brake at 5 projects or 20 tasks a minute per connection?**
   Recommend: in, at those numbers.
7. **"Made with Claude" on the task sheet and project dialog only?**
   Recommend: in, those two places.
8. **Hold notifications for Claude-made items?** Recommend: later.
9. **README: one named connection per org?** Recommend: in.
10. **Delete the notifications that link to a deleted task?** Applies to the
    web project delete and Claude's deletes. Web task delete keeps today's
    behaviour. Recommend: yes for the new deletes; web task delete can follow
    in a small change.
