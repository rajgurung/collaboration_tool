# Plan: connect Claude to Collab Tool (MCP server with OAuth and personal tokens)

Status: approved by staff review (2026-10-08, second pass). The user asked for one PR, so the
reviewer's suggested split into two PRs is not taken. One PR, branch `claude-connector`, worktree
`../collaboration_tool-mcp`.

## Goal

Let Claude read and change tasks and projects in Collab Tool, the way Claude
connects to Linear:

- **claude.ai (web, desktop, mobile):** add a custom connector with the URL
  `https://collab.rajgurung.me/mcp`. Claude sends you to a Collab Tool "Allow
  access" page (OAuth). No password goes to Claude.
- **Claude Code:** `claude mcp add --transport http collab https://collab.rajgurung.me/mcp`
  and sign in the same way. Or paste a personal access token as a header.

Every call acts as one person in their one organisation, and can do only what
that person can do in the browser. Notifications fire exactly as they do for
the web pages.

First real use after deploy: create a "Collab Tool website" project and backfill
the shipped PRs as Done tasks.

## Background facts (from research, 2026-10-08)

- MCP (Model Context Protocol) is how Claude calls tools on a server. The
  latest spec is 2026-07-28 (stateless, no `initialize`), but Claude's docs only
  name 2025-03-26, 2025-06-18 and 2025-11-25. The server must handle both eras.
- `rmcp` 3.5.x (official Rust SDK) supports 2025-03-26 to 2026-07-28. Its
  `StreamableHttpService` is a plain tower service on http 1.x, so it nests into
  our axum 0.8 router. Not yet test-compiled against our Cargo.lock.
- Tool handlers can read `http::request::Parts` from the request context, so an
  axum middleware can put the authenticated member in `parts.extensions`.
- OAuth for MCP: Protected Resource Metadata (RFC 9728), authorization server
  metadata (RFC 8414), PKCE S256, `resource` parameter bound to the token
  (RFC 8707), 401 with `WWW-Authenticate: Bearer resource_metadata="…"`.
- Claude redirect URIs: `https://claude.ai/api/mcp/auth_callback` (hosted apps),
  `http://localhost:<port>/callback` and `http://127.0.0.1:<port>/callback`
  (Claude Code, any port).
- Client registration: Claude uses CIMD only if we advertise it, otherwise it
  falls back to Dynamic Client Registration (DCR, RFC 7591).
- No maintained Rust crate covers an OAuth authorization server with these
  RFCs. Hand-rolling the few endpoints is normal.
- Each user has at most one membership (`memberships::Model::find_for_user`),
  so a token never needs an organisation choice.

## Decisions

1. **rmcp 3.5.x**, features `server`, `macros`, `transport-streamable-http-server`.
   Config: `legacy_session_mode: false`, `json_response: true`,
   `allowed_hosts`/`allowed_origins` from `settings.app_url` (plus localhost in
   development and tests). Step 1 is a compile spike. If rmcp does not fit our
   dependency graph, fall back to a hand-written stateless JSON-RPC handler for
   2025-11-25 only (`initialize`, `tools/list`, `tools/call`, `ping`) and say so
   in the PR.
2. **Mount `/mcp` as a normal Loco route**, not in an initializer. Loco runs
   `after_routes` after its own middleware (body limit, catch_panic, logger,
   request id; `loco-rs-1.2.0/src/boot.rs:543`), so an initializer mount would
   skip all of it. Add `controllers::mcp::routes(ctx)` (called from `App::routes(ctx)`; the
   rmcp service and the bearer layer via `from_fn_with_state` both need the
   context) using
   `axum::routing::post_service(svc)` (GET/DELETE answer 405), with bearer auth
   as a `route_layer`. The service holds an `AppContext` clone. Because it is a
   normal route, `OriginCheckInitializer` covers it too; a test confirms a
   foreign `Origin` is refused and a missing `Origin` is allowed.
3. **DCR only, no CIMD.** CIMD means our server fetches arbitrary client URLs
   (SSRF surface) for no gain: Claude falls back to DCR. Revisit when DCR
   support is removed from Claude.
4. **Redirect URI allowlist.** `/oauth/register` accepts only
   `https://claude.ai/api/mcp/auth_callback`, `https://claude.com/api/mcp/auth_callback`,
   and loopback `http://localhost|127.0.0.1:<any port>/callback`. Anything else
   gets `invalid_redirect_uri`. This is a private team tool, so the allowlist
   removes open-registration phishing risk. The list lives in one constant.
   **Matching is on the parsed URL** (`url` crate), never string prefixes. One
   function, used by register and authorize: for claude.ai/claude.com, exact
   scheme, host and path; for loopback, scheme `http`, host exactly `localhost`
   or `127.0.0.1`, path exactly `/callback`, no userinfo, no query, no fragment,
   any port. Unit tests reject `http://localhost.evil.com/callback`,
   `http://localhost:1@evil.com/callback`,
   `https://claude.ai.evil.com/api/mcp/auth_callback`,
   `http://localhost/callback/../x`.
5. **One scope: `tasks`.** Read and write projects, tasks, assignees and task
   notes. No member, role, chat, meeting or admin access. Tools refuse anything
   else by not existing.
6. **Tokens are opaque random strings, stored as SHA-256 hashes.** 32 random
   bytes, base64url, with a readable prefix: `collab_pat_`, `collab_at_`,
   `collab_rt_`. Compare by looking up the hash (constant-time is not needed
   when looking up by hash of a high-entropy secret). The plain value is shown
   once and never stored.
7. **Lifetimes.** OAuth access token 1 hour. Refresh token 30 days, rotated on
   every use; the 30 days **slide** from each rotation, so a connector used at
   least monthly stays connected. Using an already-rotated refresh token
   revokes the whole grant. Authorization codes 10 minutes, single use; reusing
   a code also revokes its grant. Personal access tokens do not expire until
   revoked; they show "last used". Docs note that changing `APP_URL`
   invalidates every token (the `resource` is stored).
7a. **Single use is atomic.** Redeem with one statement, never read-check-write:
   `UPDATE oauth_codes SET used_at = now() WHERE code_hash = $1 AND used_at IS NULL AND expires_at > now() RETURNING *`,
   and the same on `access_tokens` for refresh
   (`WHERE refresh_hash = $1 AND revoked_at IS NULL AND refresh_expires_at > now()`,
   setting `revoked_at`). Zero rows: look the row up again to tell "reused"
   (revoke the grant, `invalid_grant`) from "unknown or expired"
   (`invalid_grant`). A rotated row keeps its `refresh_hash`; only
   `revoked_at` is set, so reuse stays detectable.
7b. **The refresh grant re-checks everything:** `client_id` matches the row's
   client, `resource` matches (absent means `{app_url}/mcp`), and the
   membership is still active.
8. **Membership checked on every request.** A token works only while the user
   has an active membership in the token's organisation. Today the only way an
   active member loses access is the user being deleted (memberships cascade;
   `reject` only applies to pending ones), but the check is cheap and protects
   future "remove member" work. Super admins get no special power through
   tokens: `acting` is always false. **`/settings/claude` POSTs and
   `/oauth/authorize` refuse `member.acting`** (403), so a super admin cannot
   mint tokens for an organisation they only act inside. Test it.
9. **Actor identity.** Changes made through Claude are made as the token's
   user. Notifications read "Raj assigned you…", same as the web. No "via
   Claude" label in this PR.
10. **Partial updates in tools.** Tools accept only the fields to change. The
    tool loads the task, fills a full `TaskParams` from current values, applies
    the changes, and calls the existing `tasks::Model::update_from`. Validation
    stays in one place.
11. **People by username.** Tools take and return usernames (as people see
    them), resolved inside the org. `list_members` exposes username and role
    only, no emails.
12. **Login `next`.** `/login` gains an optional `next` parameter so the consent
    page can send a signed-out user to log in and come back. Only values that
    start with `/oauth/authorize?` and contain no `\`, CR or LF are accepted
    (checked after URL-decoding); anything else goes to `/`. Nothing else needs
    `next`. It travels as a hidden field in the login form and survives a
    failed-login re-render (`src/controllers/auth.rs`). `/oauth/authorize`
    URL-encodes it when building `/login?next=…`.
13. **DCR is lenient and bounded.** Ignore the requested
    `token_endpoint_auth_method`; always register a public client and return
    `"token_endpoint_auth_method": "none"` (RFC 7591 lets the server override).
    Cap the body at 8 KB with `axum::extract::DefaultBodyLimit::max(8192)` on
    that route (Loco's `limit_payload` is app-wide), `client_name` at 100 chars, 5 redirect URIs.
    Prune opportunistically on each register: clients older than 24 hours with
    no codes or tokens. Prune `access_tokens` rows revoked or expired more than
    30 days ago ("expired" means `coalesce(refresh_expires_at, expires_at)`),
    and `oauth_codes` older than a day, on each `/oauth/token` call. The
    migration indexes `oauth_codes.created_at`, `access_tokens.revoked_at` and
    `access_tokens.expires_at`/`refresh_expires_at` for these deletes.
14. **No secrets in URLs or logs.** The PAT is rendered in the POST response
    body, never in a redirect or query string (the Loco logger records
    `http.uri`). Codes appear only in the redirect to Claude, as OAuth requires.
15. **Consent page cannot be framed or cached:** `X-Frame-Options: DENY`,
    `Content-Security-Policy: frame-ancestors 'none'`, `Cache-Control: no-store`
    on `/oauth/authorize` (GET and POST). `no-store` also on the PAT response
    and all `/oauth/token` responses.

## Data model (one migration, additive)

`m2026…_claude_connector` creates three tables, following
`m20261008_175915_saved_views.rs` (`create_table` plus raw `CREATE INDEX`).
`oauth_clients` is not tenant-scoped (clients register before anyone signs
in). `oauth_codes` and `access_tokens` carry `organisation_id` and `user_id`,
both `on_delete = Cascade`.

**`oauth_clients`**: `id`, `client_id` (text, the public random id, unique),
`client_name` (text, max 100), `redirect_uris` (jsonb array, max 5),
`created_at`, `updated_at`.

**`oauth_codes`**: `id`, `code_hash` (unique), `grant_id` (uuid, made at
consent), `oauth_client_id` → `oauth_clients.id` (cascade), `user_id`,
`organisation_id`, `redirect_uri`, `code_challenge`, `resource`, `scope`,
`expires_at`, `used_at` (nullable), timestamps. Index on `grant_id`.

**`access_tokens`**: `id`, `kind` (`personal` | `oauth`), `name` (PAT label or
client name), `token_hash` (unique), `refresh_hash` (unique, nullable),
`grant_id` (uuid; copied from the code; a PAT gets its own), `user_id`,
`organisation_id`, `oauth_client_id` (nullable → `oauth_clients.id`, cascade),
`resource`, `scope`, `expires_at` (nullable), `refresh_expires_at` (nullable),
`last_used_at` (nullable), `revoked_at` (nullable), timestamps.
Indexes: `(organisation_id, user_id)`, `grant_id`.

The name `client_id` is used only for the public text value; integer foreign
keys are `oauth_client_id`.

Revoking a grant (Settings, or reuse detection) sets `revoked_at` on every
`access_tokens` row with that `grant_id` and marks its unused codes as used.

Rollback drops the three tables. Nothing else changes. Generate entities with
`cargo loco db entities`. Add the tables to `App::truncate` in child-first
order.

New direct crates: `rmcp`, `sha2`, `rand`, `base64`, `url` (versions already in
Cargo.lock where possible).

## Endpoints

| Method | Path | What |
|---|---|---|
| GET | `/.well-known/oauth-protected-resource` and `/.well-known/oauth-protected-resource/mcp` | RFC 9728 JSON: `resource` = `{app_url}/mcp`, `authorization_servers` = [`{app_url}`], `scopes_supported` = ["tasks"], `bearer_methods_supported` = ["header"] |
| GET | `/.well-known/oauth-authorization-server` | RFC 8414 JSON: issuer, `authorization_endpoint`, `token_endpoint`, `registration_endpoint`, `response_types_supported` ["code"], `grant_types_supported` ["authorization_code","refresh_token"], `code_challenge_methods_supported` ["S256"], `token_endpoint_auth_methods_supported` ["none"], `scopes_supported` ["tasks"], `authorization_response_iss_parameter_supported` true |
| POST | `/oauth/register` | DCR, JSON in and out. Always registers a public client (see decision 13). Allowlisted redirect URIs only. |
| GET | `/oauth/authorize` | Validates `client_id`, `redirect_uri` (registered for that client, via the shared matcher), `response_type=code`, `code_challenge` + `S256`, `resource` (absent defaults to `{app_url}/mcp`; otherwise must equal it), `scope` (empty or `tasks`), `state`. Bad client or redirect: error page, no redirect. Other errors: redirect with `error=`. Signed out: redirect to `/login?next=<this URL>`. Signed in with an active membership and not acting: render the consent page. |
| POST | `/oauth/authorize` | Consent form (Allow / Deny), same hidden fields. Re-validates everything. Allow: make a `grant_id`, create code, redirect with `code`, `state`, `iss`. Deny: redirect with `error=access_denied`. Protected by the existing Origin check. |
| POST | `/oauth/token` | Form-encoded. `authorization_code`: code, `code_verifier` (43 to 128 chars, S256 check), `redirect_uri`, `client_id`, `resource` (absent = `{app_url}/mcp`) must match the code; atomic redeem (7a); reuse revokes the grant. `refresh_token`: decision 7b checks, atomic rotate; reuse of a rotated token revokes the grant. Errors per RFC 6749 JSON (`invalid_grant`, `invalid_request`, `invalid_client`). `Cache-Control: no-store`. |
| POST | `/mcp` | The MCP service, a normal Loco route with a bearer `route_layer`. GET/DELETE return 405. |
| GET | `/settings/claude` | "Connect Claude" page (see UI). |
| POST | `/settings/claude/tokens` | Create a personal token (name required, max 60). Renders it once in the response body (`no-store`). Limit 10 active per user. Refuses `acting`. |
| POST | `/settings/claude/grants/{grant_id}/revoke` | Revoke a PAT or a whole OAuth grant: every token row and unused code with that `grant_id`, scoped by the member's `user_id` and `organisation_id` (404 otherwise). |

The well-known and OAuth routes are plain Loco controllers in
`src/controllers/oauth.rs`. Settings routes go in `src/controllers/claude.rs`.

## Bearer auth for `/mcp`

A `route_layer` on the `/mcp` route (`src/extractors/bearer.rs`):

1. Read `Authorization: Bearer <token>`. Missing or malformed: 401 with
   `WWW-Authenticate: Bearer resource_metadata="{app_url}/.well-known/oauth-protected-resource", scope="tasks"`.
2. Hash, look up an `access_tokens` row by `token_hash`. Reject if revoked,
   expired, `resource` is not `{app_url}/mcp` (PATs store the same value), or
   scope lacks `tasks`. Same 401 (with `error="invalid_token"`).
3. Load the user and their membership. Reject unless active and in the token's
   organisation.
4. Build a `CurrentMember` (add `CurrentMember::from_membership(user, org, membership)`
   so the cookie extractor and this path share construction; `acting = false`).
5. Update `last_used_at` at most once a minute per token (skip the write if it
   was updated in the last 60s).
6. Insert `CurrentMember` into request extensions. rmcp passes `Parts` through
   to tools.

`CurrentMember` must be `Clone` (or wrapped in `Arc`) for extensions.

## MCP tools (`src/mcp/` module)

All tools take the member from `Parts` extensions; none take an org id. Inputs
use `schemars` structs with descriptions. Outputs are JSON text content with
ids, so Claude can chain calls. Errors from validation come back as tool errors
(`isError: true`) with the field message, not protocol errors.

| Tool | Input | Does |
|---|---|---|
| `list_projects` | none | id, name, lane, status, owner username, progress |
| `create_project` | name, lane (now/next/later), status (free text up to 40 chars, e.g. "Active", "Planned"; not a task status), summary?, owner? | Tool picks `accent` as `ACCENTS[project count % ACCENTS.len()]` (required by `ProjectParams`), then `projects::Model::create` and `notifications::project_saved` |
| `list_members` | none | username, role for active members |
| `list_tasks` | project_id?, status?, assignee? (username or "me"), query?, limit (default 50, max 200) | tasks in the org with project name, assignees, status, priority, due. Assignees via `task_assignees::Model::by_task` (one query), not per row |
| `get_task` | task_id | task plus its notes (author username, body, time) |
| `create_task` | title, project_id?, status?, priority (default medium), due_on?, assignees? | `tasks::Model::create` + `notifications::task_saved(None)` |
| `update_task` | task_id, any of title, project_id (or null for a chore), status, priority, due_on (or null), assignees (replaces the whole list; the schema says so) | merge into `TaskParams`, `update_from` + `task_saved(Some(before))` |
| `add_task_note` | task_id, body | `task_notes::Model::create` + `notifications::note_added` |

Delete is left out on purpose. Tool annotations: list/get are `readOnlyHint`,
none are `destructiveHint`.

Server info: name "Collab Tool", instructions string telling Claude that tasks
belong to projects in lanes Now/Next/Later, statuses are todo, progress,
blocked, done, project status is free text, and people are referred to by
username.

## UI

**More page:** new row "Connect Claude" under Account, linking to
`/settings/claude`.

**`/settings/claude`** (app layout, mobile-first, light and dark):
- Connector URL `{app_url}/mcp` with a copy button.
- "claude.ai": Settings → Connectors → Add custom connector → paste the URL →
  Connect, then Allow.
- "Claude Code": the `claude mcp add --transport http collab {app_url}/mcp`
  command with copy button; sign in when prompted. Collapsed section for a
  personal token: create form, then the token shown once with the
  `--header "Authorization: Bearer …"` command.
- "Connected": one row per active grant (grouped by `grant_id`, so rotated
  refresh rows don't repeat): name, kind, connected on, last used, Revoke.

**`/oauth/authorize` consent page** (auth layout like login):
- "Allow {client_name} to use Collab Tool?"
- "It will be able to see and change projects, tasks and task notes in
  {org name}, as {username}."
- Shows the redirect host ("You'll go back to claude.ai").
- Allow (primary) and Deny buttons.

## Changes to existing code

- `src/extractors/current_member.rs`: `Clone`, shared constructor.
- `src/controllers/auth.rs` + login template: safe `next` parameter.
- `src/app.rs`: register controllers (`mcp::routes(ctx)`), truncate order.
- `config/*.yaml`: nothing new unless rmcp needs it; issuer comes from
  `settings.app_url`.
- Origin check: `/oauth/token`, `/oauth/register` and `/mcp` are normal routes,
  so the existing check covers them. Claude's servers and Claude Code send no
  `Origin`, which it allows. No change; tests confirm no-Origin works and a
  foreign Origin is refused on `/mcp`. (MCP Inspector in direct browser mode
  sends an Origin and will be refused; use its proxy mode.)

## Tests (request tests unless noted)

Discovery and auth:
- Both well-known documents return the expected JSON with `app_url`.
- `/mcp` without a token: 401 and the `WWW-Authenticate` header.
- Expired, revoked, wrong-resource tokens: 401.
- Token whose membership row was deleted, or is pending: 401 (delete the row
  directly in the test; the app has no remove-member action).

OAuth flow (end to end in one test, plus focused failures):
- register → authorize (signed out redirects to login with `next`) → consent
  Allow → code → token with correct PKCE → `/mcp` tools/call works.
- Wrong or short `code_verifier`, reused code (and its grant's token stops
  working), expired code, mismatched `redirect_uri` or `resource`:
  `invalid_grant`.
- Two concurrent token calls with the same code: one 200, one `invalid_grant`,
  and the winner's token then stops working (reuse revokes the grant). Same
  shape for two concurrent refreshes. Docs note that a client refreshing in
  parallel will have to reconnect; a grace window can come later if needed.
- Refresh with a different `client_id` or `resource`: `invalid_grant`.
- Consent page sends `X-Frame-Options: DENY` and `no-store`.
- Super admin acting in another org: consent page and PAT create are 403.
- `/oauth/register` with `token_endpoint_auth_method: client_secret_post`
  still registers, as `none`. Body over 8 KB refused.
- Refresh rotates; old refresh reused → `invalid_grant` and the new access
  token stops working (grant revoked).
- Register with a non-allowlisted redirect: 400. Loopback with any port: OK.
- Deny redirects with `access_denied`.
- `next` rejects `//evil.example`, `/\evil.example`, `/%5Cevil.example`,
  absolute URLs, and anything not under `/oauth/authorize?`; it survives a
  failed login.

Tools (via PAT, against `/mcp` with JSON-RPC):
- `tools/list` lists the eight tools.
- `create_project` succeeds (accent filled in).
- `create_task` creates in the token's org with assignees by username; the
  assignee gets a notification; the project owner gets one.
- `update_task` partial change keeps other fields; status change to blocked
  notifies the owner.
- `list_tasks` never returns another organisation's tasks; `get_task` with
  another org's id errors.
- Validation error (title too long, unknown username) returns `isError`.
- `add_task_note` with an @mention notifies the mentioned person.

Settings:
- Create a PAT: shown once, stored hashed (row has no plain value).
- Revoke own grant works and kills every token in it; revoking someone
  else's is 404.
- 10-token limit.

Unit: token generation prefix and hashing; redirect matching (the reject list
in decision 4); `next` sanitising.

Manual (before the PR): MCP Inspector (proxy mode) against local server; `claude mcp add`
with a PAT; `claude mcp add` with OAuth on localhost.

## Docs

- `docs/notes/claude-connector.md`: how it works, token lifetimes, why DCR and
  not CIMD, the redirect allowlist, how to revoke.
- README: a short "Use with Claude" section.

## Out of scope

- CIMD, `static_headers`, rate limiting on `/oauth/*` (follow-up; note it),
  PAT expiry,
  an activity log of Claude changes, deleting tasks via Claude, chat or meetings
  tools, a "via Claude" label on notifications.

## Steps for the engineer

1. Spike: add rmcp, mount an empty server at `/mcp` as a Loco route, compile,
   one `tools/list` test. Confirm `Parts` (and our extensions) reach tool
   handlers. Decide rmcp vs fallback.
2. Migration + entities + models (`oauth_clients`, `oauth_codes`,
   `access_tokens`) with token helpers and unit tests.
3. Bearer middleware + `CurrentMember` constructor + 401 behaviour tests.
4. Tools, one at a time, with tests.
5. Well-known, register, authorize (consent page), token, login `next`. Flow tests.
6. Settings page, More row, PAT create/revoke. Tests.
7. Docs. fmt, clippy `-D warnings`, full `cargo test`.
8. Manual check with MCP Inspector and Claude Code on localhost.

Report what changed, what was verified, and anything that differs from this plan.

## Backfill note (after deploy, not in the PR)

Creating Done tasks with assignees, or in a project someone else owns, sends
"assigned you" and "added to" notifications. Run the backfill as the project
owner with no assignees, or accept the noise. Decide with the user then.
