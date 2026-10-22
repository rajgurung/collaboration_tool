# Claude connector for people in more than one organisation

Status: Approved by Raj 22 October 2026, with additions A to D below · One PR · Owner: Raj

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
`grant_id` is a plain `Uuid`, never an `Option`. New small struct in
`src/extractors/bearer.rs`. It never goes into a tool result.

New `fn connection(parts) -> Result<McpConnection, Fail>` in
`src/mcp/mod.rs`, beside `member()` (`:204`). A missing extension is
`Fail::Broken`, in creates and deletes alike. Nothing falls back to `None`:
a create without it would record a web-looking row, and a delete compared
against `None` would match every web-made row.

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
- `get_info` keeps the fixed text, because the trait needs it. But
  `initialize` and `discover` never use it: with no member in the request
  they return an internal error, not the fixed `INSTRUCTIONS`.
- `discover` keeps rmcp's `cache_scope: Private` default
  (`rmcp/src/model.rs:1391`). The text differs per connection, so no shared
  cache may hold it.
- First step for the engineer: tests that both rmcp paths see the member.
  `initialize` with protocol `2025-11-25`, and `discover` and `tools/list`
  with the `2026-07-28` `_meta` version (`model.rs:169`). Each must return
  the connected org. If either path falls back to the fixed text, or the
  `Parts` aren't there, stop and report. Don't work around it.

**`organisation` in every result.** Change `finish(outcome)` (`:191`) to
`finish(parts, outcome)`. On success, when the value is a JSON object, it
inserts `"organisation": {"name": ..., "slug": ...}` from the member. All
eight tools (and the two new ones) go through `finish`, so one change covers
them. Arrays are always wrapped in an object today (`{"projects": ...}`,
`{"tasks": ..., "more": ...}`, `{"members": ...}`), so nothing is missed.

**Errors name the org.** New `Fail::NotFound` variant. `From<ModelError>`
maps `EntityNotFound` to it (`:168`) instead of a `Refused` string. `finish`
formats it with the member's org: "Not found in {org name}." Nothing depends
on matching message text. Other tool errors stay as they are.

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
in `src/mcp/mod.rs`, called first in every write `*_for`, the two deletes
included, before `Team::load` or any id lookup. It makes no database call:
it compares against `member.org.slug`, already loaded by the bearer gate.

- Missing or blank: `Fail::Refused("Collab was updated. Tell the user:
  writes now name their organisation. This connection is {name} ({slug}).
  Retry with organisation: \"{slug}\", and restart Claude to refresh its
  tools.")` (addition B). No grace period: a write without it is refused.
- Given, after `trim()` and ASCII lowercase, equal to `member.org.slug`: ok.
- Anything else: `Fail::Refused("This connection is {name} ({slug}), not
  {given}. Nothing was changed. To work in {given}, the user needs a
  connection for that organisation.")`. `{given}` is echoed only when it is
  slug-shaped (`links::is_slug`: 1 to 60 bytes of `a`-`z`, `0`-`9`, `-`).
  Otherwise the message says "not a different organisation" in its place.
  So no free text from the input reaches the model.

The check can only refuse. It never picks an org. The token's org stays the
only org any tool can reach.

**Slug in the descriptions.** Override `list_tools` on `CollabServer`
(`rmcp-macros` `tool_handler.rs:64` skips generating it when the impl has
one). It takes `self.tool_router.list_all()`, reads the member from
`context.extensions`, and for each write tool appends " Connected to {name}
({slug}): pass organisation \"{slug}\"." to the description. Read tools are
unchanged. `call_tool` and `get_tool` stay generated.

- No member in the request: `list_tools` returns an internal error. It never
  lists tools without the slug.
- It sets `cache_scope: Some(CacheScope::Private)` on the result, because
  the descriptions differ per connection. A missing scope means Public on
  the wire (`model.rs:1682`). It keeps `result_type` and `ttl_ms` as rmcp
  builds them (`ListToolsResult::with_all_items`).

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

The migration runs in one transaction, so the `ACCESS EXCLUSIVE` lock from
`ALTER TABLE` is held until the index builds finish. That blocks reads too,
not only writes. At today's table sizes (check with the counts in "Checks")
that is milliseconds. The PR text says this.

**Entities.** Add the one `pub claude_grant_id: Option<Uuid>` field by hand
to `src/models/_entities/projects.rs` and `src/models/_entities/tasks.rs`.
Don't regenerate: it would rewrite other entity files.

#### 3b. Recording it

- `projects::Model::create` (`src/models/projects.rs:136`) and
  `tasks::Model::create` (`src/models/tasks.rs:154`) take a new last argument,
  `claude_grant: Option<Uuid>`, and set the column. An argument, not a field
  on `ProjectParams` or `TaskParams`: those are deserialized from web forms,
  and a form must never be able to set it.
- Callers: `src/controllers/roadmap.rs:209` and `src/controllers/tasks.rs:744`
  pass `None`. `src/mcp/mod.rs:425` and `:528` pass
  `Some(connection(parts)?.grant_id)`. About 16 call sites change in all; a
  grep finds 15, the rest in tests (`tests/requests/tenancy.rs` has 4,
  `dashboard_numbers.rs` 2, and one each in `tasks.rs`, `notifications.rs`,
  `note_reactions.rs`, `remove_member.rs`, `tests/models/note_reactions.rs`).
  The compiler lists any the grep misses. Tests pass `None`.
- **Comparing.** Everywhere: `row.claude_grant_id == Some(connection.grant_id)`.
  Never compare two `Option`s, so a NULL row can't match.
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

1. `find_in_org` with `FOR UPDATE`. Not found: `Fail::NotFound`.
2. Refuse unless `claude_grant_id == Some(grant)` and `created_at > now() -
   24 hours`: "Only tasks this connection created in the last 24 hours can
   be deleted from Claude. Ask the user to delete it in Collab Tool."
3. Refuse when anyone else has content on it: "Someone else has added notes
   or reactions to this task, so it can't be deleted from Claude." Their
   words are not Claude's to throw away. "Someone else" means:
   - a note with `author_id IS DISTINCT FROM $user`. A note whose author was
     deleted (`author_id` NULL, `ON DELETE SET NULL`) counts as someone
     else's;
   - a reaction on any of its notes with `user_id <> $user`.
4. Delete notifications with `organisation_id = member.org.id` and
   `link = '/tasks?open={id}'`, using the id of the row locked in step 1.
5. Delete the task. Notes, assignees and reactions cascade.
6. After commit: log and refresh bells (see "After every hard delete").

Returns `{"deleted": {"id", "title"}, "organisation": ...}`.

**`delete_project { organisation, project_id }`.** Description: "Delete a
project this connection created in the last 24 hours, with its tasks, to
undo a mistake. Refused if anyone else has added tasks or notes to it."

New `projects::Model::remove_made_by(...)` in `src/models/projects.rs`. One
transaction:

1. `find_in_org` with `FOR UPDATE`. Inserting a task into the project, or
   moving one in, takes `FOR KEY SHARE` on the project row for the foreign
   key check, so it waits for us.
2. Lock its tasks: `SELECT id FROM tasks WHERE project_id = $1 AND
   organisation_id = $2 FOR UPDATE`. Every check below runs against these
   locked rows, so no task can change its grant or gain a note's parent
   underneath us.
3. Refuse unless the project's `claude_grant_id == Some(grant)` and it is
   under 24 hours old.
4. Refuse when any locked task has `claude_grant_id` other than
   `Some(grant)` (NULL included): "Someone else has tasks in this project,
   so it can't be deleted from Claude. Ask the user to delete it in Collab
   Tool."
5. Refuse when any locked task has someone else's note or reaction (the
   rules in `delete_task` step 3).
6. Delete notifications with `organisation_id = member.org.id` and
   `link IN ('/tasks?open={id}', ...)` for the locked task ids.
7. Delete the project. Tasks, notes, assignees and reactions cascade.
8. After commit: log and refresh bells.

Returns `{"deleted": {"id", "name", "tasks": n}, "organisation": ...}`.

**A race we accept.** A person who submits a new task into the project
while the delete runs waits on the lock, then fails the foreign key check
because the project is gone. They get a 500 and the task isn't saved. This
needs a person and Claude on the same fresh project in the same few
milliseconds. We accept it rather than add code for it.

No notifications are sent for either delete. Nothing like it exists today
for task deletes in the web app.

**After every hard delete** (both tools and the web project delete):

- Log at info level after commit: `user_id`, `org_id`, the project or task
  id, the task count, `acting`, and for the MCP deletes the `grant_id`.
  Logs are the only record once the rows are gone. The grant id is fine in
  logs: it isn't a secret and can't be used as a token.
- Bells: the cleanup removes unread notices, so open pages would show a
  stale count. Call `NotifyHub::publish(org_id, &user_ids)`
  (`src/data/notify_hub.rs:38`) with the owners of the deleted notices, the
  same as `send` does (`src/controllers/notifications.rs:68-70`). Their bell
  refreshes over the stream.

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
  (`src/extractors/current_member.rs:138`), and (recommended, question 11)
  `member.require_own_membership()?` (`:159`), then `find_in_org`, then
  `projects::Model::remove(db, org_id)` (new): one transaction that locks
  the project and its tasks `FOR UPDATE` as in `delete_project`, deletes the
  notifications for those task ids (`organisation_id` filtered), then the
  project. Then the log line (with `acting`) and the bell refresh, as above.
  HTMX gets the same trigger
  shape as task delete (`src/controllers/tasks.rs:847-851`): toast "Project
  deleted", `roadmap-changed`, `close-dialogs`. No HTMX: redirect to
  `member.url("/roadmap")`.
- `assets/views/roadmap/_form.html`: when `project_id` is set and
  `me.can_manage` (the existing template value, as in
  `assets/views/members/index.html:6`), a `btn-text-danger` "Delete project"
  left of Cancel
  (`:47`), with `hx-confirm="Delete “{name}” and its {n} tasks? Their notes
  go with them. This can't be undone."` (`n` = 0 reads "and its 0 tasks";
  the engineer may drop the clause for 0). Same pattern as the task sheet
  (`assets/views/tasks/_sheet.html:43`). `form_data` (`:151`) adds
  `task_count`. `me` is set by `member.page` (`current_member.rs:198`), not
  by `fragment`, and the form is a fragment. So `form_data` also adds
  `"me": {"can_manage": member.can_manage()}`, keeping the template's name.
- What goes: the project, its tasks, their notes, assignees, note reactions,
  and the notifications that link to those tasks. Project-owner notices link
  to a lane (`/roadmap?lane=...`), still valid, and stay.
- Who: owners and admins (`can_manage`). A super admin acting in the org has
  the owner role, so without `require_own_membership` they could delete
  another org's project. Documents and member removal already require own
  membership (`current_member.rs:159`). Recommend the same here; the log
  line records `acting` either way.

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
  signed-in and acting checks. Call `memberships::Model::for_user_with_orgs`
  (`src/models/memberships.rs:206`) and filter to active rows in the
  handler, with the pinned org first. The query itself doesn't change: the
  switcher uses it. One org: today's page. Two or more: the radio cards,
  `name="org_id"`, the pinned org checked. The hidden `org_id` field (`:335`)
  goes when there is a picker.
- The checked card is always the pinned org. `authorize_page` ignores any
  `org_id` in the query string (`AuthorizeParams` parses one, `:136`), so a
  crafted link can't pre-select an org.
- `authorize` (`:343`), in this order:
  1. Acting super admins get `acting.html`, as today (`:354`).
  2. Deny goes back with `access_denied`, as today.
  3. `org_id` is now a choice, not a pin. Parse it, then
     `memberships::Model::active_in(db, org_id, user.id)` (`:879`), with the
     session's user.
  4. A non-number, an org they never joined, one they left, and one where
     they are pending all get the same page and message: "You can't connect
     Claude to that organisation." and the list of active orgs. No code is
     issued. Nothing in the response says whether the org exists.
  5. The code is issued for the chosen org (`:373`).
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
- 5 projects or 60 tasks already: refuse with "Paused: this connection has
  made 5 projects in {name} ({slug}) in the last minute. Confirm with the
  user that this is the right organisation and that they want more, then
  try again in a minute."
- Constants `PROJECTS_PER_MINUTE = 5`, `TASKS_PER_MINUTE = 60` in
  `src/mcp/mod.rs`. 60, not 20: 20 would stop a normal "turn this plan into
  30 tasks". The project brake is the one that catches a wrong-org run.
- Per connection, so a token refresh doesn't reset it.
- The Softly run (9 projects) would have stopped at the sixth with the org
  name in the message.

**A guard against mistakes, not a security control.** Ways around it, all
accepted:

- Parallel calls: several can pass the count at the same moment.
- More connections: each personal token (up to 10) and each OAuth grant has
  its own count.
- Waiting a minute.
- Tasks in existing projects, `update_task` and `add_task_note` aren't
  counted.
- Anyone with a token can do the same through the web app at any speed.

#### 4c. "Made with Claude" line (recommend: in, two places)

- Task sheet (`assets/views/tasks/_sheet.html`): a muted line "Made with
  Claude" under the details.
- Project edit dialog (`assets/views/roadmap/_form.html`): the same line.
- The handlers pass `made_with_claude: bool`
  (`claude_grant_id.is_some()`). The uuid itself never reaches a template.
  Check that `sheet_data` and `form_data` don't serialize the whole model;
  if they do, drop the field there.
- Not on cards, the board or lists. Later if wanted.
- It says how, not who. We don't record which person's Claude, and the
  token row that knows is pruned 30 days after it is revoked.

#### 4d. Docs: one connection per org (recommend: in)

- `README.md:43-44`: for Claude Code, one named connection per org:
  `claude mcp add --transport http collab-softly https://collab.zen-scale.io/mcp`,
  pick Softly on the Allow page; then `collab-himalayan` and pick Himalayan
  Ritual. For claude.ai, one custom connector per org, each named after it.
- Unverified: that Claude Code, and claude.ai, keep a separate OAuth token
  per connection for the same URL. The engineer checks both by hand with two
  names and two orgs, and writes down what happened.
- If a client can't hold two connections to the same URL, the README for
  that client says: use the picker, and to switch org, disconnect and
  connect again, picking the other org. The mismatch and "Say which
  organisation" messages then say "connect again and pick that
  organisation" instead of "add a connection". The messages stay one
  string for all clients, so they use the wording that works everywhere.
- `docs/notes/claude-connector.md`: the new tools, the `organisation`
  parameter, the limits on deletes, the brake, the picker. Remove "and no
  delete" (`:27`). Say that any member, not only owners and admins, can
  undo a project their own Claude connection made in the last 24 hours, by
  asking Claude to delete it. That is deliberate: it is their own mistake to
  undo, and it can't touch anyone else's work.

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
| `delete_task` | Same | Tasks made by this connection in the last 24 h, no one else's notes or reactions |
| `delete_project` | Same | Projects made by this connection in the last 24 h, all tasks by this connection, no one else's notes or reactions |
| Web project delete | Owners and admins, own membership (recommended) | `find_in_org`, `CurrentMember` claim check |
| Picker | Signed in, not acting | Only orgs where the person is active; re-checked on Allow and at code exchange |

Every lookup stays `find_in_org(member.org.id, ...)` or `in_tenant`. No tool
takes an org id. The `organisation` slug is compared, never looked up.

## Edge cases

- **Old-schema client, write without `organisation`.** Tool error, nothing
  written. Covered above.
- **`organisation: "Himalayan-Ritual "`.** Trimmed and lowercased: accepted.
- **`organisation` that isn't slug-shaped** (spaces, a sentence, 500
  characters). Refused, and the message says "not a different
  organisation" without echoing it.
- **Org renamed.** The slug doesn't change, so a saved slug keeps working.
  The name in messages follows the rename.
- **A task Claude made, then moved by a person into a human project.**
  `delete_task` still allows it within 24 hours (it is Claude's task). Its
  move is lost with it. Acceptable.
- **A person reassigns or edits a Claude-made task.** `delete_task` still
  allows it. Only notes and reactions block it, since those are a person's
  own.
- **A person adds a task to a Claude-made project while it is deleted.** They
  wait on the project lock, then their insert fails its foreign key check:
  a 500, task not saved. Accepted (section 3c).
- **A note by a deleted user.** `author_id` is NULL. It counts as someone
  else's, so Claude can't delete the task.
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
- The 15 or so `projects::Model::create` and `tasks::Model::create` calls in
  tests pass `None`.

**New: `tests/requests/mcp_orgs.rs`** (add to `tests/requests/mod.rs`).
Setup: Raj active in Himalayan Ritual and Softly, Kreepa only in Himalayan
Ritual. Helper `token_in(ctx, email, slug)` (like `token_for`, `mcp.rs:22`,
but for a named org).

Both rmcp paths (engineer step 1):

- `initialize_names_the_org`: protocol `2025-11-25`; instructions start with
  "Connected to Himalayan Ritual (himalayan-ritual) as raj".
- `discover_names_the_org`: `server/discover` with the `2026-07-28` `_meta`
  version; instructions name the org, `cacheScope` is `"private"`.
- `tools_list_is_private_and_names_the_slug`: `tools/list` with the
  `2026-07-28` `_meta` version returns `cacheScope: "private"`, and each
  write tool's description contains `himalayan-ritual`. Neither path ever
  returns the fixed `INSTRUCTIONS` text alone.

The tools:

- `write_tools_require_organisation`: each write tool's
  `inputSchema.required` contains `organisation`. Read tools don't.
- `every_result_names_the_org`: all ten tools' success results carry
  `organisation == {"name": "Himalayan Ritual", "slug": "himalayan-ritual"}`.
- `a_write_for_another_org_is_refused`: each write tool, the two deletes
  included, with `organisation: "softly"` is a tool error starting with
  exactly "This connection is Himalayan Ritual (himalayan-ritual), not
  softly." Row counts of projects, tasks, notes and notifications unchanged.
- `a_write_without_organisation_is_a_tool_error_not_a_protocol_error`:
  `isError: true`, names the org, nothing written.
- `an_odd_organisation_is_not_echoed`: 500 characters with a newline, and
  "Softly please": the message says "a different organisation" and contains
  neither input.
- `rows_record_the_connection`: MCP-made project and task have the token's
  `grant_id`; web-made ones (`POST /o/.../roadmap/projects`, `/tasks`) have
  NULL.
- `the_record_survives_a_token_refresh`: OAuth flow (reuse the helpers in
  `tests/requests/oauth.rs`), create a task, refresh, `delete_task` works
  with the new access token.
- `delete_task_rules`: refused for a second personal token's task, a
  web-made task, a task backdated 25 hours, a task with Kreepa's note, a
  task with a note whose author is NULL, and a task where Kreepa reacted to
  Raj's note. Allowed with Raj's own note and reaction. Success removes
  notes, assignees, reactions and this org's `/tasks?open={id}`
  notifications; other notifications stay, including another org's with the
  same link text.
- `delete_project_rules`: refused with one web-made task inside, with a task
  from another connection, with Kreepa's note or reaction on a task, with a
  NULL-author note, after 24 hours. Success removes the project and all its
  tasks, and returns the count.
- `delete_tools_stay_in_their_org`: ids from another org are "Not found in
  Himalayan Ritual." and nothing is deleted.
- `no_connection_means_no_delete_and_no_create` (a unit or model-level test,
  since the bearer gate always adds it): with the `McpConnection` extension
  missing, `delete_task` on a web-made task is `Fail::Broken` and the task
  is still there; `create_task` is `Fail::Broken` and writes nothing.
- `the_brake_stops_the_sixth_project_and_sixty_first_task`: the sixth
  `create_project` in a minute is refused with "Confirm with the user";
  after backdating the five by 61 seconds it works. Same for 61 tasks. A
  second connection has its own count.
- `hard_deletes_are_logged`: if the suite already captures tracing output,
  check the line has the user, org, id, count and `acting`. Otherwise the
  engineer checks by hand and says so in the PR.

**`tests/requests/roadmap.rs`.**

- `owners_and_admins_delete_a_project_with_its_tasks`: owner and admin both;
  tasks, notes, assignees, reactions and task notifications gone; other
  projects untouched; HTMX trigger and non-HTMX redirect.
- `members_cannot_delete_projects`: 403, nothing deleted; the button isn't
  rendered for them.
- `a_super_admin_acting_cannot_delete_a_project` (if question 11 is yes):
  403 with the "Leave admin mode" message, nothing deleted.
- `another_orgs_project_is_not_found`: 404, nothing deleted.
- The new route is picked up by the route walk in
  `tests/requests/org_address.rs` (`a_strangers_org_and_a_made_up_slug_look_identical`).
  Confirm it is listed.

**`tests/requests/oauth.rs`.**

- `the_picker_lists_only_active_orgs_pinned_first`: pending, removed and
  declined orgs absent; roles and usernames shown.
- `the_query_string_cannot_pick_the_org`: `GET /oauth/authorize?...&org_id=<softly>`
  with the cookie on Himalayan Ritual: Himalayan Ritual is checked.
- `one_org_shows_no_picker`.
- `allow_binds_the_chosen_org`: cookie on Himalayan Ritual, pick Softly, the
  token works in Softly and `initialize` says Softly.
- `every_unusable_org_gets_the_same_answer`: `org_id` of `abc`, an org they
  never joined, one they left, and one where they are pending. All four
  responses have the same status and body (apart from the form's own
  fields), and no code is issued.
- `a_super_admin_acting_elsewhere_is_asked_to_leave_first` (`:628`) still
  passes with two orgs, and the acting page comes before any org check.

**Render tests** (`tests/views/render.rs`): the "Made with Claude" line shows
only when `made_with_claude` is true, and no rendered page contains the
grant uuid.

## Risks

- **The org name goes into the model's context.** An owner can set an org
  name like "Ignore earlier instructions". The same is already true of task
  titles and notes in tool results. Names are 2 to 80 characters. The
  instructions quote it as a name, in one sentence.
- **Claude may copy the slug without asking.** The parameter is a speed
  bump. The picker and one connection per org are the real guard.
- **A hard delete can't be undone.** Owners and admins only, with the task
  count in the confirm. Claude's deletes are narrow by design. Every hard
  delete is logged.
- **A rare 500** for a person adding a task to a project as it is deleted.
  Accepted.
- **`initialize` and `discover` overrides.** If rmcp changes how it
  negotiates, our overrides could drift. We call its own
  `negotiate_initialize`, as its docs show, and the step 1 tests cover both
  protocol versions.
- **Migration lock.** Held through the index builds; covered by
  `lock_timeout` and today's table sizes.

## Rollback

- **Code only (preferred).** Revert the PR without running the down
  migration. The old code doesn't select `claude_grant_id`, and the columns
  are nullable, so old inserts work. The picker goes back to last-used.
- **Schema too.** Then `cargo loco db down` once: drops both columns and
  their indexes. Loses only the "made with Claude" marks. Safe.
- **Deleted data doesn't come back** in either case. That is the point of
  the confirm and the narrow tool rules.

## What the security reviewer must check

1. `check_org` runs first in every write tool, both deletes included, before
   any lookup. It makes no database call and can only refuse. No path takes
   an org from the input.
2. `claude_grant_id` can't be set from a web form or a tool argument. Only
   from the bearer's `McpConnection`. A missing `McpConnection` is
   `Fail::Broken` in creates and deletes; nothing falls back to `None`, and
   every comparison is `== Some(grant)`.
3. The grant id is never read from a request except in `revoke_mine`
   (`src/controllers/claude.rs:152-154`). The "Made with Claude" line renders
   no uuid: templates get `made_with_claude: bool`.
4. Both delete tools: tenant-scoped, grant-scoped, 24 hours, the notes and
   reactions rule (NULL authors count as someone else), one transaction,
   the project locked and then its tasks locked `FOR UPDATE`, checks run
   against the locked rows.
5. Notification cleanup filters `organisation_id = member.org.id` and uses
   task ids read inside the locked transaction.
6. Web project delete: `require_manager` and (recommended)
   `require_own_membership`, `find_in_org`, POST only, claim checked by
   `CurrentMember`, same response for a stranger's org and a made-up slug.
7. Every hard delete is logged at info level with user, org, item, task
   count, `acting`, and the grant for MCP deletes.
8. The picker: lists only active memberships, pinned first; ignores
   `org_id` in the query string; in `authorize`, the acting check comes
   before `active_in(org_id, session user)`; a non-number, a never-joined
   org, a left org and a pending org get the same page; `exchange_code`
   still re-checks `FOR SHARE`.
9. `instructions`, `discover` and `list_tools` fail if the member is
   missing, and never fall back to the fixed text. `list_tools` and
   `discover` answer with `cacheScope: private`.
10. The org name in `instructions` and descriptions: length-capped, no way
    to break out of the sentence into tool schema JSON (it is a JSON string).
11. The `organisation` value is echoed only when slug-shaped.
12. The rate brake is a guard against mistakes, not a security control. The
    ways around it are listed in section 4b and accepted.

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
- claude.ai with two custom connectors on the same URL, one per org. Same
  checks. If claude.ai can't hold two, switch the README and the messages to
  "connect again and pick that organisation" (section 4d).
- Ask the Himalayan connection to create a project "for Softly". It should
  stop and say it is connected to Himalayan Ritual. Save the output for the
  PR screenshot.
- Six projects in a minute: the brake message.
- `delete_task` and `delete_project` on what it just made; then on a task
  Kreepa noted on: refused. Check the info log lines.
- A Claude Code session opened before the branch started: a write without
  `organisation` gets the clear error; `/mcp` reconnect fixes it.
- Web: owner deletes a project with tasks and notes; a member sees no button;
  an assignee's bell count drops without a reload.
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

## PR text must say

- The migration adds two nullable columns and two partial indexes. The
  `ACCESS EXCLUSIVE` lock is held while the indexes build, which is fine at
  today's table sizes (give the counts). `lock_timeout` 5s. It rolls back
  with `cargo loco db down`; a code-only revert is also safe.
- Project delete is a hard delete. Nothing restores it.

## Out of scope (later)

- Holding notifications for Claude-made items (4e).
- A bin with restore for projects and tasks.
- "Made with Claude" on cards, the board and lists.
- Recording which person's Claude made a row.
- Optional `organisation` on read tools.
- A role check on web task delete (any member can delete any task today).
- Notification cleanup and bell refresh for web task delete.

## Steps for the engineer

1. Tests that `initialize` (`2025-11-25`) and `discover` and `tools/list`
   (`2026-07-28` `_meta`) see the member through `Parts`. Stop and report if
   not.
2. `McpConnection` in the bearer gate, `connection()`. `instructions`,
   `discover`, `organisation` in results, `Fail::NotFound`. Tests.
3. `organisation` parameter, `check_org`, `list_tools` override with
   `cache_scope` Private. Update existing MCP tests. New tests.
4. Migration, entity fields by hand, `claude_grant` argument on both
   `create`s and their call sites. Tests.
5. Delete tools, with the locks, the notes and reactions rule, the scoped
   notification cleanup, the log line and the bell refresh. Tests.
6. Web project delete. Tests.
7. Brake. Tests.
8. Picker. Tests.
9. "Made with Claude" lines. Render tests.
10. Docs: README, `docs/notes/claude-connector.md`.
11. Checks, the hand run (Claude Code and claude.ai), screenshots.

## Changes after review

Staff and security both approved with changes. All applied.

- **Grant comparison.** `McpConnection.grant_id` is a plain `Uuid`. Every
  check is `== Some(grant)`. A missing extension is `Fail::Broken` in
  creates and deletes; never a fallback to `None`. Test added (3b, Tests).
- **`delete_project` race.** After the project lock, its tasks are locked
  `FOR UPDATE` and every check runs against them. A person's racing insert
  gets a 500; accepted and written down (3c).
- **Someone else's content.** Notes with `author_id IS DISTINCT FROM $user`
  (NULL authors included) and other people's reactions on Claude's notes
  block a delete. Tests added (3c).
- **Notification cleanup** filters `organisation_id` and uses task ids read
  inside the locked transaction (3c, 3d).
- **Picker.** Active filter in the handler with the pinned org first, the
  switcher's query unchanged; the query string can't pick the card; acting
  check before `active_in`; one page and message for every unusable org (4a).
- **Logging.** Every hard delete logs user, org, item, count, `acting`, and
  the grant for MCP deletes (3c, 3d).
- **Caching.** `list_tools` sets `cache_scope` Private and keeps rmcp's
  `result_type` and `ttl_ms`; `discover` keeps Private. Test added (1, 2).
- **Step 1** tests both rmcp paths; neither may fall back to the fixed text.
- **claude.ai** added to the two-connection hand check, with a fallback in
  the docs and messages (4d, Checks).
- **Security checklist** rewritten (items 1, 2, 3, 9, 12 and new ones).
- **Nice to have, all in:** `Fail::NotFound` (1); the create call sites
  listed (3b); entity fields by hand (3a); `me.can_manage` (3d); bell refresh
  over the stream after cleanup (3c); slug-shaped echo only (2); `acting` in
  the log and own membership recommended for web delete (3d, question 11);
  member self-undo documented (4d); the lock note for the PR text.
- **Task brake** raised from 20 to 60 a minute. Projects stay at 5 (4b,
  question 6).

## Additions after approval (22 October 2026)

Raj approved every recommendation in "Questions for Raj", with no grace
period: a write missing `organisation` is refused. One organisation per
connection. Four additions, all in this PR.

### A. Update notice in tool results

Connections made before this release get a notice in every successful tool
result until they have made one write that passes `organisation`, and for
at most 14 days:

```json
"notice": "Tell the user once: the Collab connector was updated. Each write now names its organisation (this connection is Himalayan Ritual (himalayan-ritual)). Restart Claude, or reconnect the connector, to load the new tools."
```

**Where it is recorded: a small table, not a column on `access_tokens`.**
New table `claude_update_notices`, keyed by `grant_id`, in the same
migration as 3a:

```sql
CREATE TABLE claude_update_notices (
  grant_id uuid PRIMARY KEY,
  organisation_id bigint NOT NULL,
  user_id bigint NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  announced_at timestamptz NULL,
  written_at timestamptz NULL
);
INSERT INTO claude_update_notices (grant_id, organisation_id, user_id)
SELECT DISTINCT ON (grant_id) grant_id, organisation_id, user_id
  FROM access_tokens
 WHERE revoked_at IS NULL
   AND (kind = 'personal' OR refresh_expires_at > now())
 ORDER BY grant_id, id;
```

Why a table: a grant is many token rows (one per refresh), so a column on
`access_tokens` would have to be copied onto every new row by `issue`, and
pruning would lose it. A row per grant is set once by the migration, needs
no change to token code, and is easy to set up in tests. "Made before this
release" means "has a row". `created_at` is the migration time, so the 14
days count from the deploy. No foreign keys, like `claude_grant_id`: it is a
marker that dies with time, not data to protect.

- The bearer gate reads the row by primary key and sets
  `McpConnection.update_notice: bool` (row exists, `written_at` NULL,
  `created_at` within 14 days). `finish` adds `"notice"` next to
  `"organisation"` on success.
- After `check_org` passes in a write, and only when `update_notice` is
  true, `written_at` is set. One cheap update, once per grant.
- Tool errors don't carry it. The missing-organisation refusal (B) says
  the same thing, and other errors are about the input.
- Write results don't carry it either: a write that succeeded named its
  organisation, so the connection already has the new tools. (Found in the
  Claude Code hand check: the notice on that write's own result confused it.)
- After 14 days the code is dead; a later PR drops the table and the code.

### B. The missing-organisation refusal is written for Claude to relay

"Collab was updated. Tell the user: writes now name their organisation.
This connection is {name} ({slug}). Retry with organisation: \"{slug}\",
and restart Claude to refresh its tools." A wrong organisation keeps the
mismatch wording from section 2.

### C. A one-time bell notice

On the first server start after the deploy, every person with a live
Claude connection gets one bell notification in that connection's org:
"Claude connector updated." with the line "If Claude says a write needs an
organisation, restart it.", linking to `/o/{slug}/settings/claude`. No
email (only mentions are emailed).

- Guard: `claude_update_notices.announced_at`. A new initializer's
  `before_run` calls `claude_update_notices::Model::announce`, which in one
  statement sets `announced_at` on rows where it is NULL, `RETURNING` the
  org and user, then sends one notice per (org, user). A second start, or a
  second instance, finds nothing to announce. Skipped in tests, which call
  `announce` directly.
- New notification kind `connector`. The list shows it with no actor name
  ("Collab Tool" avatar) and a plug icon.
- Only active members get it (joined to `memberships`, status active).

### D. `/settings/claude` names the organisation

Each connection row says which organisation it reaches ("Connector ·
Himalayan Ritual · Connected ..."). Under the list: "Each connection
reaches one organisation. To use Claude in another organisation, connect
again and pick it on the Allow page." The page already lists only this
org's connections, so the name says which one you are looking at.

### Tests for A to D

- `the_update_notice_shows_until_a_write_names_the_org`: a grant with a
  row gets `notice` on reads and on a refused write; after a write with
  `organisation` it is gone. A grant without a row never sees it.
- `the_update_notice_stops_after_14_days`: row backdated 15 days, no notice.
- `the_bell_notice_is_sent_once`: two grants for one person in one org and
  one in another org: `announce` sends one notice per org, with the link and
  the `connector` kind; a second `announce` sends nothing. A row for a
  person no longer active sends nothing.
- `the_settings_page_names_the_org` and the "connect again" line.
- `a_write_without_organisation_is_a_tool_error_not_a_protocol_error`
  checks the exact B wording.

### Rollback for A to D

`down` drops `claude_update_notices` and deletes notifications of kind
`connector` (old code would show them as "Someone ...").

## Questions for Raj (answered: all recommendations approved)

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
5. **Claude's deletes: same connection, 24 hours, nothing a person wrote or
   reacted?** Recommend: yes. Anything wider is a web-app job.
6. **Rate brake at 5 projects or 60 tasks a minute per connection?** 20 tasks
   would stop "turn this plan into 30 tasks". Recommend: in, at 5 and 60.
7. **"Made with Claude" on the task sheet and project dialog only?**
   Recommend: in, those two places.
8. **Hold notifications for Claude-made items?** Recommend: later.
9. **README: one named connection per org?** Recommend: in, for each client
   the hand check shows can hold two; otherwise "connect again and pick".
10. **Delete the notifications that link to a deleted task, and refresh
    bells?** Applies to the web project delete and Claude's deletes. Web task
    delete keeps today's behaviour. Recommend: yes for the new deletes; web
    task delete can follow in a small change.
11. **Web project delete needs your own membership (not a super admin acting
    in the org)?** Matches documents and member removal. Recommend: yes.
