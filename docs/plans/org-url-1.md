# Organisation in the address 1: routes, links and bare addresses

Status: Draft · PR 1 of 3 · Owner: Raj · 20 October 2026

Brief: `docs/briefs/org-in-the-address.md`, approved by Raj on 20 October with
all 9 questions as recommended. Mockup: `docs/designs/org-in-the-address.html`,
being drawn in parallel. This plan names the mockup's parts by what they show
(the switcher, the status pages, the not-found page, the Home notice). The
visuals and copy are the mockup's. Update the section numbers once it lands.

This touches tenancy and access rules, so it needs the security reviewer as
well as the staff reviewer and the product manager.

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
organisation's status page. No row, or no such organisation: the same plain
not-found page, from the same single query. The page claim (`X-Org-Id` or
`_org`) is now compared with the address's organisation.

**Bare addresses.** A bare page load (`/tasks`, an old email's
`/tasks?open=58`) redirects to `/o/{slug}/` plus the same path and query. An
old item link first looks for the item's organisation among yours. Anything
else bare (a POST, an HTMX call, a `fetch`, the socket, the stream) is never
redirected. It is refused and the tab goes to Home, so a write can never be
replayed into the last-used organisation.

**Every link through one helper.** `org_url(path=...)` in templates,
`member.url(...)` in Rust, `orgUrl(...)` in `app.ts`. A test walks the
rendered pages and fails on any app link without the prefix. A second test
greps the sources for hand-built tenant paths.

**The switcher becomes links.** Each row links to the same section in that
organisation, so Cmd-click and long-press open it in a new tab. Nothing closes
live connections any more, since each tab has its own.

**Proposed as three sub-PRs** (decision 1), each safe to ship alone:

| Sub-PR | What | Visible |
|---|---|---|
| 1a. Addresses resolve | The layer, resolution by slug, the remember cookie, the link helpers, `robots.txt`, `noindex`, `Referrer-Policy`. Bare addresses keep today's cookie behaviour. | Only the status pages' new link. Nobody lands on `/o/` unless they type it. |
| 1b. Links carry the org | Every template, Rust redirect, `app.ts` URL, socket and stream. The switcher as links. Home notices. The link tests. | Addresses change. Side-by-side tabs work. |
| 1c. Bare addresses redirect | Bare page loads redirect (with the old-item lookup). Everything else bare is refused. The test migration. | Old bookmarks and email links land under `/o/`. |

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

**`src/initializers/org_path.rs` (new).** For each request:

1. The raw path (never decoded) is `/o/{slug}` or `/o/{slug}/` exactly: 303 to
   `/o/{slug}/dashboard`. Same-origin, the slug passes the shape check below.
2. The raw path is `/o/{slug}/{rest}`, where:
   - `{slug}` is 1 to 64 bytes, each `a`-`z`, `0`-`9` or `-`, checked byte by
     byte like `join_page` (`src/controllers/auth.rs:84`). No lowercasing, no
     decoding: `/o/Acme/...` and `/o/%61cme/...` don't match.
   - the first segment of `{rest}` is in `TENANT_SECTIONS`: `dashboard`,
     `tasks`, `chat`, `documents`, `members`, `more`, `notifications`,
     `roadmap`, `meetings`, `settings`, `guide`, `orgs`.

   Then the request URI becomes `/{rest}` with the same query. The layer
   inserts `PathOrg { slug }` and axum's `OriginalUri` (the full address), so
   the router doesn't replace it with the stripped one.
3. Anything else, including `/o/acme/login`, `/o/acme/mcp`,
   `/o/acme/static/...`, `/o/acme/o/beta/tasks` and a bad slug shape, passes
   through untouched with no `PathOrg`. No route matches `/o/...`, so it is a
   404, drawn by `not_found_page` as today. The same for every slug, so it
   says nothing about which organisations exist.

The layer strips once, never loops, and never reads the body. It has no
`unwrap`.

**Response headers, in the same layer:** `Referrer-Policy: same-origin` on
every response (see section 9).

**`src/app.rs`.** Register `OrgPathInitializer` last, with a comment: it must
be outermost because routing happens inside it.

**Path parameters, HTMX, uploads, the socket and the stream** all work
unchanged after the strip:

- `/o/acme/tasks/58` routes to `/tasks/{id}` with `Path(58)`.
  `/o/acme/chat/12/threads/9` routes to `/chat/{id}/threads/{message_id}`.
- HTMX requests are ordinary requests with a prefixed URL.
- `/o/acme/documents/folders/{id}/files` keeps its route-level
  `DefaultBodyLimit::max(UPLOAD_BODY_LIMIT)` and the `no_framing` layer,
  because both sit on the inner route.
- `/o/acme/chat/{id}/ws`: the upgrade's `OnUpgrade` extension passes through
  `fallback_service` with the request. To prove on the real server
  (`tests/requests/live_sign_out.rs`) and by hand on Railway.
- `/o/acme/notifications/stream`: an ordinary GET.

### 2. Resolution (1a)

`src/extractors/current_member.rs`. `resolve_org` gets a second mode. With
`PathOrg` it resolves from the address. Without it, it keeps today's order
(acting, then `current_org`, then the default) until 1c changes bare
addresses.

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
   - A page load gets `Error::NotFound`. `not_found_page` then draws
     `errors/not_found_public.html` (its `PinnedMember` fails the same way for
     both cases).
   - HTMX and `fetch` (the `wants_hx_redirect` test) get 409 with
     `HX-Redirect: /dashboard` and `no-store`.
   - Both cases make the same single query and get byte-identical responses.
     The only timing difference is whether the slug index finds a row before
     the membership lookup misses. That is microseconds, well under network
     jitter. Not measured; the engineer logs both locally.
5. **Active row:** the member. `org_menu` as today.
6. **Row not active** (pending, removed, declined):
   - A super admin goes to `/admin`, as today (PR 6's rule).
   - A page load gets that org's `tenancy/waiting.html`, `removed.html` or
     `declined.html` with 403, as today. New: the page links to the person's
     last active org, if any (the mockup's status pages). That costs one more
     query, only on this path: the bare "last org" resolver (below) filtered
     to active.
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

**Remembering the last org.** On a page load (GET or HEAD, not HTMX) in
address mode, own membership (not acting), when the request's `current_org`
cookie isn't that org's id: set it to that id. HTMX, `fetch`, POSTs and
acting never change it.

- An extractor can't set cookies. `src/initializers/current_org.rs` already
  puts a `StaleCurrentOrg` flag in every request and clears the cookie when
  it is raised. It becomes a small `OrgCookie` slot: nothing, clear, or
  remember `id`. The layer appends the matching `Set-Cookie` from
  `current_org_cookie` or `cleared_current_org_cookie`. Same cookie rules as
  PR 2: HttpOnly, `SameSite=Lax`, `Path=/`, 7 days.
- Address mode never raises "clear". A forged or stale cookie no longer
  matters there.

**`PinnedMember`** resolves the same way and skips the claim check. With
`PathOrg` it uses the address. Without it, it uses the last org. That keeps
both of its uses working:

- `POST /o/{slug}/settings/claude/tokens`: the hidden `org_id` pin is now
  compared with the address's org.
- `POST /oauth/authorize` stays bare (brief question 5) and keeps the last
  org until PR 3 adds the picker.

`not_found_page` (`src/app.rs`) uses `PinnedMember`, so an unknown task id
under `/o/acme/` is drawn inside Acme's shell, and a bare 404 inside the last
org's shell, as today.

**`CurrentMember::url(&self, path) -> String`**, and `page()` adds
`"org_base": "/o/{slug}"` next to `current_org_id`.

### 3. Bare addresses (1c)

**The last org** is today's resolution order, kept as one function: a valid
`acting_org` for a super admin, then a `current_org` you're active in, then
`default_for_user` in any status. No membership: a super admin goes to
`/admin`; anyone else gets `tenancy/no_org.html`, as today. It is used for
bare addresses, sign-in landing (`/` then `/dashboard`) and `PinnedMember`
without an address.

**A bare page load** is GET or HEAD, with no `HX-Request`, no `X-Org-Id`, no
`Upgrade` header, and an `Accept` that doesn't ask for `text/event-stream`.
`CurrentMember` (bare mode) answers it with 303 and `no-store`:

```
/o/{target slug}{raw path}{?raw query}
```

- The path and query are the request's own bytes, unchanged. The result
  always starts with `/o/{slug}/`, so it is same-origin whatever the path
  holds. The bare request only gets here when it matched a tenant route, and
  the `http` crate rejects raw CR and LF.
- The target is the item's org when the path names an item (below) and the
  person has a membership row there in any status, or is acting there.
  Otherwise the last org.
- A pending, removed or declined row still counts, so an old link into Beta
  lands on Beta's status page, as the brief wants for links. An item in an
  org you're not in goes to your last org, where it 404s. So does an id that
  doesn't exist. Both look the same.

**Id-bearing bare paths.** The lookup is
`src/models/old_links.rs::item_org(db, path, query) -> Option<i64>`, a match
on path segments. Each arm is one indexed query on the item's
`organisation_id` (all eight tables have one):

| Bare page load | Item | Table |
|---|---|---|
| `/tasks/{id}`, `/tasks/{id}/edit` | task | `tasks` |
| `/tasks?open={id}` (mention emails, bell) | task | `tasks` |
| `/tasks/views/{id}/edit` | saved view | `saved_views` |
| `/chat/{id}` (mention emails, bell) | conversation | `conversations` |
| `/chat/{id}/threads/{message_id}` (thread emails, bell) | conversation | `conversations` |
| `/documents/folders/{id}` | folder | `folders` |
| `/documents/files/{id}` (downloads) | document | `documents` |
| `/documents/logins/{id}` | login | `logins` |
| `/members/{id}/rotate` | membership | `memberships` |
| `/roadmap/projects/{id}/edit` | project | `projects` |

The other id-bearing tenant routes are never page loads, so they never
redirect: `/chat/{id}/feed`, `/chat/{id}/ws`,
`/chat/{id}/messages/{message_id}/actions`,
`/chat/{id}/threads/{message_id}/feed`, and every POST
(`/notifications/{id}/open`, `/settings/claude/grants/{id}/revoke`, ...).
`/roadmap?lane=` and `/notifications` carry no id and go to the last org,
as today.

**Anything else bare is refused, never redirected.** POST, PUT, DELETE, an
HTMX or `fetch` GET, the socket upgrade, the stream:

- HTMX or `X-Org-Id` (raw `fetch`): 409 with `HX-Redirect`. A plain form:
  303 with `Location`. Never 307 or 308, which would repeat the body.
- Target: `/o/{t}/dashboard?from=`, where `t` is the claimed org when the
  claim is `Org(id)` and `active_in(id, user)` finds a row, else the last org.
  An empty `from` gives Home's nameless "nothing was saved" notice (section
  8). One query, only on this path. Only a slug from the database goes into
  the address, never request text.
- `no-store`. One info log line, numbers only: user id, claimed org,
  method.
- A bare socket upgrade gets the 303. The handshake fails, and the old page's
  `app.js` probes on `htmx:wsClose` and moves. A bare `EventSource` follows
  the 303 to an HTML page, fails, ends `CLOSED`, and the old page's probe
  moves it.

Who sends bare requests after 1b: tabs drawn before the deploy, and any link
1b missed. Both are refused once, with nothing saved. A missed link is a
visible bug, never a write into the wrong org.

**`src/controllers/oauth.rs`.** `authorize_page` takes
`Result<PinnedMember, Response>` instead of `Result<CurrentMember, Response>`.
Without this, 1c would redirect the Allow page to `/o/{last}/oauth/...`,
which is a 404. For a non-HTMX GET the claim check never ran anyway, so
nothing else changes.

**`src/controllers/page.rs`** keeps sending a signed-in visitor at `/` to
`/dashboard`, which then redirects. Two hops after sign-in. Accepted; it
avoids a second copy of the resolver.

**Email links in PR 1.** Mailers are PR 2. After 1c, for a signed-in reader:

| Email link | Lands on |
|---|---|
| Mention: `/tasks?open=58`, `/chat/12`, `/chat/12/threads/9` | The item's org (lookup) |
| Mention: `/roadmap?lane=now`, "all notifications" `/notifications` | The last org, as today |
| Security: `/documents/activity` | The last org, as today (PR 2 fixes it) |
| Approval: `/dashboard` | The last org, as today |
| Forgot, reset | Unchanged, not tenant routes |

Signed out, every link goes to `/login` without `next` and then the default
org, as today. Nothing regresses. PR 2 adds the org to these links and lets
sign-in come back to them.

### 4. Every link (1b)

**One helper, three spellings.**

| Where | Helper | Example |
|---|---|---|
| Rust | `src/views/links.rs::org_path(slug, path)`, and `CurrentMember::url(path)` | `redirect_response(&headers, &member.url("/tasks"))` |
| Templates | Tera function `org_url(path=...)`, registered in `src/initializers/view_engine.rs` next to `asset_url` | `href="{{ org_url(path="/tasks/" ~ task.id) }}"` |
| `frontend/app.ts` | `orgUrl(path)`, from `document.body.dataset.orgBase` | `new EventSource(orgUrl("/notifications/stream"))` |

- `org_path` requires `path` to start with `/` and not with `/o/`, and
  `debug_assert!`s it. One place builds `/o/{slug}`.
- `org_url` reads the top-level `org_base` that `page()` adds, through
  `tera::State::get`. If it is missing it returns an error, so a template
  rendered without `page()` fails loudly in tests. **Unverified:** whether a
  Tera 2 component sees the caller's context through `State`
  (`component_parent` in `tera-2.4.0/src/vm/state.rs` suggests it does). If
  not, components take `org_base` as a prop.
- Templates rendered outside `page()` must add `org_base` themselves. The
  known ones are chat messages rendered for the socket's broadcast (the
  socket's own org) and HTMX fragments built with `data!`. The rendered walk
  test covers them.
- `base.html` adds `data-org-base="/o/{{ org.slug }}"` on `<body>` next to
  `data-org-id`, under the same `current_org_id` condition.
- Paths stored or built in models stay org-relative and get the prefix when
  shown: `notifications.link`, `guide.rs` step `href`s, `saved_views::url()`.
  Each row is already scoped to `member.org.id`, so the prefix is that org.
  No data migration.

**Finding them.** The counts are from the brief and a grep on this branch:

| Where | About | How |
|---|---|---|
| Templates, 64 files | 187 (178 under a tenant section by the grep below) | `grep -rnoE '(href\|action\|formaction\|hx-(get\|post\|put\|patch\|delete)\|ws-connect\|data-[a-z-]*url)="/(dashboard\|tasks\|chat\|documents\|members\|more\|notifications\|roadmap\|meetings\|settings\|guide\|orgs)' assets/views` |
| Rust, 25 files | 77 (about 60 outside mailers and tests) | `"/<section>` literals and `format!("/<section>`, minus `routes()` and `.prefix(` lines |
| `frontend/app.ts` | 14 | `"/<section>` and `` `/<section> `` literals, plus `"/dashboard"` fallbacks and the `[hx-get^="/notifications/unread"]` selector (line 660) |

The engineer turns the grep output into a checklist and converts one area
per commit: layouts, dashboard, tasks and saved views, chat, documents and
logins, members, roadmap and meetings, notifications, more, settings and
guide, orgs, `app.ts`. The two tests below must pass at the end.

**Rust call sites that change** (from the grep):

| File | Lines |
|---|---|
| `src/controllers/tasks.rs` | redirects to `/tasks` (749, 814, 841, 1057, 1081) |
| `src/controllers/chat.rs` | `thread_url` (148), redirects (507, 682, 1038, 1071) |
| `src/controllers/documents.rs` | 295, 317, 336-337, 368 |
| `src/controllers/logins.rs` | form actions and redirects (84, 175, 274, 300, 325) |
| `src/controllers/members.rs` | 245, 283, 396 |
| `src/controllers/roadmap.rs`, `meetings.rs`, `guide.rs`, `claude.rs` | 268; 101; 21, 42; 155 |
| `src/controllers/notifications.rs` | `open` (496, 508) and `read_all`. The links written at 105, 176, 255, 292, 356 stay org-relative. |
| `src/controllers/orgs.rs` | `AFTER_CREATE`, `AFTER_SWITCH`, see section 5 |
| `src/controllers/admin.rs` | `enter` (78): `/o/{slug}/dashboard` |
| `src/controllers/join.rs` | 119 `/o/{last}/dashboard?requested=`; 193 `/o/{slug}/dashboard`; 79 stays bare (the new account is pending) |
| `src/controllers/signup.rs` | 44: `/o/{new slug}/dashboard`; 25 stays bare |
| `src/controllers/auth.rs` | 229 `/more?signed_out=1` stays bare (account route, last org); 420 stays bare |
| `src/extractors/current_member.rs` | the refusal targets (392-393, 421), as in sections 2 and 3 |
| `src/controllers/page.rs` | stays bare |
| `src/mailers/*` | unchanged until PR 2 |

**Test 1, the rendered walk** (`tests/requests/links.rs`,
`every_app_page_links_only_inside_its_org`). Seed one of everything (project,
task with a note and a reaction, saved view, group, DM, thread, folder, file,
login, meeting, notification of each kind, a pending member for Members). GET
each page and fragment as a member of Acme: Home, Notifications, Tasks
(list, board, a task, edit, new), saved view edit, Chat (list, a
conversation, a thread, feed, message actions, new group, new DM),
Documents (index, folder, login, activity), Roadmap (index, list, project
edit, new), Meetings (index, new), Members (index, rotate), More,
`/settings/claude`, `/orgs` (page and fragment), `/orgs/new`, the 404 page.
Pull every `href`, `action`, `formaction`, `hx-get|post|put|patch|delete`,
`ws-connect`, `sse-connect` and `data-*url` value. Each one that starts with
`/` must start with `/o/acme-studio/`, or be on a short allowlist: `/static/`,
`/login`, `/logout`, `/account/sign-out-everywhere`, `/admin`,
`/admin/leave`, `/join/`, `/oauth/`, `/`. Also every `Location` and
`HX-Redirect` the walk sees.

**Test 2, the source grep** (`no_hand_built_tenant_paths`). Reads
`assets/views/**/*.html`, `src/**/*.rs` and `frontend/app.ts`. Fails on a
string starting with `/<tenant section>` unless:

- in a template, it is inside `org_url(path=`;
- in Rust, it is on a `.add(` or `.prefix(` line in `routes()`, in
  `old_links.rs`, inside `org_path(`, `.url(`, or a model's org-relative link,
  or in `src/mailers/` (allowlisted with "PR 2");
- in `app.ts`, it is inside `orgUrl(`.

It is a heuristic, so the walk is the real proof. It catches templates the
walk doesn't render.

### 5. Public routes, `/orgs` and the switch

**Stay bare** (brief question 6): `/`, `/login`, `/signup`, `/forgot`,
`/reset/{token}`, `/join/{slug}` and `/join/{slug}/request`, `/oauth/*`,
`/.well-known/*`, `/mcp`, `/admin` and `/admin/*`, `/logout`,
`/account/sign-out-everywhere`, `/_health`, `/_ping`, `/_readiness`, and the
new `/robots.txt`. None of them use `CurrentMember`. Under `/o/{slug}/` they
are 404 (section 1, step 3). A walk test pins both.

**`/orgs` and friends.** The brief and the task list differ here, so this is
decision 2 and 3.

| Route | Recommend | Why |
|---|---|---|
| `GET /orgs` (the switcher list) | `/o/{slug}/orgs` | It ticks the current org and builds same-section links. Both come from the address. |
| `GET /orgs/new`, `POST /orgs` | `/o/{slug}/orgs/new`, `POST /o/{slug}/orgs` | PR 4's reviewers made both `CurrentMember`: drawn in the app layout, the new org copies this org's time zone, and the claim check covers the POST. Kept bare, they would need a third way of picking an org. A bare `GET /orgs/new` still works through the 1c redirect, so the address in the brief keeps working. |
| `GET /orgs/current` (the probe) | `/o/{slug}/orgs/current` | It must check this tab's org. The probe after a socket close, a stream reopen or a bfcache restore then asks "am I still in Acme?" through the normal address mode, and gets the status-page 409 when removed. Kept bare, it would need a claim-based mode of its own. |
| `POST /orgs/{id}/switch` | Bare, for one release, as a thin redirect | Tabs drawn before the deploy still have switch buttons and the stale banner's "Back" `fetch`. It checks `active_in`, sets the remember cookie, clears `acting_org`, and answers `/o/{slug}/dashboard` (`HX-Redirect` over HTMX). It no longer ends an unlock or closes connections. Under `/o/` it is 404, one line. Removed in PR 3. |

**PR 2's rule, restated for the security reviewer.** PR 2 said "never add a
GET that changes the org", because the cookie chose the org for every tab.
Now the address chooses it for one request. The rule becomes:

1. The address grants nothing. Access is `in_org_by_slug(slug, user)` active,
   or a super admin acting in that org, checked on every request.
2. A GET may update `current_org`, which only says where bare addresses and
   sign-in landing go. It never affects a request with `/o/{slug}/`, so no
   tab's organisation changes because of another tab, a link or an email.
3. No GET starts or ends acting, ends an unlock, closes connections or writes
   data. Acting starts only from `POST /admin/orgs/{id}/enter`.
4. A bare request that isn't a page load is refused, never redirected or
   replayed.
5. Every redirect target is `/o/` plus a slug from the database plus the
   request's own path and query, so it is always same-origin.

The cost of rule 2: a cross-site link to `/o/acme/tasks` (top-level, so the
`Lax` cookie goes along) sets `current_org` to Acme, if you are active there.
Your next bare bookmark then opens Acme instead of Beta. Nothing is read or
written. Accepted.

### 6. The switcher (1b)

Follows the mockup's switcher part. Server and `app.ts` only here.

- **Rows are links.** Each active row is `<a href="/o/{slug}{section}"
  role="menuitemradio">` instead of a `<button formaction=...>` in a form.
  The form in `orgs/_list.html` goes. Pending rows stay inert. "New
  organisation" links to `/o/{this slug}/orgs/new`.
- **The same section.** `src/views/links.rs::section_path(active)` maps the
  page's `active` key to a fixed list path:

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

  Item pages land on their list, since the item doesn't exist in the other
  org. The engineer confirms each page's `active` key, and that every list
  path opens for every role. A section that needs a role the row's member
  lacks goes to `/dashboard`.
- **How the list learns the section.** The trigger in `app.html` becomes
  `href="/o/{slug}/orgs?section={{ active }}"`. `app.ts`'s `loadOrgs`
  (`app.ts:1863`) fetches that `href` instead of the literal `/orgs`. The
  server only ever maps `section` through the table, so a hand-made value
  gives `/dashboard`, never a path from the request.
- **Acting.** The acting org's row stays ticked at the top. Each own-org row
  links to that org, and acting carries on in the acting org's tabs. The
  "Switching ends acting" note goes (brief question 7). The mockup gives the
  new line, if any.
- **`app.ts`.** Drop the `hx-post` switching states and the 404 toast. A row
  is a link. Keyboard handling stays: Enter follows the link. Cmd-click,
  middle-click and long-press open a new tab with no script.
- **`?switched=1` retires** (decision 4). The toast said where you landed
  when a switch changed every tab. Now the page names its org at the top.
  If the mockup keeps a cue, it says which.

**`enter_org`** (`src/controllers/orgs.rs`), used by create and the compat
switch: sets the remember cookie, clears `acting_org`, redirects. It no
longer calls `close_in_browser` or `end_unlock_for_session`. Other tabs keep
their own org, so there is nothing to close or end.

### 7. Tabs, the socket and the stream (1b)

- `ws-connect` in `chat/show.html` and `chat/thread.html` becomes
  `org_url(path="/chat/" ~ conversation.id ~ "/ws")` (and `?thread=`). The
  upgrade runs `CurrentMember` in address mode, so the socket's `org_id` is
  the tab's org. The htmx ws extension reconnects to the same URL.
- `new EventSource(orgUrl("/notifications/stream"))` in `app.ts:661`. The
  guard selector on line 660 matches the prefixed `hx-get`. The stream
  already filters by its org (PR 5, `events_for`), so each tab's bell counts
  its own org.
- The probes (`app.ts:207`, `1401`, `1560`) call `orgUrl("/orgs/current")`.

**`CloseScope::Browser` is no longer needed.** It existed so a switch could
close this browser's old-org connections when the cookie moved every tab.
Now no tab's org changes under it. Remove `users::Model::close_in_browser`,
the `Browser` variant in `src/data/live.rs`, and their unit tests (including
the `Browser` close in `notifications.rs:663`). `Account` (sign out
everywhere, reset, lock) and `Org` (removal) stay as they are.
`end_unlock_for_session` loses its only caller and goes too.

**The logins unlock** stays one per user, tied to one org (PR 2). Unlocking
in the Northwind tab ends the Acme tab's unlock. Accepted in the brief.

### 8. Home notices (1b)

`src/controllers/dashboard.rs` and `assets/views/dashboard/index.html`.

| Query | After |
|---|---|
| `?from=<id>` | Shown after a claim refusal or a bare refusal. `stale_notice` keeps its rules (names the org only if you are active or removed there). It adds the org's `slug`, and "Back to Acme" becomes a link to `/o/{slug}/dashboard`, not a switch form. The copy follows the mockup: "You switched organisation in another tab, and every tab follows" can no longer happen. The nameless notice (empty `from`) says this page was out of date and nothing was saved. |
| `?switched=1` | Retired (section 6). The query is ignored. |
| `?created=1` | Unchanged, at `/o/{new slug}/dashboard?created=1`. |
| `?requested=<id>` | Unchanged, at `/o/{last slug}/dashboard?requested=<id>`. |

`app.ts` drops `switched` from its `replaceState` block. `no-store` stays
whenever any of these is present.

The stale banner in `app.html` loses its "Back to {org}" `fetch` to the
switch (`app.ts:190`): the tab is already at its org's address. The gone
banner (PR 5) is unchanged. The copy is the mockup's.

The in-app not-found page (`errors/not_found.html`) drops "It may be in
another of your organisations. Switch from the menu, then open the link
again." In 1c the bare lookup has already tried your orgs. The mockup gives
the new line.

### 9. Search engines and the referrer (1a)

- **`GET /robots.txt`** in `src/controllers/page.rs`, `text/plain`:

  ```
  User-agent: *
  Disallow: /o/
  Disallow: /oauth/
  Disallow: /admin
  Disallow: /mcp
  ```

  `/join/` is not disallowed on purpose. A crawler must fetch a join page to
  see its `noindex`. A disallowed URL can still be indexed from links alone.
- **`noindex`.** `base.html` emits `<meta name="robots" content="noindex">`
  unless the template sets `{% block indexable %}`. Only `home/index.html`
  (the homepage) sets it. So app pages, join pages, status pages, sign-in and
  sign-up are all `noindex` (decision 9).
- **`Referrer-Policy: same-origin`**, set by the org-path layer on every
  response. Checked: nothing in `src/` or `config/` sets it. Loco's
  `secure_headers` middleware would, but it is off by default (`loco-rs`
  1.2.0 `controller/middleware/mod.rs:137`) and `config/production.yaml`
  doesn't enable it. Whether Railway's edge adds one is unverified. Browsers
  default to `strict-origin-when-cross-origin`, which already hides the path
  cross-site. `same-origin` sends nothing cross-site at all, so a link in a
  chat message or a task can't leak even the host. Same-site requests keep
  the full referrer. Nothing in the app reads `Referer`.

### 10. Docs

| File | Change |
|---|---|
| `docs/flows/accounts.md` | "Choosing an organisation": the address picks it, the cookie remembers the last, bare addresses redirect, the status pages per address, the identical not-found. Remove the switch-closes-connections text. |
| `docs/flows/super-admin.md` | Acting is per address. Own orgs work in other tabs. The banner only on the acting org. |
| `docs/flows/chat.md`, `docs/flows/notifications.md` | Socket and stream under `/o/{slug}/`. Bell links get the prefix when shown. No close on switch. |
| `docs/flows/claude-connector.md` | `/settings/claude` under `/o/`. The Allow page stays bare and uses the last org until PR 3. `/mcp` unchanged. |
| `docs/flows/documents.md`, `docs/flows/tasks.md` | Paths. |
| `docs/architecture/erd.md` | `organisations.slug`: "used in the join link and every app address". `notifications.link`: "path inside its organisation". |
| `docs/plans/multi-org-2.md`, `-3.md`, `-5.md` | A one-line note where they describe the switch closing connections and the cookie choosing the org: superseded by this plan. |

## Edge cases

- **Two tabs, Acme and Northwind.** Independent. Each posts to its own
  address with its own claim. The cookie follows whichever page loaded last,
  which only matters for bare addresses.
- **The same org in two tabs.** As today.
- **A copied address sent to a teammate in the same org.** Opens the same
  page. Sent to someone outside: the plain not-found page.
- **Not a member vs no such org.** Same single query, same status, body and
  headers. The 404 page names no org. HTMX gets the same 409.
- **Bad slug shapes** (`/o/Acme/`, `/o/%61cme/`, 65 bytes, `/o//tasks`).
  Not stripped, so the router's 404, the same for every slug.
- **`/o/acme/login`, `/o/acme/mcp`, `/o/acme/static/app.js`,
  `/o/acme/o/beta/tasks`.** 404.
- **`/o/acme` and `/o/acme/`.** 303 to `/o/acme/dashboard`, which then
  resolves like any page (so a stranger still gets the identical 404 there).
- **An unknown id under a real org** (`/o/acme/tasks/999999`). Acme's in-app
  404 page, as today.
- **Pending in Beta, active in Acme, opens `/o/beta/tasks`.** Beta's waiting
  page with a link to Acme.
- **Removed from Beta with a Beta tab open.** PR 5 closes Beta's sockets and
  streams. The tab's probe to `/o/beta/orgs/current` gets the 409 with
  `X-Org-Gone`. It moves to `/o/beta/dashboard`, Beta's removed page, with a
  link to Acme. Typed text stays behind the gone banner. Acme tabs are
  untouched.
- **Removed from your only org.** The removed page, signed in, Sign out only,
  as today.
- **Super admin, acting in X, also a member of A.** `/o/x/...` acts, with the
  banner. `/o/a/...` is their own membership, no banner. Leaving
  (`POST /admin/leave`) clears acting. An open X tab's next page load is the
  identical 404 and its HTMX calls the 409 to `/dashboard`.
- **Super admin, not acting, opens `/o/x/...` of an org they don't belong
  to.** The identical 404. The address never starts acting.
- **Super admin acting in an org they also belong to.** Acting wins, as today.
- **A claim that disagrees with the address.** Refused to the address org's
  Home with `?from=<claim>`. Only a bug or a forged request does this.
- **Tabs open across the 1b deploy.** Their links are bare and still work
  through today's cookie mode. Their writes are checked against the cookie,
  as today.
- **Tabs open across the 1c deploy.** Their bare writes and polls are refused
  once and land on Home of the org they claimed, with "nothing was saved".
  The old `app.js` guard keeps typed text behind its banner. Its "Back" posts
  to the compat switch.
- **Old bookmarks** (`/tasks`, `/chat`). The last org.
- **Old item links**, signed in. The item's org if you have a row there,
  else your last org, where it 404s.
- **The lookup reveals an id's org** in the `Location` header, only for orgs
  where the person has a row. For example, a private DM's id in your own org.
  The page itself still checks access and 404s. Ids are sequential and
  global already. For the security reviewer.
- **Odd bytes in a bare path** (`/chat/%2F%2Fevil`). Redirected to
  `/o/{slug}/chat/%2F%2Fevil`, which stays on this site and then fails its
  `Path` parse.
- **Signed out on any `/o/` or bare address.** `/login`, no `next`, then the
  default org. PR 2.
- **Sign in.** Clears `current_org` (PR 2), so it lands on the default org.
  Unchanged.
- **Sign out everywhere from a `/o/beta/more` tab.** Redirects to bare
  `/more?signed_out=1`, so it shows in the last org, normally Beta since you
  just loaded More there. Accepted.
- **Create a new org from Acme's tab.** Lands on `/o/new/dashboard?created=1`.
  Other tabs and their sockets carry on.
- **Bell notices written before the deploy.** Org-relative paths, prefixed
  when shown. Nothing to migrate.
- **A link pasted into a task or a message.** Bare: redirected. Full
  `/o/...`: opens in that org. Outbound links send no referrer.
- **HEAD.** Treated like GET.
- **A 303 cached by a browser.** Every redirect here is `no-store`.
- **A `current_org` cookie for an org you've left.** Only used by bare
  addresses, which fall through to the default and clear it, as today.

## Tests

The full suite stays green after each sub-PR. Route walks fill path
parameters with `1`, as in `tests/requests/stale_tab.rs`.

### 1a: `tests/requests/org_address.rs` (new), plus a unit test in `org_path.rs`

Alice is in Acme and Beta (`alice_in_two_orgs`). Kreepa is only in Acme.

- `the_layer_strips_once_and_keeps_the_query` (unit): `/o/acme/tasks/5?x=1`
  becomes `/tasks/5?x=1`. Bad shapes, non-tenant sections and a nested `/o/`
  pass through untouched.
- `path_parameters_reach_the_handlers`: `/o/acme/tasks/{id}` and
  `/o/acme/chat/{id}/threads/{message_id}` render.
- `two_tabs_write_into_their_own_org` (done-when 1): with `current_org` set
  to Beta, `POST /o/acme/tasks` with Acme's claim creates the task in Acme.
  `POST /o/beta/tasks` with Beta's claim creates it in Beta. No refusal, no
  notice.
- `the_cookie_never_overrides_the_address`: every combination of cookie and
  address shows the address's org.
- `a_strangers_org_and_a_made_up_slug_look_identical` (done-when 8): Kreepa on
  `/o/beta/tasks` and `/o/made-up-name/tasks`. Same status, body and headers
  apart from `Date`, for a page load and for HTMX.
- `bad_slug_shapes_are_the_routers_404`.
- `public_routes_are_404_under_o`: every route in the bare list, under
  `/o/acme/`.
- `o_slug_alone_goes_to_home`.
- `a_pending_member_sees_that_orgs_waiting_page` (done-when 9), with a link
  to their active org. The same for removed and declined. Over HTMX: 409,
  `HX-Redirect: /o/beta/dashboard`, `X-Org-Gone`.
- `a_super_admin_with_no_active_row_goes_to_admin`.
- `acting_applies_only_to_the_acting_orgs_address`: the banner on `/o/x/`,
  none on `/o/a/`. Not acting: `/o/x/` is the identical 404.
- `a_claim_for_another_org_is_refused_to_the_address_orgs_home`.
- `a_page_load_remembers_the_org`: `Set-Cookie: current_org=<beta>` with PR 2's
  attributes. None when it already matches, none for HTMX, POST or acting.
- `the_token_form_pin_is_checked_against_the_address`.
- `the_allow_page_without_an_address_uses_the_last_org`: the PR 2 OAuth tests
  pass unchanged.
- `an_upload_under_o_keeps_its_body_limit`.
- `robots_txt_disallows_app_paths_not_join`.
- `only_the_homepage_is_indexable`.
- `every_response_has_referrer_policy_same_origin`.
- `bare_addresses_still_work_as_before`: the existing suite, untouched, proves
  it.

### 1b: `tests/requests/links.rs` (new), plus updates

- `every_app_page_links_only_inside_its_org` and `no_hand_built_tenant_paths`
  (section 4).
- `bell_links_carry_the_org`: the list shows `/o/acme/tasks?open=N`, and
  `POST /o/acme/notifications/{id}/open` redirects there. A stored link that
  isn't a local path still goes to `/o/acme/notifications`.
- `switcher_rows_link_to_the_same_section`: from `/o/acme/tasks/58` the Beta
  row is `/o/beta/tasks`. From More, `/o/beta/more`. From `/o/acme/orgs/new`,
  `/o/beta/dashboard`. `?section=../../evil` gives `/dashboard`.
- `the_switcher_has_no_switch_form`.
- `creating_an_org_lands_in_it_and_closes_nothing`.
- `the_old_switch_post_lands_under_o_and_closes_nothing`, and is 404 under
  `/o/`.
- `there_is_still_no_get_that_switches`: `GET /orgs/{id}/switch` is 405.
- `join_open_is_a_link`, `admin_enter_lands_under_o`,
  `home_back_is_a_link`, `switched_is_ignored`.
- Live (`tests/requests/live_sign_out.rs`, real server):
  `two_orgs_chat_live_in_their_own_tabs` (done-when 2): sockets on
  `/o/acme/chat/{a}/ws` and `/o/beta/chat/{b}/ws`, a message in each arrives
  only on its own. `each_tabs_stream_counts_its_own_org`. The existing live
  tests move to `/o/` URLs.
- Updated: every test asserting a `Location` or `HX-Redirect` the app builds
  (about 100 strings), now through `m.at(...)`.
- Removed: the `CloseScope::Browser` unit tests and the PR 2 and PR 3 tests
  that expect a switch to close sockets or end an unlock.

### 1c: `tests/requests/bare_addresses.rs` (new), plus the migration

- `every_bare_tenant_page_load_redirects_keeping_path_and_query`: a walk over
  tenant GET routes. 303, `no-store`, `/o/{last}/...`.
- `old_item_links_find_their_org`: one case per row of the id-bearing table.
  With `current_org` = Acme, a Beta item lands in Beta (done-when 6).
- `an_old_link_into_an_org_you_are_pending_in_shows_its_waiting_page`.
- `an_item_in_a_strangers_org_looks_like_a_missing_id`.
- `every_bare_non_get_is_refused_and_saves_nothing`: a walk over non-GET
  tenant routes. 409 for HTMX and `X-Org-Id`, 303 for plain forms, never 307
  or 308, and no row written.
- `bare_htmx_gets_and_fetches_are_refused`, `a_bare_socket_upgrade_is_not_upgraded`,
  `a_bare_stream_is_not_a_stream`.
- `a_refusal_goes_to_the_claimed_org_when_active`, else the last org. It never
  echoes the raw claim.
- `odd_bytes_in_a_bare_path_stay_on_this_site`.
- `a_bookmark_of_tasks_opens_the_last_org` (done-when 7).
- `old_email_links_still_land` (section 3's table, signed in).
- `signed_out_links_go_to_sign_in_as_today`.
- `the_allow_page_is_still_bare`.
- `tests/requests/stale_tab.rs`: the walks run on `/o/` URLs. `EXEMPT` and
  `EXEMPT_GET` stay as they are, plus `/robots.txt`.
  `the_exempt_lists_match_real_routes` keeps them honest.

### The migration (1c)

About 534 request-test calls hit bare tenant paths. After 1c they would get
a 303 or a refusal.

- `TestMember` (`tests/requests/prepare_data.rs`) gains `slug`, and
  `fn at(&self, path: &str) -> String`, which calls the app's own
  `collab::views::links::org_path`. `member_from` follows Home's redirect to
  read the slug. `sign_up_member` returns it.
- The edit is one line per call: `request.get("/tasks")` becomes
  `request.get(&alice.at("/tasks"))`. `.as_member(&alice)` is unchanged.
  Tests that hold a bare `Cookie` switch to a `TestMember` first.
- A throwaway script does the mechanical part, file by file, for the
  variable names each file uses. The compiler finds the rest. The reviewer
  checks the diff changes nothing but the URL.
- Tests that are about the old cookie behaviour are rewritten, not migrated:
  the PR 2 cookie tests in `multi_org.rs`, PR 3's switch tests in `orgs.rs`,
  and `with_a_beta_cookie_acme_tasks_404_and_new_tasks_go_to_beta`.

**Not covered by `cargo test`.** No JS runner. `orgUrl`, the switcher as
links with the keyboard, the probes, the stream URL and the gone banner are
hand checks.

## Out of scope

PR 2 (links):

- Email links name their org: mentions, "all notifications", login activity,
  approval. Account emails (forgot, reset, password changed) stay bare.
- Old links for signed-out readers: sign-in comes back to the page.
- `safe_next` accepts `/o/{slug}/...`. Needs the security reviewer.
- Mention links like `/roadmap?lane=` that carry no id.

PR 3 (Claude):

- The Allow page's organisation picker. Until then it uses the last org.
- `/settings/claude` showing that org's connections only. PR 1 moves the
  route; the content is unchanged.
- Removing the compat `POST /orgs/{id}/switch`.

Not in this project (brief): editable slugs or old-slug redirects, custom
domains, unread dots per org, a combined bell, links in Claude tool results,
removing the stale-tab check, email verification on sign-up and join.

Also not here: fixing accents in new slugs ("Lumen Café" to `lumen-cafe`,
brief question 1). Decision 10.

## Risks

- **Size.** About 250 link edits and 534 test edits. The split keeps each
  sub-PR reviewable: 1a is the security core and small, 1b is wide but each
  edit is one helper call, 1c is mostly the test migration.
- **A missed link.** Before 1c it uses today's cookie mode. After 1c it is
  one extra redirect for a page, or a refused write with nothing saved.
  Never a write into another org, because bare writes are never redirected.
  The two link tests make this rare.
- **The outer router.** `fallback_service` wrapping must keep the socket
  upgrade, `ConnectInfo` (sign-in limits use the socket address locally),
  Loco's `remote_ip` and static middlewares, and graceful shutdown. Proved by
  the live tests and by hand on Railway.
- **Tera 2 internals.** `org_url` reading context through `State`, and inside
  components, is unverified (section 4). The engineer checks first. The
  fallback is a component prop.
- **The remember cookie is writable cross-site** for orgs you're active in
  (section 5). It only steers bare addresses.
- **Timing of the not-found.** One query either way. The super admin acting
  branch adds a query only for super admins with the cookie.
- **Production slugs.** The shape check assumes every slug is `[a-z0-9-]`,
  1 to 64 bytes, as `slugify` makes them. An older or hand-made slug outside
  that would 404. Checked before merging (see "Checks").
- **The old-item lookup** names an id's org to members of that org (edge
  cases).
- **Tabs across the 1c deploy** are refused once. Text typed in one is kept
  by the old guard, except in plain forms, which hold no long text (PR 3).
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
  revert keeps a tiny layer in `org_path.rs`: a GET or HEAD for
  `/o/{slug}/{rest}` answers 303 to `/{rest}` with its query, and anything
  else under `/o/` is a 404. It reads no database and sets no cookie. The
  bare page then opens in the cookie's org, as before this project.
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
SELECT id, slug FROM organisations WHERE slug !~ '^[a-z0-9-]{1,64}$';
SELECT slug, count(*) FROM organisations GROUP BY slug HAVING count(*) > 1;
```

By hand, locally, with the mockup's people: Acme Studio, Northwind Bakery,
Himalayan Ritual (pending), Raj in all three, Shusma, Kreepa.

- Acme Tasks in one tab and Northwind Tasks in another. Add a task in each,
  send a chat message in each, upload a file in each, watch each bell.
  Nothing moves to Home, and nothing lands in the wrong org.
- Unlock logins in Northwind, then try a reveal in the Acme tab: it asks to
  unlock again.
- The switcher from a task page, More and Home, by mouse, keyboard,
  Cmd-click and phone long-press.
- Kreepa on `/o/northwind-bakery/tasks` and `/o/made-up-name/tasks`.
- Himalayan Ritual's waiting page from its address.
- Remove Raj from Northwind with its tab open and text typed in the task
  dialog: the gone banner, then Northwind's removed page with a link to Acme.
- Super admin: enter Northwind from `/admin` in one tab, keep Acme in
  another. The banner shows only in Northwind.
- Bare `/tasks`, `/tasks?open=<a Northwind task>`, `/chat/<id>`,
  `/documents/files/<id>` with the cookie on Acme.
- A tab drawn on `main`, then the branch started: submit a form, poll, and
  click the old switcher.
- On the deployed app: the chat socket and the stream through Railway under
  `/o/`, `robots.txt`, and the `Referrer-Policy` header (`curl -sI`).

### Screenshots needed

From the local app with the realistic data above, on the `pr-screenshots`
branch under `org-url-1a/`, `org-url-1b/` and `org-url-1c/`. Desktop 1440px
and phone 390px, light and dark where colours or layout change. Headless
captures have no address bar, so take the side-by-side shots from a visible
Chrome window, or caption each shot with its address.

- 1a: Himalayan Ritual's waiting page with the link to Acme. The not-found
  page. Otherwise "No visible change".
- 1b: two windows side by side, Acme Tasks and Northwind Tasks, addresses
  showing. The switcher open on a task page (desktop menu, phone sheet), with
  the hovered row's address. The Home notice. The removed page after the
  gone banner.
- 1c: a bare bookmark and an old mention link, before and after, with the
  address they land on.

## Decisions for Raj

Each with a recommendation. The reviewers' views will be added here.

1. **Split PR 1 into 1a, 1b and 1c.** Each is safe alone (see "Summary" and
   "Rollback"). Recommend: yes. One PR would be about 800 changed call sites
   in one review.
2. **`/orgs/new` and `POST /orgs` move under `/o/{slug}/`.** The brief listed
   them as account pages outside `/o/`. Recommend: move them, for the reasons
   in section 5. A bare `/orgs/new` still works through the redirect.
3. **The probe `/orgs/current` moves under `/o/{slug}/`.** Recommend: yes. It
   has to check the tab's own org.
4. **Retire the "You're now in Northwind" toast (`?switched=1`).** Recommend:
   yes. Every page names its org. The mockup can add a lighter cue.
5. **Keep `POST /orgs/{id}/switch` as a thin redirect until PR 3.**
   Recommend: yes, for tabs drawn before the deploy. It no longer closes
   connections or ends an unlock.
6. **Old item links find their org in PR 1, for signed-in readers.** The brief
   put "old item links find their organisation" in PR 2. Recommend: PR 1 (in
   1c), because 1c's bare redirect would otherwise send item links to the
   last org and 404. PR 2 keeps the signed-out part.
7. **An old link into an org where you're pending, removed or declined opens
   that org's status page.** Recommend: yes. It matches the brief's rule for
   `/o/` links and reveals only your own row.
8. **In-app bell links get the prefix in PR 1.** The brief grouped
   "notification links" with PR 2. Recommend: the bell in PR 1 (it is in-app
   and each row knows its org), emails in PR 2.
9. **`noindex` on every page except the homepage, and
   `Referrer-Policy: same-origin`.** The brief asked for `noindex` on app and
   join pages. Recommend: every page but the homepage. Sign-in and sign-up
   gain nothing from search.
10. **Accents in new slugs** ("Lumen Café" to `lumen-cafe`, brief question 1).
    Not placed in any PR yet. Recommend: a small change of its own after
    PR 1, since `slugify` needs a transliteration table and its own tests.
11. **Remove `CloseScope::Browser` and `close_in_browser`.** Recommend: yes, in
    1b. Nothing calls them once the switch stops closing connections.
