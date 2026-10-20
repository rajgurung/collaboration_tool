# Organisation in the address 1: routes, links and bare addresses

Status: Reviewed; waiting on Raj (staff: approve with changes, applied; security: approve with changes, applied) · PR 1 of 3 · Owner: Raj · 20 October 2026

Brief: `docs/briefs/org-in-the-address.md`, approved by Raj on 20 October with
all 9 questions as recommended. Mockup: `docs/designs/org-in-the-address.html`,
approved by Raj on 20 October with all 12 questions as recommended. Section
numbers below (§1 to §9) are the mockup's. Visuals and copy are the
mockup's; this plan covers what the server and `app.ts` must do.

The production host is `collab.zen-scale.io` (made permanent on 20 October).

## Summary

Every app page moves to `/o/{slug}/...`, for example
`/o/acme-studio/tasks/58`. The address picks the organisation for that
request. The `current_org` cookie stops choosing the organisation and only
remembers the last one, for bare addresses like `/tasks`. Each tab then holds
its own organisation.

**Routing: a layer that strips the prefix.** One new layer, outside the whole
router, turns `/o/acme-studio/tasks/58` into `/tasks/58` before routing and
records `acme-studio` in a request extension. Every route, handler and `Path`
extractor stays as it is. The 49 `Path(...)` extractors would all break with
a Loco prefix or an axum `nest`, because the slug becomes a second path
parameter.

**Resolution: the address, checked per request.** `CurrentMember` reads the
slug and loads your membership there in one query. Active, or a super admin
acting in that organisation: you're in. Pending, removed or declined: that
organisation's status page (§3). No row, or no such organisation: the same
plain not-found page (§4), from the same single query. The page claim
(`X-Org-Id` or `_org`) is now compared with the address's organisation.

**Bare addresses.** A bare page load (`/tasks`, an old email's
`/tasks?open=58`) redirects to `/o/{slug}/` plus the same path and query. An
old item link follows the item only into an org where you're active (or
acting). Anything else bare (a POST, an HTMX call, a `fetch`, the socket, the
stream) is never redirected. It is refused and the tab goes to Home, so a
write can never be replayed into the last-used organisation.

**Every link through one helper.** `org_url(path=...)` in templates,
`member.url(...)` and `member.fragment(...)` in Rust, `orgUrl(...)` in
`app.ts`. A test walks the rendered pages and write responses and fails on
any app link without the prefix. A second test greps the sources.

**The switcher becomes links** (§2). Each row links to the same section in
that organisation, so Cmd-click and long-press open it in a new tab. Nothing
closes live connections any more, since each tab has its own.

**Three sub-PRs** (decision 1), each safe to ship alone:

| Sub-PR | What | Visible |
|---|---|---|
| 1a. Addresses resolve | The layer, resolution by slug, the remember cookie, the link helpers, the status pages (§3) and plain not-found (§4), `robots.txt`, `X-Robots-Tag`, `Referrer-Policy`, the rollback stub. Bare addresses keep today's cookie behaviour. | The status pages and the plain not-found page. Nobody lands on `/o/` unless they type it. |
| 1b. Links carry the org | Every template, Rust redirect, `app.ts` URL, socket and stream. The switcher as links. The phone and task-sheet org lines. Home notice and banner copy. The link tests. | Addresses change. Side-by-side tabs work. |
| 1c. Bare addresses redirect | Bare page loads redirect (with the old-item lookup). Everything else bare is refused. The in-org 404 copy. The test migration. | Old bookmarks and email links land under `/o/`. |

No migration. No new crate.

## What changes, file by file

### 1. Routing (1a)

**Decision: a layer that strips `/o/{slug}` before routing.**

| Option | Cost | Verdict |
|---|---|---|
| `AppRoutes::nest_routes("/o/{slug}", ...)` (Loco prefix) | The slug becomes a path parameter. Every `Path(id): Path<i64>` (49 in 14 controllers) fails with "wrong number of parameters" and must become a tuple or struct. Bare routes need registering a second time for the redirects. | Rejected |
| `axum::Router::nest("/o/{slug}", tenant)` | The same: nested captures reach the inner `Path` extractors. | Rejected |
| A layer that strips the prefix and records the org | No handler changes. Routes stay bare in `routes()`, so `cargo loco routes` and the route walk tests keep their shape. | Chosen |

**Why it must wrap the whole router.** A layer added with `Router::layer` runs
after routing, so rewriting the path there changes nothing (axum 0.8 docs,
"Rewriting request URI in middleware"). Loco builds the final router in
`setup_routes` (`loco-rs` 1.2.0 `boot.rs:539`): routes, then
`Hooks::after_routes`, then each initializer's `after_routes` in list order.
Both `serve` (`app.rs:481`) and the test harness (`testing/request.rs:266`)
take that router as it is. So the new initializer is registered last and
returns `AxumRouter::new().fallback_service(strip.layer(router))`. The outer
router has no routes, so the whole inner router, with every Loco middleware
and our other layers, runs on the rewritten path.

**One slug shape.** New `src/views/links.rs::SLUG_MAX = 60` and
`is_slug(s)`: 1 to `SLUG_MAX` bytes, each `a`-`z`, `0`-`9` or `-`, checked
byte by byte. No lowercasing, no decoding. The strip, `join_page`
(`src/controllers/auth.rs:84`, 1 to 60 today) and `org_path` all call it.
`slugify` truncates so that the base plus a `-N` suffix stays within
`SLUG_MAX` (today it takes 40 and adds the suffix after).

**`src/initializers/org_path.rs` (new).** For each request, working only on
`uri.path()` and `uri.query()`:

1. **`/o/{slug}` or `/o/{slug}/` exactly, GET or HEAD, valid slug:** 303 to
   `/o/{slug}/dashboard`. Other methods fall to step 3.
2. **`/o/{slug}/{rest}`**, where the slug is valid and the first segment of
   `{rest}` (cut at the next `/` or the end) equals a name in
   `TENANT_SECTIONS` exactly: `dashboard`, `tasks`, `chat`, `documents`,
   `members`, `more`, `notifications`, `roadmap`, `meetings`, `settings`,
   `guide`, `orgs`, and no segment of `{rest}` is `.`, `..` or their
   percent-encoded forms. Then:
   - The new path is `/{rest}`. The new URI is built with `Uri::from_parts`
     from the old parts, so scheme and authority survive (they are set on
     HTTP/2), with only `path_and_query` replaced by `/{rest}` plus `?` and
     the original query.
   - The request is changed in place (`*req.uri_mut() = new`), so its
     extensions, including `OnUpgrade` and `ConnectInfo`, are untouched.
   - It inserts `PathOrg { slug }` and axum's `OriginalUri` (the full
     address), so the router doesn't replace it with the stripped one.
   - It records the slug on the request's tracing span. Loco's logger logs
     the stripped path, so without this the logs can't tell orgs apart.
3. **Anything else** passes through untouched with no `PathOrg`: no route
   matches `/o/...`, so it is a 404 drawn by `not_found_page` as today, the
   same for every slug.

Any failure to build the URI or a header value is a 400. No `unwrap`, no
panic. The layer strips once and never reads the body.

How the rules treat the inputs the review named (each a unit test):

| Input | Result |
|---|---|
| `/o/acme/tasks` | `/tasks`, `PathOrg(acme)` |
| `/o/acme/tasks;x` | Segment is `tasks;x`, not a section: untouched, 404 |
| `/o/acme/tasksx` | Not a section: 404 |
| `/o/acme/tasks%2F..%2Fadmin` | Segment `tasks%2F..%2Fadmin`: 404 |
| `/o/acme//tasks` | Empty first segment: 404 |
| `/o/acme/./tasks` | Segment `.`: 404 |
| `/o/acme/tasks/../../admin` | Any `.` or `..` segment anywhere in `{rest}` (raw or as `%2E`): untouched, 404. It never reaches `/admin`. |
| `/o/Acme/tasks` | Bad slug: 404 |
| `/o/acme/o/beta/tasks` | Segment `o`: 404 |
| Absolute form `http://collab.zen-scale.io/o/acme/tasks` | Path and query are read from the URI parts, so the same as `/o/acme/tasks`. Scheme and authority are kept. |
| `/o/acme` (POST) | Untouched: 404 |

**Response headers, in the same layer, on every response:**

- `Referrer-Policy: same-origin` (section 9).
- `X-Robots-Tag: noindex`, except for `GET /` (the homepage).

**Rollback stub, written in 1a** (`org_path::revert_layer`, unused). The tiny
layer a revert would keep: GET or HEAD for `/o/{slug}/{rest}` (valid slug)
answers 303 to `/{rest}` plus its query, built from `uri.path()` and
`uri.query()`; anything else under `/o/` is a 404. No database, no cookie.
It has its own unit test now, so a revert under pressure doesn't need new
code. See "Rollback".

**`src/app.rs`.** Register `OrgPathInitializer` last, with a comment: it must
be outermost because routing happens inside it.

**Path parameters, HTMX, uploads, the socket and the stream** work unchanged
after the strip:

- `/o/acme/tasks/58` routes to `/tasks/{id}` with `Path(58)`.
  `/o/acme/chat/12/threads/9` routes to `/chat/{id}/threads/{message_id}`.
- `/o/acme/documents/folders/{id}/files` keeps its route-level
  `DefaultBodyLimit::max(UPLOAD_BODY_LIMIT)` and the `no_framing` layer.
- `/o/acme/chat/{id}/ws`: the upgrade's `OnUpgrade` stays in the extensions,
  since the request is changed in place. Proved on the real server
  (`tests/requests/live_sign_out.rs`) and by hand on Railway.
- `/o/acme/notifications/stream`: an ordinary GET.

### 2. Resolution (1a)

`src/extractors/current_member.rs`. `resolve_org` gets a second mode. With
`PathOrg` it resolves from the address. Without it, it keeps today's order
(acting, then `current_org`, then the default) until 1c changes bare
addresses.

**`PathOrg` is read only here, and by the switch's 404** (section 5). A
source test (`only_the_resolver_reads_path_org`) greps `src/` and fails on
any other use.

**Address mode, in order:**

1. `CurrentUser`. Signed out: `/login`, as today. The same for every slug,
   real or not.
2. **Acting.** Only for a super admin with an `acting_org` cookie that parses
   as an `i64`: `organisations::find_by_id`. If its slug equals the address's
   slug, the member is acting there, exactly as today (owner role, the
   banner). Acting wins over their own membership in that org, which is
   today's order. Acting in X never touches `/o/a/...` for their own org A,
   which works normally in another tab. So the banner shows only on X's
   pages (brief question 7).
3. **One query:** new `memberships::Model::in_org_by_slug(db, slug,
   user_id) -> Option<(memberships::Model, organisations::Model)>`. It joins
   `memberships` to `organisations` on `organisations.slug = $1 AND
   memberships.user_id = $2`, any status. It uses the unique slug index and
   `idx-memberships-org-user`. This replaces today's `active_in` plus
   `find_by_id`, so it is one query fewer per request.
4. **No row** (no such organisation, or not a member):
   - A page load gets `Error::NotFound`. `not_found_page` draws the plain
     not-found page (§4, below).
   - HTMX and `fetch` (the `wants_hx_redirect` test) get 409 with
     `HX-Redirect: /dashboard` and `no-store`.
   - Both cases make the same single query and get byte-identical responses,
     `Set-Cookie` and `Cache-Control` included. The only timing difference is
     whether the slug index finds a row before the membership lookup misses.
     That is microseconds, well under network jitter. Not measured; the
     engineer logs both locally.
5. **Active row:** the member. `org_menu` as today.
6. **Row not active** (pending, removed, declined):
   - A super admin goes to `/admin`, as today (PR 6's rule).
   - A page load gets that org's `tenancy/waiting.html`, `removed.html` or
     `declined.html` with 403, built to §3: icon tiles; "Open {org}" first
     (your last active org), "Check again" second, "Sign out" as a quiet
     foot link. With two or more other active orgs, a "Your other
     organisations" list: the last used first, then A to Z, each with its
     role. The data is one query, only on this path: all your active
     memberships with their orgs (`for_user_with_orgs`, active only), ordered
     with the `current_org` cookie's org first.
   - HTMX and `fetch` get 409, `HX-Redirect: /o/{slug}/dashboard`,
     `no-store` and `X-Org-Gone: 1`. The tab lands on that status page.
     `app.ts` keeps typed text behind the gone banner, as in PR 5. A distinct
     answer here reveals nothing: the row is the person's own.
7. **The claim check** (`CurrentMember` only, not `PinnedMember`): as today,
   compared with the address's org. A refusal goes to
   `/o/{address slug}/dashboard?from=<claim>`, 409 or 303 as today. Only a
   page drawn for one org that posts to another org's address gets here,
   which means a bug or a forged request. `removed_from` and `X-Org-Gone` are
   unchanged.

**The plain not-found page (§4).** `not_found_page` (`src/app.rs`) gets a
third branch. Today it draws the in-app page when `PinnedMember` resolves,
and the public page when it fails. New: when `PinnedMember` fails but
`CurrentUser` resolves, it draws `errors/not_found_signed_in.html`: "We
couldn't find that page", "Signed in as {email}", "Use another account"
(sign out, then `/login`) and "Go to Home" (bare `/dashboard`). It reads only
the user, never the slug or the org, so a stranger's org and a made-up slug
still look identical. The identical-response test covers it.

**The in-org 404 (§4)** stays inside the org's shell. Its button becomes
"Back to {section}", linking to `org_url(path=section_path(active))`, or "Go
to Home" when the page has no section. The "It may be in another of your
organisations" line stays until 1c (section 8).

**Remembering the last org.** On a page load (GET or HEAD, not HTMX) in
address mode, own membership (not acting), when the request's `current_org`
cookie isn't that org's id, set it to that id, but only when
`Sec-Fetch-Site` is `same-origin` or `none`, or the header is missing.
`none` is a typed address or a bookmark. A navigation started by another
site (`cross-site` or `same-site`), including a redirect chain it started,
never moves the cookie. HTMX, `fetch`, POSTs and acting never change it.

- An extractor can't set cookies. `src/initializers/current_org.rs` already
  puts a `StaleCurrentOrg` flag in every request and clears the cookie when
  it is raised. It becomes a small `OrgCookie` slot: nothing, clear, or
  remember `id`. The layer appends the matching `Set-Cookie` from
  `current_org_cookie` or `cleared_current_org_cookie`. Same cookie rules as
  PR 2: HttpOnly, `SameSite=Lax`, `Path=/`, 7 days.
- Address mode never raises "clear".

**`PinnedMember`** resolves the same way and skips the claim check.

- With `PathOrg`, it uses the address. `POST /o/{slug}/settings/claude/tokens`
  compares its hidden `org_id` pin with the address's org.
- Without `PathOrg`, the last-org mode is allowed only for `/oauth/authorize`
  (GET and POST), which stays bare until PR 3's picker. It checks the
  request's path for exactly that route. Every other bare `PinnedMember`
  request, including `POST /settings/claude/tokens`, gets the 1c bare refusal
  and writes nothing (in 1a and 1b it keeps today's cookie mode).
- Its uses become three: the Allow page (GET `authorize_page`, new in 1c,
  section 3), the Allow page POST, and the token form. `not_found_page`
  calls it too but never as a handler. The list in `stale_tab.rs` and the
  `PinnedMember` doc comment are updated to match.

**`CurrentMember` helpers.**

- `url(&self, path) -> String`, through `org_path`.
- `page()` adds `"org_base": "/o/{slug}"` next to `current_org_id`, and
  `"multi_org": bool` for the phone org line (section 7). `multi_org` is
  worked out with `switcher_counts` (active at least 2) on page loads only,
  the same place as `org_menu`, so HTMX and POSTs pay nothing.
- New `fragment(&self, json) -> json`: adds `org_base` (and nothing else) to
  a fragment's data. For HTMX fragments and write responses that don't use
  `page()`.

### 3. Bare addresses (1c)

**The last org** is today's resolution order, kept as one function: a valid
`acting_org` for a super admin, then a `current_org` you're active in, then
`default_for_user` in any status. No membership: a super admin goes to
`/admin`; anyone else gets `tenancy/no_org.html`, as today. It is used for
bare addresses, sign-in landing (`/` then `/dashboard`) and `PinnedMember` on
`/oauth/authorize`.

**A bare page load** is GET or HEAD, with no `HX-Request`, no `X-Org-Id`, no
`Upgrade` header, and an `Accept` that doesn't ask for `text/event-stream`.
`CurrentMember` (bare mode) answers it with 303 and `no-store`:

```
/o/{target slug}{uri.path()}{?uri.query()}
```

- Built from `uri.path()` and `uri.query()` only, through `org_path`. The
  result always starts with `/o/{slug}/`, so it is same-origin whatever the
  path or query holds (`/chat/%2F%2Fevil.com` and `/tasks?x=//evil.com` are
  unit tests). The `http` crate rejects raw CR and LF. A header value that
  fails to build is a 400.
- The target is the item's org when the path names an item and you qualify
  there (below). Otherwise the last org.

**The old-item lookup: active rows only, one query.**
`src/views/links.rs::old_link_item(path, query) -> Option<(Item, i64)>` is a
pure parse (no database): it matches path segments and returns which table
and which id. The extractor then runs one query, `item_org_for(db, item, id,
user_id, acting_org)`:

```sql
SELECT t.organisation_id
FROM <table> t
JOIN memberships m
  ON m.organisation_id = t.organisation_id
 AND m.user_id = $2 AND m.status = 'active'
WHERE t.id = $1
UNION ALL
SELECT t.organisation_id FROM <table> t
WHERE t.id = $1 AND t.organisation_id = $3   -- only when acting; $3 = acting org
LIMIT 1
```

- It returns an org only where you are active, or acting. It can never
  return a foreign org id. A missing id and an id in someone else's org run
  the same query and return the same nothing, so the timing doesn't depend
  on whether the id exists anywhere.
- The query lives in the extractor's module (`current_member.rs`), next to
  the resolver, since only it calls it. The table name comes from the `Item`
  enum, never from the request.
- No row: the last org. There the page 404s like any missing id, inside the
  last org's shell (decision 12).
- An item in an org where you're pending, removed or declined follows the
  same rule: the last org. The status pages (decision 7) are only for
  `/o/{slug}/` addresses, which name the org themselves.

**Id-bearing bare paths:**

| Bare page load | Item | Table |
|---|---|---|
| `/tasks/{id}`, `/tasks/{id}/edit` | task | `tasks` |
| `/tasks?open={id}` (mention emails, bell) | task | `tasks` |
| `/tasks/views/{id}/edit` | saved view | `saved_views` |
| `/chat/{id}` (mention emails, bell) | conversation | `conversations` |
| `/chat?c={id}` (the legacy query, `chat.rs:504-508`) | conversation | `conversations` |
| `/chat/{id}/threads/{message_id}` (thread emails, bell) | conversation | `conversations` |
| `/documents/folders/{id}` | folder | `folders` |
| `/documents/files/{id}` (downloads) | document | `documents` |
| `/documents/logins/{id}` | login | `logins` |
| `/members/{id}/rotate` | membership | `memberships` |
| `/roadmap/projects/{id}/edit` | project | `projects` |

The other id-bearing tenant routes are never page loads, so they never
redirect: `/chat/{id}/feed`, `/chat/{id}/ws`,
`/chat/{id}/messages/{message_id}/actions`,
`/chat/{id}/threads/{message_id}/feed`, and every POST. `/roadmap?lane=` and
`/notifications` carry no id and go to the last org, as today.

`chat::index`'s own redirect for `?c=` (`format!("/chat/{id}")`, line 507)
becomes `member.url(...)`.

**Anything else bare is refused, never redirected.** POST, PUT, DELETE, an
HTMX or `fetch` GET, the socket upgrade, the stream:

- HTMX or `X-Org-Id` (raw `fetch`): 409 with `HX-Redirect`. A plain form:
  303 with `Location`. Never 307 or 308, which would repeat the body.
- **Where it lands** (mockup Q8, §5): Home in the org the tab was showing,
  when you're still active there.
  - Claim `Org(id)` and `in_org_for_user(id, user)` is active: `/o/{that
    slug}/dashboard?from=`. Home shows "Your last change wasn't saved."
  - Claim `Org(id)` and your row there is removed, declined or pending:
    `/o/{last slug}/dashboard?from=<id>` with `X-Org-Gone: 1`. Home shows
    the removed notice, as in PR 5.
  - Anything else (no row, missing, garbage): `/o/{last slug}/dashboard?from=`.
  - One query, only on this path, and only for the person's own row. Only a
    slug from the database and a parsed `i64` go into the address.
- `no-store`. One info log line, numbers only: user id, claimed org,
  method.
- A bare socket upgrade gets the 303. The handshake fails, and the old page's
  `app.js` probes on `htmx:wsClose` and moves. A bare `EventSource` follows
  the 303 to an HTML page, fails, ends `CLOSED`, and the old page's probe
  moves it.

Who sends bare requests after 1b: tabs drawn before the deploy, and any link
1b missed. Both are refused once, with nothing saved.

**`src/controllers/oauth.rs`.** `authorize_page` takes
`Result<PinnedMember, Response>` instead of `Result<CurrentMember, Response>`.
Without this, 1c would redirect the Allow page to `/o/{last}/oauth/...`,
which is a 404. For a non-HTMX GET the claim check never ran anyway.

**`src/controllers/page.rs`** keeps sending a signed-in visitor at `/` to
`/dashboard`, which then redirects. Two hops after sign-in. Accepted.

**Email links in PR 1.** Mailers are PR 2. After 1c, for a signed-in reader:

| Email link | Lands on |
|---|---|
| Mention: `/tasks?open=58`, `/chat/12`, `/chat/12/threads/9` | The item's org, if you're active there |
| Mention: `/roadmap?lane=now`, "all notifications" `/notifications` | The last org, as today |
| Security: `/documents/activity` | The last org, as today (PR 2 fixes it) |
| Approval: `/dashboard` | The last org, as today |
| Forgot, reset | Unchanged, not tenant routes |

Signed out, every link goes to `/login` without `next` and then the default
org, as today. Nothing regresses.

**For the engineer:** after 1c, a test that still sends a bare tenant GET
with `.as_member(...)` gets a 409, not a page, because `as_member` adds
`X-Org-Id`, which makes the request a `fetch` rather than a page load. A
failing test with 409 means "not migrated yet".

### 4. Every link (1b)

**One helper, three spellings.**

| Where | Helper | Example |
|---|---|---|
| Rust | `src/views/links.rs::org_path(slug, path)`, `CurrentMember::url(path)`, `CurrentMember::fragment(json)` | `redirect_response(&headers, &member.url("/tasks"))` |
| Templates | Tera function `org_url(path=...)`, registered in `src/initializers/view_engine.rs` next to `asset_url` | `href="{{ org_url(path="/tasks/" ~ task.id) }}"` |
| `frontend/app.ts` | `orgUrl(path)`, from `document.body.dataset.orgBase` | `new EventSource(orgUrl("/notifications/stream"))` |

- **`org_path` checks at runtime and fails closed.** The slug must pass
  `is_slug`, and `path` must start with `/` and not with `/o/` or `//`. On
  failure it returns `/dashboard` and logs an error. Never a link built from
  a bad slug. A unit test covers each case.
- `org_url` reads the top-level `org_base` through `tera::State::get`. If it
  is missing it returns an error, so a template rendered without it fails
  loudly in tests. **Unverified:** whether a Tera 2 component sees the
  caller's context through `State` (`component_parent` in
  `tera-2.4.0/src/vm/state.rs` suggests it does). If not, components take
  `org_base` as a prop.
- `base.html` adds `data-org-base="/o/{{ org.slug }}"` on `<body>` next to
  `data-org-id`, under the same `current_org_id` condition. It also adds
  `data-multi-org` when `multi_org` is true (section 7).
- Paths stored or built in models stay org-relative and get the prefix when
  shown: `notifications.link`, `guide.rs` step `href`s, `saved_views::url()`.
  Each row is already scoped to `member.org.id`. No data migration.

**Fragments drawn by writes and by the socket.** Every tenant `.view(...)`
gets its data from `member.page(...)` or `member.fragment(...)`. The chat
socket renders messages for broadcast without a request
(`chat_ws.rs:451-464`: `_thread_link.html`, `_message_oob.html`,
`_thread_count.html`, and the other `session.view.render` calls). Its
`Session` gains `org_slug`, set at upgrade from `member.org.slug`, and those
calls add `org_base` from it. Every subscriber to a conversation is in its
org, so one base is right for all of them.

**Finding them.** The counts are from the brief and a grep on this branch:

| Where | About | How |
|---|---|---|
| Templates, 64 files | 187 (178 under a tenant section by the grep below) | `grep -rnoE '(href\|action\|formaction\|hx-(get\|post\|put\|patch\|delete)\|ws-connect\|data-[a-z-]*url)="/(dashboard\|tasks\|chat\|documents\|members\|more\|notifications\|roadmap\|meetings\|settings\|guide\|orgs)' assets/views` |
| Rust, 25 files | 77 (about 60 outside mailers and tests) | `"/<section>` literals and `format!("/<section>`, minus `routes()` and `.prefix(` lines |
| `frontend/app.ts` | 14 | `"/<section>` and `` `/<section> `` literals, plus `"/dashboard"` fallbacks and the `[hx-get^="/notifications/unread"]` selector (line 660) |

The engineer turns the grep output into a checklist and converts one area
per commit: layouts, dashboard, tasks and saved views, chat (including the
socket), documents and logins, members, roadmap and meetings, notifications,
more, settings and guide, orgs, `app.ts`.

**Rust call sites that change** (from the grep):

| File | Lines |
|---|---|
| `src/controllers/tasks.rs` | redirects to `/tasks` (749, 814, 841, 1057, 1081) |
| `src/controllers/chat.rs` | `thread_url` (148), redirects (507, 682, 1038, 1071) |
| `src/controllers/chat_ws.rs` | `Session.org_slug`; broadcast renders (451-464 and the rest) |
| `src/controllers/documents.rs` | 295, 317, 336-337, 368 |
| `src/controllers/logins.rs` | form actions and redirects (84, 175, 274, 300, 325) |
| `src/controllers/members.rs` | 245, 283, 396 |
| `src/controllers/roadmap.rs`, `meetings.rs`, `guide.rs`, `claude.rs` | 268; 101; 21, 42; 155 |
| `src/controllers/notifications.rs` | `open` (496, 508) and `read_all`. The links written at 105, 176, 255, 292, 356 stay org-relative. |
| `src/controllers/orgs.rs` | `AFTER_CREATE`, the compat switch, see section 5 |
| `src/controllers/admin.rs` | `enter` (78): `/o/{slug}/dashboard` |
| `src/controllers/join.rs` | 119 `/o/{last}/dashboard?requested=`; 193 `/o/{slug}/dashboard`; 79 stays bare (the new account is pending) |
| `src/controllers/signup.rs` | 44: `/o/{new slug}/dashboard`; 25 stays bare |
| `src/controllers/auth.rs` | 229 `/more?signed_out=1` stays bare (account route, last org); 420 stays bare |
| `src/extractors/current_member.rs` | the refusal targets (392-393, 421), as in sections 2 and 3 |
| `src/controllers/page.rs` | stays bare |
| `src/mailers/*` | unchanged until PR 2 |

**Test 1, the rendered walk** (`tests/requests/links.rs`,
`every_app_page_links_only_inside_its_org`).

- Seed one of everything: project, task with a note and a reaction, saved
  view, group, DM, thread, folder, file, login, meeting, notification of
  each kind, a pending member for Members.
- GET each page and fragment as a member of Acme: Home, Notifications, Tasks
  (list, board, a task, edit, new), saved view edit, Chat (list, a
  conversation, a thread, feed, message actions, new group, new DM),
  Documents (index, folder, login, activity), Roadmap (index, list, project
  edit, new), Meetings (index, new), Members (index, rotate), More,
  `/settings/claude`, `/orgs` (page and fragment), `/orgs/new`, the 404
  page.
- **And the HTML each write returns:** send a chat message, react to a
  message, reply in a thread, add a task note, react to a note, change a
  task's status, create and edit a task, save a view, create a folder, add a
  login, rename a file, approve a member, change a role, create a project,
  create a meeting. Each one over HTMX, reading its response body.
- **And the socket's broadcasts:** on the live server, read the frames a
  second member's socket receives for a message, a reply and a reaction.
- Pull every `href`, `action`, `formaction`, `hx-get|post|put|patch|delete`,
  `ws-connect`, `sse-connect` and `data-*url` value. Each one that starts
  with `/` must start with `/o/acme-studio/`, or be on a short allowlist:
  `/static/`, `/login`, `/logout`, `/account/sign-out-everywhere`, `/admin`,
  `/admin/leave`, `/join/`, `/oauth/`, `/`. Also every `Location` and
  `HX-Redirect` the walk sees.

**Test 2, the source grep** (`no_hand_built_tenant_paths`). Reads
`assets/views/**/*.html`, `src/**/*.rs` and `frontend/app.ts`. Fails on:

- a string starting with `/<tenant section>` unless, in a template, it is
  inside `org_url(path=`; in Rust, it is on a `.add(` or `.prefix(` line in
  `routes()`, in the old-link parser, inside `org_path(`, `.url(`, or a
  model's org-relative link, or in `src/mailers/` (allowlisted with "PR 2");
  in `app.ts`, inside `orgUrl(`;
- any `.view(` or `.render(` call in a tenant controller (every controller
  but `auth`, `signup`, `join`, `oauth`, `admin`, `page`, `mcp`) whose data
  is a bare `data!(` or `json!(` rather than `member.page(` or
  `member.fragment(`. The socket's renders are listed as the one exception,
  with `org_base` from `Session`.

It is a heuristic, so the walk is the real proof. It catches templates and
writes the walk doesn't reach.

### 5. Public routes, `/orgs` and the switch

**Stay bare** (brief question 6): `/`, `/login`, `/signup`, `/forgot`,
`/reset/{token}`, `/join/{slug}` and `/join/{slug}/request`, `/oauth/*`,
`/.well-known/*`, `/mcp`, `/admin` and `/admin/*`, `/logout`,
`/account/sign-out-everywhere`, `/_health`, `/_ping`, `/_readiness`, and the
new `/robots.txt`. None of them use `CurrentMember`. Under `/o/{slug}/` they
are 404 (section 1). A walk test pins both.

**`/orgs` and friends** (decisions 2 and 3, §2's table):

| Route | Recommend | Why |
|---|---|---|
| `GET /orgs` (the switcher list) | `/o/{slug}/orgs` | It ticks the current org and builds same-section links. Both come from the address. |
| `GET /orgs/new`, `POST /orgs` | `/o/{slug}/orgs/new`, `POST /o/{slug}/orgs` | PR 4's reviewers made both `CurrentMember`: drawn in the app layout, the new org copies this org's time zone, and the claim check covers the POST. A bare `GET /orgs/new` still works through the 1c redirect. |
| `GET /orgs/current` (the probe) | `/o/{slug}/orgs/current` | It must check this tab's org, through the normal address mode. |
| `POST /orgs/{id}/switch` | Bare, for one release, as a thin redirect | Tabs drawn before the deploy still have switch buttons and the stale banner's "Back" `fetch`. It checks `active_in`, sets the remember cookie, clears `acting_org`, and answers `/o/{slug}/dashboard` (`HX-Redirect` over HTMX). It no longer ends an unlock or closes connections. With `PathOrg` present it is a 404, before anything else. Removed in PR 3. |

**PR 2's rule, restated for the security reviewer.** PR 2 said "never add a
GET that changes the org", because the cookie chose the org for every tab.
Now the address chooses it for one request. The rule becomes:

1. The address grants nothing. Access is `in_org_by_slug(slug, user)` active,
   or a super admin acting in that org, checked on every request.
2. A GET may update `current_org` only when the person started the
   navigation on this site or by hand (`Sec-Fetch-Site` is `same-origin` or
   `none`, or the header is missing). The cookie only says where bare
   addresses and sign-in landing go. It never affects a request with
   `/o/{slug}/`, so no tab's organisation changes because of another tab, a
   link or an email.
3. No GET starts or ends acting, ends an unlock, closes connections or writes
   data. Acting starts only from `POST /admin/orgs/{id}/enter`.
4. A bare request that isn't a page load is refused, never redirected or
   replayed.
5. Every redirect target is `/o/` plus a slug from the database plus the
   request's own path and query, so it is always same-origin.

**The cost of rule 2, and the 1a edge cases.**

- A link from another site to `/o/acme/tasks` opens Acme in that tab but
  doesn't move the cookie. Neither does a redirect chain another site starts.
- A browser that sends no `Sec-Fetch-Site` (old or unusual browsers) is
  treated as same-origin. A cross-site link there can move the cookie to an
  org you're active in, and your next bare bookmark opens that org. Nothing
  is read or written. Accepted.
- A super admin acting in X who types `/o/x/tasks` gets the acting page and
  no cookie change (acting is never remembered). Typing `/o/a/tasks` for
  their own org A is remembered as usual, and acting in X's tabs carries on.
- In 1a nothing links to `/o/` yet, so only typed addresses move the cookie.
  A bare tab whose cookie moves that way is then a stale tab, and PR 3's
  check refuses its next write, as today.

### 6. The switcher (1b, §2)

- **Rows are links.** Each other active row is `<a href="/o/{slug}{section}"
  role="menuitemradio">`. The current row stays a button with a tick and
  closes the menu, as today. Pending rows stay disabled. "New organisation"
  links to `/o/{this slug}/orgs/new`. The switch `<form>` in
  `orgs/_list.html` goes.
- **The same section.** `src/views/links.rs::section_path(active)` maps the
  page's `active` key to a fixed list path, with no query (§2: drop the
  filters):

  | `active` | Goes to |
  |---|---|
  | `dashboard` | `/dashboard` |
  | `notifications` | `/notifications` |
  | `tasks` (list, a task, edit, saved views) | `/tasks` |
  | `chat` (list, a conversation, a thread, new) | `/chat` |
  | `roadmap` (and project edit) | `/roadmap` |
  | `meetings` | `/meetings` |
  | `documents` (folders, files, logins, activity) | `/documents` |
  | `members` (and rotate) | `/members` |
  | `more` | `/more` |
  | the `/settings/claude` page's key | `/settings/claude` |
  | anything else (`orgs`, `not_found`, ...) | `/dashboard` |

  The engineer confirms each page's `active` key, and that every list path
  opens for every role. A section that needs a role the row's member lacks
  goes to `/dashboard`. The same table gives the in-org 404's "Back to
  {section}" (section 2).
- **How the list learns the section.** The trigger in `app.html` becomes
  `href="/o/{slug}/orgs?section={{ active }}"`. `app.ts`'s `loadOrgs`
  (`app.ts:1863`) fetches that `href`. The server only maps `section`
  through the table, so a hand-made value gives `/dashboard`.
- **A plain click** (no Cmd, Ctrl, Shift or Alt, left button): `app.ts`
  shows the row's spinner and "Opening…", sets a `sessionStorage` flag with
  the row's org name (try/catch; nothing if storage is blocked), and lets
  the link navigate. On the next page, if the flag is set, it shows the toast
  "You're now in {org}" from the page's own `org.name` and clears the flag.
  The flag is only "show it"; the name on screen comes from the server.
- **Modified clicks, middle-click and long-press** get neither spinner nor
  flag. No `preventDefault` on them, and no `contextmenu` handler, so the
  browser opens a new tab with no toast, as §2 says.
- **`?switched=1` retires.** The server ignores it. `app.ts` drops it from
  `replaceState`.
- **Acting.** The acting org's row stays ticked at the top. Own-org rows link
  to those orgs, and acting carries on in the acting org's tabs. The
  "Switching ends acting" note goes. Copy is §2's.
- **Keyboard.** Enter follows the link. Arrow and letter keys as today.

**`enter_org`** (`src/controllers/orgs.rs`), used by create and the compat
switch: sets the remember cookie, clears `acting_org`, redirects. It no
longer calls `close_in_browser` or `end_unlock_for_session`.

### 7. Tabs, the socket, the stream, and naming the org on phones (1b)

- `ws-connect` in `chat/show.html` and `chat/thread.html` becomes
  `org_url(path="/chat/" ~ conversation.id ~ "/ws")` (and `?thread=`). The
  upgrade runs `CurrentMember` in address mode, so the socket's `org_id` is
  the tab's org.
- `new EventSource(orgUrl("/notifications/stream"))` in `app.ts:661`. The
  guard selector on line 660 matches the prefixed `hx-get`. The stream
  already filters by its org (PR 5, `events_for`), so each tab's bell counts
  its own org.
- The probes (`app.ts:207`, `1401`, `1560`) call `orgUrl("/orgs/current")`.

**Phone org line (§1).** For people active in two or more orgs, every phone
page except Home shows the org's name in a small muted line above the title.
`app.html` renders it when `multi_org` is set. One-org people see today's
header.

**Task sheet org line (§7).** The task sheet always renders an org line
above the project. CSS shows it only under `body[data-multi-org]`. The sheet
is often drawn by HTMX, which skips the count, so the line can't depend on
the fragment's own data; the attribute on `<body>` comes from the page load.

**`CloseScope::Browser` is no longer needed.** It existed so a switch could
close this browser's old-org connections when the cookie moved every tab.
Now no tab's org changes under it. Remove `users::Model::close_in_browser`,
the `Browser` variant in `src/data/live.rs`, `end_unlock_for_session`, and
their tests: the `Browser` close in `notifications.rs:663` and
`tests/models/users.rs:489` (`end_unlock_for_session_clears_only_that_sign_in`).
`Account` (sign out everywhere, reset, lock) and `Org` (removal) stay.

**The logins unlock** stays one per user, tied to one org (PR 2). Unlocking
in the Northwind tab ends the Acme tab's unlock. Accepted in the brief.

### 8. Home notice, the banner and the in-org 404

`src/controllers/dashboard.rs`, `assets/views/dashboard/index.html`,
`assets/views/layouts/app.html`, `assets/views/errors/not_found.html`.

**Home notice (1b, §5).**

| Query | After |
|---|---|
| `?from=` (empty) or `?from=<an org you're active in>` | "Your last change wasn't saved." and "The app was updated while this tab was open, so we stopped the change to be safe. Please make it again." Dismiss only. No Back link. |
| `?from=<an org your row was removed from>` | The removed notice, unchanged from PR 5. |
| `?from=<anything else>` | The first row's notice. It never names an org you have no row in. |
| `?switched=1` | Ignored (section 6). |
| `?created=1` | Unchanged, at `/o/{new slug}/dashboard?created=1`. |
| `?requested=<id>` | Unchanged, at `/o/{last slug}/dashboard?requested=<id>`. |

`stale_notice` keeps its rules for which org it may name. Since there is no
Back link now, it only needs the removed case. `no-store` stays whenever any
of these is present.

**The in-place banner (1b, §5).** When the tab holds typed text or an upload,
the stale banner reads "This tab is out of date, so nothing was saved." and
"The app was updated while it was open. Your text is still here. Copy it,
then reload the page." Its one button is "Reload page"
(`location.reload()`). The "Back to {org}" `fetch` to the switch
(`app.ts:190`) goes. The gone banner (PR 5) is unchanged.

**The in-org 404 (1c, §4).** `errors/not_found.html` drops "It may be in
another of your organisations. Switch from the menu, then open the link
again." This lands in 1c with the item lookup, because until then a bare
item link can still open in the wrong org, and the line is true. Its button
becomes "Back to {section}" in 1a (section 2).

### 9. Search engines and the referrer (1a)

- **`GET /robots.txt`** in `src/controllers/page.rs`, `text/plain`:

  ```
  User-agent: *
  Disallow: /o/
  Disallow: /oauth/
  Disallow: /admin
  Disallow: /mcp
  ```

  `/join/` is not disallowed on purpose: a crawler must fetch a join page to
  see its `noindex`.
- **`X-Robots-Tag: noindex`** on every response except the homepage, set in
  the org-path layer (section 1). It covers HTML, file downloads and
  redirects alike, with no per-template work. No `<meta name="robots">`.
- **`Referrer-Policy: same-origin`**, same layer, every response. Checked:
  nothing in `src/` or `config/` sets it. Loco's `secure_headers` middleware
  would, but it is off by default (`loco-rs` 1.2.0
  `controller/middleware/mod.rs:137`) and `config/production.yaml` doesn't
  enable it. Whether Railway's edge adds one is unverified. Browsers default
  to `strict-origin-when-cross-origin`, which already hides the path
  cross-site. `same-origin` sends nothing cross-site at all. Nothing in the
  app reads `Referer`.

### 10. Docs

| File | Change |
|---|---|
| `docs/flows/accounts.md` | "Choosing an organisation": the address picks it, the cookie remembers the last (and when it moves), bare addresses redirect, the status pages per address, the identical not-found. **Be honest about what that protects:** an organisation's existence is already public through `/join/{slug}`. The identical 404 protects who belongs to an organisation, not whether it exists. Remove the switch-closes-connections text. |
| `docs/flows/super-admin.md` | Acting is per address. Own orgs work in other tabs. The banner only on the acting org. |
| `docs/flows/chat.md`, `docs/flows/notifications.md` | Socket and stream under `/o/{slug}/`. Broadcast fragments carry the socket's org. Bell links get the prefix when shown. No close on switch. |
| `docs/flows/claude-connector.md` | `/settings/claude` under `/o/`. The Allow page stays bare and uses the last org until PR 3. `/mcp` unchanged. |
| `docs/flows/documents.md`, `docs/flows/tasks.md` | Paths. |
| `docs/architecture/erd.md` | `organisations.slug`: "used in the join link and every app address". `notifications.link`: "path inside its organisation". |
| `docs/deploy.md` | Logs carry the slug on the span; the logged path is the stripped one. |
| `docs/plans/multi-org-2.md`, `-3.md`, `-5.md` | A one-line note where they describe the switch closing connections and the cookie choosing the org: superseded by this plan. |

## Edge cases

- **Two tabs, Acme and Northwind.** Independent. Each posts to its own
  address with its own claim.
- **A copied address sent to a teammate in the same org.** Opens the same
  page. Sent to someone outside: the plain not-found page (§4).
- **Not a member vs no such org.** Same single query, same status, body,
  `Set-Cookie` and `Cache-Control`. The 404 page depends only on the user.
  HTMX gets the same 409. Existence of an org is public anyway through its
  join page; membership is what this hides.
- **Bad shapes and odd segments.** The table in section 1. All 404, the same
  for every slug.
- **`/o/acme` and `/o/acme/`.** GET or HEAD: 303 to `/o/acme/dashboard`,
  which then resolves like any page. Other methods: 404.
- **An unknown id under a real org** (`/o/acme/tasks/999999`). Acme's in-app
  404 with "Back to Tasks".
- **Pending in Beta, active in Acme, opens `/o/beta/tasks`.** Beta's waiting
  page (§3): "Open Acme Studio", "Check again", "Sign out".
- **Removed from Beta with a Beta tab open.** PR 5 closes Beta's sockets and
  streams. The tab's probe to `/o/beta/orgs/current` gets the 409 with
  `X-Org-Gone`, and it moves to Beta's removed page. Typed text stays behind
  the gone banner.
- **Super admin, acting in X, also a member of A.** `/o/x/...` acts, with the
  banner. `/o/a/...` is their own membership. After `POST /admin/leave`, an
  open X tab's next page load is the plain not-found and its HTMX calls the
  409 to `/dashboard`.
- **Super admin, not acting, or acting in another org, opens `/o/x/...`.**
  The plain not-found, identical to a made-up slug. The address never starts
  acting.
- **A claim that disagrees with the address.** Refused to the address org's
  Home with `?from=<claim>`. Only a bug or a forged request does this.
- **Tabs open across the 1b deploy.** Bare links still work through today's
  cookie mode.
- **Tabs open across the 1c deploy.** Bare writes and polls are refused once
  and land on Home in the org they were showing, with "Your last change
  wasn't saved." The old `app.js` banner keeps its old words, once per tab
  (§9: accepted). Its "Back" posts to the compat switch.
- **Old item links, signed in.** The item's org when you're active (or
  acting) there. Otherwise the last org's in-org 404, the same as a missing
  id (decision 12).
- **Odd bytes in a bare path** (`/chat/%2F%2Fevil.com`, `/tasks?x=//evil.com`).
  Redirected under `/o/{slug}/`, same-origin.
- **Signed out on any `/o/` or bare address.** `/login`, no `next`, then the
  default org. PR 2.
- **Sign out everywhere from a `/o/beta/more` tab.** Bare
  `/more?signed_out=1`, so it shows in the last org, normally Beta. Accepted.
- **Create a new org from Acme's tab.** Lands on `/o/new/dashboard?created=1`.
  Other tabs and their sockets carry on.
- **The toast flag in `sessionStorage`** is per tab. A blocked storage just
  means no toast.
- **Bell notices written before the deploy.** Org-relative paths, prefixed
  when shown.
- **A link pasted into a task or a message.** Bare: redirected. Full `/o/...`:
  opens in that org. Outbound links send no referrer.
- **HEAD.** Treated like GET.
- **A 303 cached by a browser.** Every redirect here is `no-store`.

## Tests

The full suite stays green after each sub-PR. Route walks fill path
parameters with `1`, as in `tests/requests/stale_tab.rs`.

### 1a: `tests/requests/org_address.rs` (new), plus unit tests

Unit tests:

- `org_path.rs`: every input in section 1's table; the absolute-form URI
  keeps scheme and authority; a request's `OnUpgrade` and `ConnectInfo`
  extensions survive the strip; a header value that won't build is a 400;
  the rollback stub's redirects and 404s.
- `links.rs`: `is_slug` at 0, 1, 60 and 61 bytes and for each bad byte;
  `org_path` fails closed for a bad slug and a bad path; `slugify` never
  exceeds `SLUG_MAX` with a suffix; `join_page` uses the shared constant.

Requests (Alice in Acme and Beta, Kreepa only in Acme, a super admin):

- `path_parameters_reach_the_handlers`.
- `two_tabs_write_into_their_own_org` (done-when 1): with `current_org` on
  Beta, `POST /o/acme/tasks` with Acme's claim creates the task in Acme, and
  the Beta one in Beta. No refusal, no notice.
- `the_cookie_never_overrides_the_address`.
- `a_strangers_org_and_a_made_up_slug_look_identical` (done-when 8). For
  every tenant route in the route list, under `/o/beta/` and
  `/o/made-up-name/`: a page GET, an HTMX GET, a HEAD and a same-origin POST
  (with a real session, `Origin` set to the app URL, and a claim). It
  includes `POST /o/.../settings/claude/tokens` (a `PinnedMember` route). Run
  as Kreepa, as a super admin who isn't acting, and as a super admin with
  `acting_org` set to a third org. Status, body, `Set-Cookie` and
  `Cache-Control` must match.
- `public_routes_are_404_under_o`.
- `o_slug_alone_goes_to_home_for_get_only`.
- `a_pending_member_sees_that_orgs_waiting_page` (done-when 9): §3's buttons;
  with two other orgs, the list in order with roles. The same for removed and
  declined. Over HTMX: 409, `HX-Redirect: /o/beta/dashboard`, `X-Org-Gone`.
- `the_signed_in_not_found_names_only_the_user`: "Signed in as" with the
  email, no org name anywhere.
- `the_in_org_404_goes_back_to_its_section`.
- `a_super_admin_with_no_active_row_goes_to_admin`.
- `acting_applies_only_to_the_acting_orgs_address`.
- `a_claim_for_another_org_is_refused_to_the_address_orgs_home`.
- `a_same_origin_page_load_remembers_the_org`: `Sec-Fetch-Site: same-origin`,
  `none` and no header each set `current_org` with PR 2's attributes.
- `a_cross_site_navigation_sets_no_cookie`: `Sec-Fetch-Site: cross-site` and
  `same-site` set nothing. Nor do HTMX, POST or acting.
- `the_token_form_pin_is_checked_against_the_address`.
- `only_the_allow_page_uses_the_last_org`: the PR 2 OAuth tests pass
  unchanged.
- `the_switch_is_404_under_an_org_address`: `POST /o/acme/orgs/{id}/switch`
  with a real session and a same-origin `Origin` is a 404, and sets no
  cookie.
- `only_the_resolver_reads_path_org` (source test).
- `an_upload_under_o_keeps_its_body_limit`.
- `robots_txt_disallows_app_paths_not_join`.
- `every_response_but_the_homepage_is_noindex`: `X-Robots-Tag` on pages,
  redirects, a file download and a 404; none on `GET /`.
- `every_response_has_referrer_policy_same_origin`.
- The existing suite, untouched, proves bare addresses still work.

### 1b: `tests/requests/links.rs` (new), plus updates

- `every_app_page_links_only_inside_its_org` and `no_hand_built_tenant_paths`
  (section 4), including write responses and socket frames.
- `bell_links_carry_the_org`.
- `switcher_rows_link_to_the_same_section`: from `/o/acme/tasks/58` the Beta
  row is `/o/beta/tasks`; from More, `/o/beta/more`; from
  `/o/acme/orgs/new`, `/o/beta/dashboard`; `?section=../../evil` gives
  `/dashboard`. The current row is a button with a tick.
- `the_switcher_has_no_switch_form`.
- `multi_org_pages_carry_the_flag`: `data-multi-org` and the phone org line
  for two orgs; neither for one; the task sheet fragment always has its line.
- `creating_an_org_lands_in_it_and_closes_nothing`.
- `the_old_switch_post_lands_under_o_and_closes_nothing`.
- `there_is_still_no_get_that_switches`: `GET /orgs/{id}/switch` is 405.
- `join_open_is_a_link`, `admin_enter_lands_under_o`, `switched_is_ignored`.
- `home_notice_copy`: empty `from`, an active org's `from` (no Back link), a
  removed org's `from`, a stranger's.
- Live (`tests/requests/live_sign_out.rs`, real server):
  `two_orgs_chat_live_in_their_own_tabs` (done-when 2),
  `each_tabs_stream_counts_its_own_org`, and the broadcast frames carry
  `/o/` links. The existing live tests move to `/o/` URLs.
- Updated: every test asserting a `Location` or `HX-Redirect` the app builds
  (about 100 strings), now through `m.at(...)`.
- Removed: the `CloseScope::Browser` tests (section 7) and the PR 2 and PR 3
  tests that expect a switch to close sockets or end an unlock.

### 1c: `tests/requests/bare_addresses.rs` (new), plus the migration

- `every_bare_tenant_page_load_redirects_keeping_path_and_query`: a walk over
  tenant GET routes. 303, `no-store`, `/o/{last}/...`.
- `old_item_links_follow_only_active_rows`: one case per row of the
  id-bearing table, `/chat?c=` included. With `current_org` on Acme, a Beta
  item lands in Beta (done-when 6). Pending, removed or declined in Beta:
  the last org's 404. Acting in Beta: Beta.
- `an_item_in_a_strangers_org_looks_like_a_missing_id`: same `Location`, same
  query count.
- `every_bare_non_get_is_refused_and_saves_nothing`: a walk over non-GET
  tenant routes, `POST /settings/claude/tokens` included. 409 for HTMX and
  `X-Org-Id`, 303 for plain forms, never 307 or 308, and no row written in
  any table.
- `a_refused_tab_lands_in_the_org_it_was_showing`, else the last org; a
  removed claim gets `?from=<id>` and `X-Org-Gone`. It never echoes the raw
  claim.
- `bare_htmx_gets_and_fetches_are_refused`,
  `a_bare_socket_upgrade_is_not_upgraded`, `a_bare_stream_is_not_a_stream`.
- `bare_redirects_stay_on_this_site`: `/chat/%2F%2Fevil.com` and
  `/tasks?x=//evil.com` give a `Location` starting with `/o/{slug}/`.
- `a_bookmark_of_tasks_opens_the_last_org` (done-when 7).
- `old_email_links_still_land` (section 3's table, signed in).
- `signed_out_links_go_to_sign_in_as_today`.
- `the_allow_page_is_still_bare`.
- `the_in_org_404_has_no_other_org_hint`.
- `tests/requests/stale_tab.rs`: the walks run on `/o/` URLs. `EXEMPT` and
  `EXEMPT_GET` gain `/robots.txt`. The `PinnedMember` list names its three
  uses (the Allow page GET and POST, the token form).
  `the_exempt_lists_match_real_routes` keeps them honest.

### The migration (1c)

About 534 request-test calls hit bare tenant paths.

- `TestMember` (`tests/requests/prepare_data.rs`) gains `slug`, and
  `fn at(&self, path: &str) -> String`, which calls the app's own
  `collab::views::links::org_path`. `member_from` follows Home's redirect to
  read the slug. `sign_up_member` returns it.
- One line per call: `request.get("/tasks")` becomes
  `request.get(&alice.at("/tasks"))`. `.as_member(&alice)` is unchanged.
  Tests that hold a bare `Cookie` switch to a `TestMember` first.
- A throwaway script does the mechanical part, file by file. The compiler and
  the 409s (section 3, "For the engineer") find the rest. The reviewer checks
  the diff changes nothing but the URL.
- Tests about the old cookie behaviour are rewritten, not migrated: the PR 2
  cookie tests in `multi_org.rs`, PR 3's switch tests in `orgs.rs`, and
  `with_a_beta_cookie_acme_tasks_404_and_new_tasks_go_to_beta`.

**Not covered by `cargo test`.** No JS runner. `orgUrl`, the switcher's
plain and modified clicks, the toast flag, "Opening…", the probes, the
stream URL and both banners are hand checks.

## Out of scope

PR 2 (links):

- Email links name their org: mentions, "all notifications", login activity,
  approval. Account emails (forgot, reset, password changed) stay bare.
- Old links for signed-out readers: sign-in comes back to the page, with §7's
  copy.
- `safe_next` accepts `/o/{slug}/...`. Needs the security reviewer.
- Mention links like `/roadmap?lane=` that carry no id.

PR 3 (Claude, §6):

- The Allow page's organisation picker. Until then it uses the last org.
- `/settings/claude` showing that org's connections only. PR 1 moves the
  route; the content is unchanged.
- Removing the compat `POST /orgs/{id}/switch`.

Not in this project (brief): editable slugs or old-slug redirects, custom
domains, unread dots per org, a combined bell, links in Claude tool results,
removing the stale-tab check, email verification on sign-up and join.

Also not here: fixing accents in new slugs (decision 10). Tab titles stay
"Tasks · Acme Studio" (§1).

## Risks

- **Size.** About 250 link edits and 534 test edits. The split keeps each
  sub-PR reviewable.
- **A missed link.** Before 1c it uses today's cookie mode. After 1c it is
  one extra redirect for a page, or a refused write with nothing saved. Never
  a write into another org. The two link tests make this rare.
- **A missed fragment.** A write response or socket frame drawn without
  `org_base` fails to render, so the write shows an error. Test 2 and the
  extended walk catch it.
- **The outer router.** `fallback_service` wrapping must keep the socket
  upgrade, `ConnectInfo`, Loco's `remote_ip` and static middlewares, and
  graceful shutdown. Proved by unit and live tests and by hand on Railway.
- **Tera 2 internals.** `org_url` reading context through `State`, and inside
  components, is unverified (section 4). The engineer checks first. The
  fallback is a component prop.
- **The remember cookie** relies on `Sec-Fetch-Site`. Browsers without it are
  treated as same-origin (section 5).
- **Timing of the not-found.** One query either way. The acting branch adds a
  query only for super admins with the cookie. The old-item lookup is one
  query whether or not the id exists.
- **Production slugs.** The shape check assumes every slug is `[a-z0-9-]`,
  1 to 60 bytes. Checked before merging (see "Checks").
- **Logs.** The logged path is the stripped one; the slug is on the span.
  Dashboards or alerts keyed on paths need the span field.
- **Tabs across the 1c deploy** are refused once, with old banner words.
- **The unlock across tabs.** Unlocking in one org's tab ends another's.
  Accepted in the brief.
- **Two hops after sign-in** (`/`, then `/dashboard`, then `/o/...`).

## Rollback

No migration, so each sub-PR reverts as code. Revert in reverse order: 1c,
then 1b, then 1a.

- **Reverting 1c** puts bare addresses back on today's cookie mode. Links
  stay under `/o/`. Safe.
- **Reverting 1b** makes links bare again, on today's cookie mode. `/o/`
  addresses still resolve. Safe.
- **Reverting 1a** removes `/o/`. Any address someone copied, bookmarked or
  pasted into a chat (and, after PR 2, every email link) would 404. So the
  revert keeps the stub written and tested in 1a (`org_path::revert_layer`,
  section 1): GET or HEAD for `/o/{slug}/{rest}` answers 303 to `/{rest}`
  with its query; anything else under `/o/` is a 404. No database, no
  cookie. The bare page then opens in the cookie's org, as before this
  project.
- The `current_org` cookie holds an org id under every version, so no cookie
  needs clearing.

## Checks

```sh
bin/build-assets
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

Before merging 1a, on production. Both should return no rows:

```sql
SELECT id, slug FROM organisations WHERE slug !~ '^[a-z0-9-]{1,60}$';
SELECT slug, count(*) FROM organisations GROUP BY slug HAVING count(*) > 1;
```

By hand, locally, with the mockup's people: Acme Studio, Northwind Bakery,
Himalayan Ritual (pending), Raj in all three, Shusma, Kreepa.

- Acme Tasks in one tab and Northwind Tasks in another (§1). Add a task in
  each, send a chat message in each, upload a file in each, watch each bell.
  Nothing moves to Home, and nothing lands in the wrong org.
- Unlock logins in Northwind, then try a reveal in the Acme tab: it asks to
  unlock again.
- The switcher (§2) from a task page, More and Home: a plain click shows
  "Opening…" and the toast; Cmd-click, middle-click and phone long-press open
  a new tab with no spinner and no toast.
- The phone org line and the task sheet line, with two orgs and with one.
- Kreepa on `/o/northwind-bakery/tasks` and `/o/made-up-name/tasks` (§4).
- Himalayan Ritual's waiting page (§3).
- Remove Raj from Northwind with its tab open and text typed in the task
  dialog: the gone banner, then Northwind's removed page.
- Super admin: enter Northwind from `/admin` in one tab, keep Acme in
  another. The banner shows only in Northwind.
- From another site's page (a local file with a link), open
  `/o/northwind-bakery/tasks`; then a bare `/tasks` still opens the earlier
  org.
- Bare `/tasks`, `/tasks?open=<a Northwind task>`, `/chat/<id>`,
  `/chat?c=<id>`, `/documents/files/<id>` with the cookie on Acme.
- A tab drawn on `main`, then the branch started: submit a form, poll, and
  click the old switcher (§5).
- On `collab.zen-scale.io` after deploy: the chat socket and the stream
  through Railway under `/o/`, `robots.txt`, and the `Referrer-Policy` and
  `X-Robots-Tag` headers (`curl -sI`). The slug shows in the request logs.

### Screenshots needed

From the local app with the realistic data above, on the `pr-screenshots`
branch under `org-url-1a/`, `org-url-1b/` and `org-url-1c/`. Desktop 1440px
and phone 390px, light and dark. Headless captures have no address bar, so
take the side-by-side shots from a visible Chrome window, or caption each
shot with its address.

- 1a: Himalayan Ritual's waiting page, and the removed page with "Your other
  organisations" (§3). The plain not-found, signed in, and the in-org 404
  with "Back to Tasks" (§4).
- 1b: two windows side by side, Acme Tasks and Northwind Tasks, addresses
  showing (§1). The switcher open on a chat page with the hovered row's
  address, and "Opening…" (§2). The phone org line, and the task sheet line
  (§7). The Home notice and the in-place banner (§5).
- 1c: a bare bookmark and an old mention link, before and after, with the
  address they land on. The in-org 404 without the old hint.

## Decisions for Raj

Each with a recommendation. Both reviewers approve with changes, and the
changes are applied above.

1. **Split PR 1 into 1a, 1b and 1c.** Each is safe alone. Recommend: yes.
2. **`/orgs/new` and `POST /orgs` move under `/o/{slug}/`.** The brief listed
   them outside `/o/`. Recommend: move them (section 5). A bare `/orgs/new`
   still works through the redirect.
3. **The probe `/orgs/current` moves under `/o/{slug}/`.** Recommend: yes. It
   has to check the tab's own org.
4. **The "You're now in Northwind" toast stays, on the client.** `?switched=1`
   retires; a plain click sets a `sessionStorage` flag and the next page
   shows the toast from its own org name (§2). Recommend: yes.
5. **Keep `POST /orgs/{id}/switch` as a thin redirect until PR 3**, 404 under
   `/o/`. Recommend: yes, for tabs drawn before the deploy.
6. **Old item links find their org in PR 1, for signed-in readers.** The brief
   put it in PR 2. Recommend: PR 1 (in 1c), following active rows only. PR 2
   keeps the signed-out part.
7. **Status pages for pending, removed and declined are for `/o/{slug}/`
   addresses only.** An old bare item link into such an org goes to your last
   org instead (security review). Recommend: yes.
8. **In-app bell links get the prefix in PR 1.** Emails stay in PR 2.
   Recommend: yes.
9. **`X-Robots-Tag: noindex` on every response except the homepage, and
   `Referrer-Policy: same-origin`.** The brief asked for `noindex` on app and
   join pages. Recommend: every response but the homepage.
10. **Accents in new slugs** ("Lumen Café" to `lumen-cafe`, brief question 1).
    Recommend: a small change of its own after PR 1.
11. **Remove `CloseScope::Browser` and `close_in_browser`.** Recommend: yes,
    in 1b.
12. **An old bare link to an item in an org you're not active in goes to your
    last org's in-org 404, the same as a missing id.** The mockup's §4 line
    says the plain not-found page, but choosing that page would tell whether
    the id exists in some other org. Recommend: the plan's version.
