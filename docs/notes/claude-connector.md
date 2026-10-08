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
| `/settings/claude` | Connect Claude: instructions, personal tokens, Revoke. |

Code: `src/controllers/{mcp,oauth,claude}.rs`, `src/extractors/bearer.rs`,
`src/mcp/mod.rs` (the tools), `src/models/{oauth_clients,oauth_codes,access_tokens}.rs`.

## Tools

`list_projects`, `create_project`, `list_members`, `list_tasks`, `get_task`,
`create_task`, `update_task`, `add_task_note`. One scope, `tasks`. No member,
role, chat, meeting or admin access, and no delete. People are referred to by
username; `list_members` returns usernames and roles, never emails.

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
changes made through Claude, a "via Claude" label on notifications, deleting
tasks, chat or meeting tools.
