# Claude connector

Claude connects to Collab Tool over MCP (Model Context Protocol), the way it
connects to Linear. Every call acts as one person, inside their one
organisation, and can only do what that person can do in the browser.
Notifications fire exactly as they do for the web pages.

## The pieces

| Path | What |
|---|---|
| `POST /mcp` | The MCP server (rmcp 3.5, stateless, JSON responses). A normal Loco route, so Loco's middleware and the Origin check run first. GET and DELETE answer 405. rmcp's Host check runs only in development and test: in production every call needs a token, and a proxy forwarding Host differently would break every call. |
| `/.well-known/oauth-protected-resource` (and `/mcp` suffix) | RFC 9728: which server issues tokens for `/mcp`. |
| `/.well-known/oauth-authorization-server` | RFC 8414: the OAuth endpoints. |
| `POST /oauth/register` | Dynamic client registration (RFC 7591). |
| `GET/POST /oauth/authorize` | The "Allow access" page. |
| `POST /oauth/token` | Codes and refresh tokens in, tokens out. |
| `/o/{slug}/settings/claude` | Connect Claude: instructions, personal tokens, and this org's connections with Revoke. |

Code: `src/controllers/{mcp,oauth,claude}.rs`, `src/extractors/bearer.rs`,
`src/mcp/mod.rs` (the tools), `src/models/{oauth_clients,oauth_codes,access_tokens}.rs`.

## Tools

`list_projects`, `create_project`, `list_members`, `list_tasks`, `get_task`,
`create_task`, `update_task`, `add_task_note`, `delete_task`,
`delete_project`. One scope, `tasks`. No member, role, chat, meeting or admin
access. People are referred to by username; `list_members` returns usernames
and roles, never emails.

## One organisation per connection

A token is bound to one organisation when the person presses Allow. With two
or more active organisations the Allow page asks which, starting on the last
used one. The query string can't pick it. On Allow, the chosen org is checked
against the person's active memberships; anything else (not a number, never
joined, left, pending) gets one message, so the page never says whether an
org exists. The code exchange checks the membership again.

Claude is told which org at every step:

- `initialize` and `server/discover` start the server's instructions with
  "Connected to {name} ({slug}) as {username}." Both fail rather than answer
  without a member. Discovery and `tools/list` are `cacheScope: private`.
- `tools/list` adds "Connected to {name} ({slug}): pass organisation
  \"{slug}\"." to each write tool's description.
- Every successful result has `"organisation": {"name", "slug"}`. A missing
  id is "Not found in {name}."

Every write tool (the two deletes included) takes a required `organisation`
slug. `check_org` in `src/mcp/mod.rs` runs first, makes no database call,
and can only refuse: missing gets "Collab was updated. Tell the user: ...
Retry with organisation: \"{slug}\", and restart Claude to refresh its
tools."; another slug gets "This connection is {name} ({slug}), not
{other}." The given value is echoed only when it is slug-shaped. It is a
speed bump, not the guard: the guard is the picker and one org per
connection.

## What Claude made, and undoing it

Projects and tasks record the connection that made them in
`claude_grant_id` (the token's `grant_id`, the same across refreshes; no
foreign key, so it outlives pruned tokens). Web-made rows are NULL. It is
set only from the bearer token, never a form or tool argument, and never
reaches a page: templates get `made_with_claude: bool`, shown as "Made with
Claude" on the task sheet and the project dialog.

`delete_task` and `delete_project` undo the same connection's work from the
last 24 hours. Refused when anyone else has a note or a reaction to a note
on it (a note whose author was deleted counts as someone else's), and for a
project when any task in it came from the web app or another connection.
The project and then its tasks are locked `FOR UPDATE` and every check runs
on the locked rows. Notifications linking to the deleted tasks go too, and
the owners' bells refresh. Each delete is logged at info level with the
user, org, item, task count, `acting` and grant.

Any member can undo a project their own Claude connection made in the last
24 hours by asking Claude. That is deliberate: it is their own mistake, and
it can't touch anyone else's work. In the web app only owners and admins
(with their own membership, not a super admin acting) can delete a project,
from its edit dialog.

## The brake

A connection that has made 5 projects, or 60 tasks, in the last minute is
paused with a message naming the org. Per connection, so a refresh doesn't
reset it. A guard against a run in the wrong org, not a security control:
parallel calls, a second connection or waiting a minute get past it.

## The update notice (until 6 November 2026)

Connections that existed when writes started needing `organisation` have a
row in `claude_update_notices`, filled by the migration. For up to 14 days,
until the connection makes a write that names its org, every successful
result carries a `notice` asking Claude to tell the user to restart.
On the first start after the deploy, each person with such a connection got
one bell notice per org (kind `connector`, no email). A later PR can drop
the table and the code.

`update_task` takes only the fields to change, fills in the rest from the
task, and saves through `tasks::Model::update_from`, so validation lives in
one place. `assignees` replaces the whole list.

## Tokens

- Opaque random strings (32 bytes, base64url) with a readable prefix:
  `collab_pat_`, `collab_at_`, `collab_rt_`. Only the SHA-256 hash is stored.
- OAuth access tokens last 1 hour. Refresh tokens last 30 days from their
  last use and rotate every time. Codes last 10 minutes and work once.
- Personal access tokens do not expire. They show "last used" (written at most
  once a minute) and are capped at 10 live ones per person.
- Using a code twice, or a refresh token that was already swapped, revokes the
  whole grant (every token issued from that consent). Both are single atomic
  statements, so two racing requests cannot both win. A client that refreshes
  in parallel will have to reconnect; a grace window can come later if needed.
- Every request checks that the person is still an active member of the
  token's organisation. Super admins get no extra power through tokens, and
  cannot connect Claude while acting inside another organisation: the Allow
  page and the token form ask them to leave first.
- Tokens are bound to the resource `{app_url}/mcp`. **Changing `APP_URL`
  invalidates every token**; everyone reconnects.
- Revoked and expired rows are deleted 30 days later, and codes after a day,
  on each `/oauth/token` call. Clients that never got a code are deleted after
  7 days, on each registration. A client that got a code is kept, because
  claude.ai may cache its client id.

## Why DCR and not CIMD

Claude registers itself with Dynamic Client Registration. Client ID Metadata
Documents (CIMD) would make our server fetch arbitrary client URLs, which is
an SSRF risk for no gain: Claude falls back to DCR when CIMD is not
advertised. Revisit if Claude drops DCR.

Registration always makes a public client (`token_endpoint_auth_method:
none`, PKCE S256 required), whatever the request asks for. Bodies are capped
at 8 KB, names at 100 characters, and 5 redirect URIs.

## Redirect allowlist

`/oauth/register` and `/oauth/authorize` only accept these redirect URIs,
matched on the parsed URL (`oauth_clients::HOSTED_CALLBACKS`):

- `https://claude.ai/api/mcp/auth_callback`
- `https://claude.com/api/mcp/auth_callback`
- `http://localhost:<any port>/callback` and `http://127.0.0.1:<any port>/callback`
  (Claude Code). Loopback ones match on any port at authorize time (RFC 8252).

No userinfo, query or fragment; the raw text must already be in normal form.
This is a private team tool, so the allowlist removes the phishing risk of
open registration.

## The Allow page

Sent with `X-Frame-Options: DENY`, `Content-Security-Policy: frame-ancestors
'none'` and `Cache-Control: no-store`. Signed-out people go to
`/login?next=…` and come back; `next` only accepts `/oauth/authorize?…`.
Unknown clients and redirect URIs get an error page, never a redirect.

## Revoking

More → Connect Claude lists each connection once (a connector's rotated
tokens are grouped). Revoke ends every token in it. Reuse detection revokes
the same way.

## Testing locally

- MCP Inspector: use its proxy mode. In direct browser mode it sends an
  `Origin` header, which the Origin check refuses.
- Claude Code with a personal token:
  `claude mcp add --transport http collab http://localhost:5150/mcp --header "Authorization: Bearer collab_pat_…"`.

## Not done yet

Rate limiting on `/oauth/*`, CIMD, personal token expiry, an activity log of
changes made through Claude, a "via Claude" label on notifications, holding
notifications for Claude-made items, a bin with restore, chat or meeting
tools.
