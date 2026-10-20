# Multi-org 3: the stale-tab org check, and the switcher

Status: Approved by Raj 19 Oct (mockup recommendations); reviews folded in (staff: approve with changes, applied; security: approve with changes, applied) · PR 3 of 7 · Owner: Raj · 19 October 2026


> Superseded in part by `docs/plans/org-url-1.md` (20 October 2026): the
> address `/o/{slug}/` now picks the organisation, the `current_org` cookie
> only remembers the last one, and a switch no longer closes connections.

## Summary

PR 2 (#43) added the `current_org` cookie and `POST /orgs/{id}/switch`, with
no button. This PR does two things.

**A. The stale-tab org check.** This is the hard gate before PR 4. Today a tab
left open on Acme posts into Beta after a switch in another tab. After this
PR:

- Every app page says which org it was drawn for: `data-org-id` and
  `hx-headers='{"X-Org-Id": <id>}'` on `<body>`.
- HTMX requests carry that header. Plain urlencoded forms get a hidden `_org`
  field from a submit listener in `frontend/app.ts`. The raw `fetch` calls
  add the header through one helper.
- A small layer in `src/initializers/current_org.rs` lifts `_org` out of
  urlencoded bodies into a request extension. `CurrentMember` compares the
  claim with the org it resolved.
- Every refusal sends the tab to Home in the org it is really in:
  `/dashboard?from=<claim>`. Home shows the mockup's notice card: which org
  the tab was showing, "nothing was saved", "Back to Acme" and Dismiss.
- A missing value on a non-GET request is refused the same way (fails
  closed). A missing value on a GET is allowed.
- If the tab holds typed text or an upload, it does not move. One banner
  appears in place, and the text stays in the form.

**B. The switcher.** Built to the approved mockup,
`docs/designs/org-switcher.html`. With two or more orgs (or one plus a
pending request), the org name at the top of the sidebar, and on the phone's
Home header and More page, opens a menu or sheet of your organisations.
Picking one switches and lands on Home with "You're now in Northwind Bakery".

No migration. Nobody has two orgs yet, so in production nothing looks
different until PR 4: with one org the name stays plain text, as today.

## Where the check lives

**Decision: the check is in `CurrentMember`. A thin layer moves `_org` from
form bodies into a request extension first.**

`CurrentMember` is the only place that knows the resolved org (acting, then
the cookie, then the default). Every tenant route already runs it, including
HTMX GETs. So the comparison goes there. The one thing an extractor can't do
is read the body without consuming it. The layer does that part.

### The layer (`src/initializers/current_org.rs`)

The existing `current-org` layer already puts `StaleCurrentOrg` in every
request. It also inserts an `OrgClaim`, one of `Missing`, `Org(i64)` or
`Garbage`:

1. **Header.** One `X-Org-Id` header that parses as an `i64`: `Org(id)`.
   Several `X-Org-Id` headers, an empty one, or one that does not parse:
   `Garbage`. The body is not touched.
2. **Body.** All of these: no header, a method other than GET, HEAD or
   OPTIONS, the session cookie (`auth_token`) present, and a content type
   whose base is `application/x-www-form-urlencoded` (case-insensitive,
   parameters such as `; charset=UTF-8` ignored). Then:
   - A `Content-Length` over the limit is a 413 before anything is read.
   - Otherwise `axum::body::to_bytes(body, FORM_BODY_LIMIT)`. Going over the
     limit while reading (a chunked body) is also a 413.
   - The first `_org` from `url::form_urlencoded::parse`: `Org(id)` if it
     parses, `Garbage` if present but not a number, `Missing` if absent.
   - The same bytes are put back as the body, so `Form` reads them again.
3. Otherwise `Missing`. Multipart and JSON bodies are never buffered, and
   neither is any request without a session.

`FORM_BODY_LIMIT = 2_000_000`: one constant matching Loco's default payload
limit (`limit_payload.rs`, `DefaultBodyLimitKind::Limit(2_000_000)`), so the
layer never accepts or refuses differently from `Form`.

The layer must not panic. Every failure is a 413, a 400 for a body that
fails to read, or `Missing` or `Garbage`. No `unwrap`.

No new crate: `url` is already a dependency.

**Order.** Loco applies each initializer's `after_routes` in list order
(`loco-rs` 1.2.0 `boot.rs:547`), and each `.layer` wraps the router so far.
So the last one registered runs first. `src/app.rs` moves
`OriginCheckInitializer` after `CurrentOrgInitializer`, so a cross-site POST
is refused before its body is buffered.

### The check (`src/extractors/current_member.rs`)

After the org is resolved, before `Ok(member)` and before any handler work.
It is a pure comparison of the claim with `member.org.id`. No database call
decides it.

| Request | Claim | Result |
|---|---|---|
| GET, HEAD or OPTIONS, not HTMX | anything | No check. Page loads, links from emails, bookmarks, file downloads, the chat socket upgrade and the notification stream all pass. |
| GET, HEAD or OPTIONS, HTMX (`HX-Request: true`) | `Missing` | Allowed. Tabs drawn before the deploy keep polling. |
| GET, HEAD or OPTIONS, HTMX | `Org(resolved)` | Allowed. |
| GET, HEAD or OPTIONS, HTMX | anything else | Refused. |
| Any other method | `Org(resolved)` | Allowed. |
| Any other method | `Missing`, `Garbage` or another org | Refused. |

- "The resolved org" is the acting org while a super admin is acting. So a
  tab drawn for the admin's own org is stale while they are inside another.
- The waiting, removed and declined pages and the `/admin` redirect return
  before the check. They have no org to compare.
- One info log line on each refusal: user id, the claimed org (only an
  `i64`, else "garbage" or "missing"), the resolved org, and the method.
  Numbers only, as in the switch log.

**The refusal.** Always to `/dashboard?from=<id>`, where `<id>` is the claim
only when it is `Org(id)`. For `Missing` and `Garbage` it is
`/dashboard?from=` with nothing after it. Nothing from the request goes into
the URL except a parsed `i64`, so there is no open redirect.

- If the request has `HX-Request` or `X-Org-Id`: 409 with
  `HX-Redirect: /dashboard?from=<id>`. Raw `fetch` calls send `X-Org-Id` but
  not `HX-Request`, so they get this one too.
- Otherwise (a plain form): 303 with `Location: /dashboard?from=<id>`.
- Both with `Cache-Control: no-store`.

There is no `tenancy/stale_org.html` and no `HX-Refresh`. Why Home and not a
reload: reloading an address with an Acme id (`/tasks/58`, `/chat/12`) in
Beta gives a 404 page. Home always works, and it is where a switch lands
too. Raj chose this with the mockup.

htmx 2.0.11 acts on `HX-Redirect` before it looks at the status, so the
`[45]..` "don't swap" rule in `base.html` doesn't stop it (checked in
`assets/static/vendor/htmx.min.js`, function `Vn`).

### The notice on Home (`src/controllers/dashboard.rs`)

`dashboard::index` reads `from: Option<String>`.

- Absent: no notice.
- Parses as the current org's id: no notice. The tab is back where it was.
- Parses as another id: `active_in(id, user)`.
  - Found: the notice names that org and offers "Back to Acme".
  - Not found, or the query errors: "another organisation" and no Back. An
    error here only changes the wording. The refusal has already happened.
- Empty or not a number: the notice without a name, and no Back.

The card follows mockup section 4: "This tab was showing Acme Studio. You're
now in Northwind Bakery.", "You switched organisation in another tab, and
every tab follows.", that nothing was saved, "Back to Acme Studio" and
Dismiss. `role="status"`, and it takes no focus.

- **Back** is a POST form to `/orgs/{id}/switch`. Its action is built from
  the parsed `i64`, never from the raw string.
- **No Back for an acting super admin outside their own org.** Acting in an
  org you don't belong to, `active_in` finds nothing, so the rule above
  already leaves Back out. To get back there they enter it again from
  `/admin`.
- **Dismiss** removes the card. `app.ts` drops `?from=` from the address
  with `history.replaceState`, so a reload doesn't repeat it.
- The response has `Cache-Control: no-store` whenever `from` or `switched` is
  in the query.

### `PinnedMember`: two forms keep their own check

The Claude Allow page (`POST /oauth/authorize`) and the personal token form
(`POST /settings/claude/tokens`) already pin their org with a hidden
`org_id` (PR 2). Their checks are the same rule, with better outcomes:

- Deny on the Allow page still goes back to Claude with `access_denied`.
- The token form keeps the typed name and shows its own message.

The generic check would answer first and lose both. So these two handlers
take a new `PinnedMember(CurrentMember)` extractor. It resolves the org
exactly like `CurrentMember` but skips the claim check. Its doc comment
says: only for forms that call `shown_for_this_org` themselves, and every
use is listed in the route walk test. A third use needs a deliberate change
to that list.

### Alternatives considered

1. **Header only.** Make every request carry `X-Org-Id` by turning plain
   forms into `fetch` calls, or by `hx-boost`ing them.
   - Converting forms means re-implementing redirects, error pages and
     navigation in `app.ts` for about 15 plain forms.
   - `hx-boost` swaps the response into `<body>`, and the `[45]..` rule means
     error pages would not show. Boosted forms also change history.
   - Both still fail without JS, so the server needs the fail-closed rule
     anyway.
   - Rejected: more moving parts in the browser than a small server layer.
2. **The layer, plus the check in `CurrentMember`.** Chosen.
3. **Per-handler checks.** Add `_org` to every form struct and call
   `shown_for_this_org` in about 55 POST handlers. HTMX GETs would need it
   too. A forgotten handler skips the check silently. Rejected.
4. **The whole check in middleware.** It would have to repeat
   `CurrentMember`'s resolution (acting, cookie, default) and its queries.
   Rejected.

## Exempt routes

None of these run `CurrentMember`, so the check never reaches them. The walk
test pins the list.

| Route | Why |
|---|---|
| `POST /login`, `POST /signup`, `POST /join/{slug}` | Not signed in to an org yet. |
| `POST /forgot`, `POST /reset/{token}` | Signed out. |
| `POST /logout`, `POST /account/sign-out-everywhere` | Account-wide, `CurrentUser`. Must work from any tab. |
| `POST /orgs/{id}/switch` | `CurrentUser`. Names its target, so a stale tab may switch. |
| `POST /admin/orgs/{id}/enter`, `POST /admin/leave` | `CurrentUser`. Name their target. |
| `POST /oauth/register`, `POST /oauth/token` | Called by Claude, no session. |
| `POST /mcp` | Bearer token. Its org comes from the token, not the cookie. |
| `POST /oauth/authorize`, `POST /settings/claude/tokens` | `PinnedMember`: their own pin (above). |
| GET page loads that aren't HTMX | Links must work. Nothing is written. |
| `/_health`, `/_ping`, `/.well-known/*`, `/` | No session or no org. |

There are no webhooks.

## Websocket and SSE

**Chat socket: no check needed.**

- The socket's org is fixed at upgrade. The upgrade only succeeds for a
  conversation in the cookie's org (`find_for_member(member.org.id, id,
  user)`). The page names that conversation. So the socket, the page and
  the conversation always share an org.
- If a switch happens before the upgrade, the upgrade is refused. If it
  happens after, the close event ends the socket and its reconnect is
  refused (PR 2). Inside the "Switch race" window a message still lands in
  the Acme conversation the page shows. The person is still an Acme member.
  Not a cross-org write.
- The htmx ws extension sends `HEADERS` in each message, and these now
  include `X-Org-Id`. The server keeps ignoring them. A cross-check is
  deferred (see "Out of scope").

PR 2 left one gap: a tab whose reconnect is refused stops getting messages
but never moves. `app.ts` now sends one probe on `htmx:wsClose` (see "`GET
/orgs/current`"). A stale tab gets the refusal and goes to Home.

**Notification stream: no check needed.** It carries no data, only "something
changed" for this user. The badge it triggers is an HTMX GET, which is
checked. On a switch the stream is closed and the browser reconnects.
`app.ts` probes when the stream reopens, so a stale tab moves within seconds
rather than at the next 60-second poll.

## The switcher

This follows mockup sections 1 to 6 on every point. The plan only covers
what the server and `app.ts` must do for it.

### When it shows

- **Plain name, no chevron, no More row:** one active org and nothing
  pending, exactly as today. Also a super admin acting with no membership of
  their own.
- **The switcher:** two or more active orgs, or one plus a pending request
  (only possible after PR 6), or a super admin acting who has any membership
  of their own.

`CurrentMember` gets a field `org_menu: bool`, and `page()` passes it to the
layout. It is worked out only on non-HTMX GET requests, the ones that draw a
full page. HTMX requests and POSTs skip it and leave it `false`.

The query is `memberships::Model::switcher_counts(db, user_id)`:
`SELECT count(*) FILTER (WHERE status = 'active'), count(*) FILTER (WHERE
status = 'pending') FROM memberships WHERE user_id = $1`. It uses
`idx-memberships-user` and reads one to a handful of rows.

**Cost:** one extra indexed query per full page load, likely well under a
millisecond. That is unmeasured; the engineer logs a timing locally.
Non-HTMX GETs that aren't pages pay it too: file downloads, the chat socket
upgrade, the notification stream. They are a few per page view. Accepted,
to keep it in one place.

### The list

`memberships::Model::for_user_with_orgs(db, user_id)` returns `Vec<(Model,
organisations::Model)>` in one query. It joins memberships to organisations,
keeps active and pending only, and sorts in SQL: active first, then
`lower(organisations.name)`, then id. So it reads active A to Z, then
pending A to Z. Declined and removed are not returned.

### `GET /orgs` (in `src/controllers/orgs.rs`)

- `CurrentMember`, so a stale tab opening the menu is moved first.
- HTMX: the list fragment `orgs/_list.html`, loaded when the menu or sheet
  first opens. Otherwise the full page `orgs/index.html`, which is only the
  no-JS fallback. The trigger is a link to `/orgs` that `app.ts` turns into
  the menu.
- `Cache-Control: no-store` on both, and `Vary: HX-Request`.
- Rows follow mockup sections 1 and 6: initial, name, role; a tick on the
  current org; pending rows greyed with "Waiting for approval" and
  `aria-disabled`. Each active row is a
  `<button formaction="/orgs/{id}/switch">` inside one `<form method="post">`,
  which works without JS.
- **Acting** (section 5): "Acting in" with the acting org, ticked, role
  "Platform admin". Then "Your organisations". Then "Switching ends acting
  in Acme Studio." Leave stays in the banner only.
- **Desktop** (section 1): the sidebar's org link (`app.html` line 7) becomes
  the menu button, `aria-haspopup="menu"`, with the mockup's keyboard
  handling in `app.ts`.
- **Phone** (section 2): Home's header name becomes a button, and More gets
  one row at the top. Both open the existing `dialog.sheet`, titled "Your
  organisations".
- No create or join rows (section 7).

### Switching from the menu

- Each row button also has `hx-post="/orgs/{id}/switch"`. The plain
  `formaction` stays as the fallback.
- While the request runs: a spinner and "Switching…" on that row, the others
  dimmed (section 6).
- Success: the endpoint answers `HX-Redirect: /dashboard?switched=1`.
- 404 (the membership changed): toast "You can't open Northwind Bakery any
  more." and the list reloads.
- No connection: toast "Couldn't switch to Northwind Bakery. Check your
  connection and try again." The list stays open.

### `GET /orgs/current`

Returns 204 through `CurrentMember`, with `Cache-Control: no-store`. It is
the probe `app.ts` sends after the chat socket closes, when the notification
stream reopens, on a bfcache restore and before each upload starts. A stale
tab gets the refusal. Cheaper and clearer than borrowing a feed or badge URL.

### After a switch

- `switch` redirects to `/dashboard?switched=1` instead of `/dashboard`
  (`HX-Redirect` for HTMX). The target is a constant, and the switch ignores
  any `next` parameter.
- `dashboard::index` reads `switched` and adds the toast "You're now in
  Northwind Bakery", built from `member.org.name`, the resolved org. This is
  the same pattern as `/members?removed=` and `/more?signed_out=1`: the URL
  only says "show it", and the name comes from the database. A normal (dark)
  toast, per section 3.
- `app.ts` removes `?switched=1` with `history.replaceState`.
- `/dashboard?switched=1` typed by hand shows your current org and changes
  nothing. There is still no GET that switches.

## The typed-text guard

Raj answered "keep it to copy" for text being typed when a tab goes stale.
The mockup proposed copying drafts into `sessionStorage` and showing them on
Home. The reviews preferred not to put typed text, which may be sensitive,
into browser storage. So the text stays where it is, in the form, and the
tab doesn't move. That still keeps what Raj typed, which was the point of
his answer.

**In `app.ts`, on `htmx:beforeOnLoad` with status 409 and `HX-Redirect`:**

- If there is unsaved work, cancel the event and show one banner in place.
  Unsaved work is either of these:
  - a text field (an `input` that is not hidden, password or
    `[data-filter]`, or a `textarea`) whose value differs from its
    `defaultValue`, in a visible form or a form inside an open `<dialog>`;
  - a queued or in-progress document upload (`uploadQueue` not empty, or
    `currentUpload` set).
- Otherwise let htmx follow the redirect.

Cancelling `htmx:beforeOnLoad` returns before `HX-Redirect` is handled
(checked in htmx 2.0.11, `Vn`).

**The banner.** One per page. Later refusals while it shows (a badge poll,
say) are cancelled without a second banner. It says the tab was showing Acme,
the browser is now in another organisation, and nothing was saved. Its text
and look come from mockup section 4's notice. It offers:

- **Back to Acme.** A `fetch` POST to `/orgs/{id}/switch` with
  `redirect: "manual"` and `credentials: "same-origin"`, where `{id}` is
  `document.body.dataset.orgId`. A response of type `opaqueredirect` means
  it worked. The browser stores the cookie from that response (hand check),
  so this tab is valid again: the banner goes and the form can be saved. A
  404 gives the section 6 toast. Not offered when the page was drawn while
  acting (`me.acting`), since the admin may not belong there. They go back
  through `/admin`.
- **Go to Home.** Follows the `HX-Redirect` it was holding.

The banner is a `<template>` in `app.html`, filled in from the page's own
org name. The page never needs the new org's name.

**Plain forms** can't be guarded: the browser has already left the page when
the 303 arrives. The plain forms left are short (mark read, open a
notification, show the guide, revoke a grant). The token form keeps its
typed name through `PinnedMember`.

## Raw `fetch` calls

`orgHeaders()` returns `{ "X-Org-Id": document.body.dataset.orgId }` and
nothing else. It never adds `HX-Request`. `redirect_response`
(`src/extractors/session.rs:134`) answers `HX-Request` with `HX-Redirect`
instead of a 303. That would change how these calls see redirects, such as
the sign-in redirect `secretRefused` relies on.

Used by:

- `postForm` (`app.ts` ~line 1352): reveal, copy and unlock.
- The chat message actions GET (~line 107).
- The reaction POST (~line 248).

One shared check, `staleRefusal(res)`, looks for `res.status === 409` with an
`HX-Redirect` header. It runs the typed-text guard, then goes to that
address if the guard allows. Each caller runs it first, before its own
`ok` and `redirected` checks.

## Uploads

- The upload form is HTMX, so its header carries the org.
- **The probe first.** `startNextUpload` (~line 1164) awaits `GET
  /orgs/current` before `requestSubmit`. A stale tab is caught before it
  sends a large body, and the queue stays put behind the banner.
- **The guard** counts a queued or in-progress upload as unsaved work.
- **A refusal mid-upload.** The server answers 409 before reading the body.
  Browsers and the Railway edge may show that as a dropped connection
  (status 0, "Stopped part way") rather than the 409. Hand check with a
  ~20 MB file, locally and on the deployed app. The probe makes this rare.
- **No JS.** A multipart form without `X-Org-Id` always fails closed. The
  layer never reads multipart bodies, so the submit listener skips
  `enctype="multipart/form-data"` forms.

## What changes, file by file

| File | Change |
|---|---|
| `src/initializers/current_org.rs` | `OrgClaim`, `FORM_BODY_LIMIT` and the body lifting (above). The module comment covers both jobs. |
| `src/app.rs` | Move `OriginCheckInitializer` after `CurrentOrgInitializer`, with a comment on why. |
| `src/extractors/current_member.rs` | Constants `ORG_HEADER = "x-org-id"` and `ORG_FIELD = "_org"`. Move today's body of `from_request_parts` into `async fn resolve(parts, ctx)`, which also sets `org_menu`. `CurrentMember` = `resolve` + `check_claim`. New `PinnedMember` = `resolve` only. `page()` adds `current_org_id` and `org_menu`. The refusal responses. |
| `src/controllers/oauth.rs` | `authorize` (POST) takes `PinnedMember`. The GET page is unchanged. |
| `src/controllers/claude.rs` | `create_token` takes `PinnedMember`. |
| `src/controllers/orgs.rs` | `GET /orgs`, `GET /orgs/current`. `switch` redirects to `/dashboard?switched=1`. |
| `src/controllers/dashboard.rs` | `Query` with `switched` and `from`. The toast, the notice, `no-store`. |
| `src/models/memberships.rs` | `switcher_counts`, `for_user_with_orgs`. |
| `assets/views/layouts/base.html` | On `<body>`, outside `{% block body_attrs %}`, only when `current_org_id` is set: `data-org-id="{{ current_org_id }}"` and `hx-headers='{"X-Org-Id": {{ current_org_id }}}'`. Plain JSON with the integer, never `js:`. It uses a separate key, not `org.id`, because the join and waiting pages pass their own `org`. Nothing in the app swaps `<body>` (no `hx-boost`, no `hx-target="body"`), so the attributes always match the page. |
| `assets/views/layouts/app.html` | The sidebar menu button and menu, and the stale banner `<template>`. |
| `assets/views/dashboard/index.html` | The notice card, and the phone header button. |
| `assets/views/orgs/index.html`, `assets/views/orgs/_list.html` (new) | The list. |
| `assets/views/more/index.html` | The "Organisation" row when `org_menu` is set. |
| `frontend/app.ts` | The submit listener (urlencoded forms only), `orgHeaders`, `staleRefusal`, the typed-text guard and banner, the probes, the upload probe, the menu and sheet with keyboard handling, the `hx-post` switch states, and `replaceState` for `?switched=` and `?from=`. |
| `frontend/styles/app.css` | Switcher, notice and banner styles from the mockup. |
| `tests/requests/prepare_data.rs` | `TestMember { cookie, org_id }`, `sign_up_member(...)` returning one, and `.as_member(&m)` on `TestRequest`, which adds the cookie and `X-Org-Id` together. |
| `tests/requests/stale_tab.rs`, `tests/requests/orgs.rs` (new), `tests/requests/mod.rs` | New tests. |
| Every request test that POSTs to a tenant route | `.add_header(cookie.0.clone(), cookie.1.clone())` becomes `.as_member(&alice)`. About 200 call sites, one line each. |
| `tests/requests/multi_org.rs` | `with_a_beta_cookie_acme_tasks_404_and_new_tasks_go_to_beta` now expects a refusal and no task. The switch tests expect `/dashboard?switched=1`. |
| `tests/models/memberships.rs` | Tests for the two new queries. |
| `docs/flows/accounts.md` | Replace "Until the stale-tab check ships" with a "Stale tabs" section. The switcher exists now. |
| `docs/flows/super-admin.md` | The check compares against the acting org. The menu while acting. |
| `docs/flows/chat.md` | Why the socket needs no check, and the probe on close. |
| `docs/flows/claude-connector.md` | The Allow page keeps its own pin and skips the generic check. |
| `docs/plans/multi-org-1.md`, `docs/plans/multi-org-2.md` | Gate wording updated in the plan commit (see below). |
| `docs/designs/org-switcher.html` | A short note in section 4, added in the plan commit: drafts stay in the form with a banner, per the reviews. The `sessionStorage` part is superseded. |

### The submit listener

It runs in the capture phase, on `document`. It acts on a `method="post"`
form with no `hx-post`, `ws-send` or `data-unlock-form`, and not
`enctype="multipart/form-data"`. It sets a hidden `_org` input to
`document.body.dataset.orgId`, creating it if needed. Only when the body has
an org id.

`htmx.ajax` calls (task drag, guide welcomed, feed reload, login sheet) use
`<body>` as their source, so they inherit `hx-headers` with no change. htmx's
`getHeaders` reads `hx-headers` up the tree (checked in 2.0.11, `yn`).

### The probes

`htmx.ajax("GET", "/orgs/current", { swap: "none" })`, only when
`document.body.dataset.orgId` is set:

- on `htmx:wsClose`;
- when the notification `EventSource` fires `open` after its first;
- on `pageshow` with `persisted`;
- in `startNextUpload`, before the upload starts.

## Edge cases

- **Two tabs.** Tab 1 shows Acme. Tab 2 switches to Beta.
  - Tab 1's next HTMX request is refused. So are its badge poll (60 s), its
    notification stream reopening, and its chat socket closing. It goes to
    Home in Beta with the notice, unless it holds typed text or an upload
    (banner instead).
  - A plain form from tab 1 is refused and goes to Home with the notice.
    Nothing is saved.
  - A link in tab 1 is a page load, unchecked. Id links (`/tasks/5`) 404 in
    Beta, as today. Until PR 7.
  - Reads in tab 1 before it moves show Acme data. The person is still an
    Acme member, so this is not a leak.
- **"Back to Acme" makes the other tab stale.** Tab 2 is then on the wrong
  org and gets the same notice next time it loads anything. Accepted with the
  mockup.
- **Back button and bfcache.** A restored page still has Acme's id. The
  `pageshow` probe catches it straight away. Without bfcache the page is
  fetched again and is simply Beta.
- **A form being typed in.** HTMX submit or a poll: the banner shows, and the
  text stays. A plain form: the tab moves, but those forms hold no long text.
- **Acting.** The admin enters org X in tab 2. Tab 1 (their own org A) is
  stale and moves to Home in X with the notice. Back to A is offered, since
  they are an active member of A, and switching ends acting. The reverse (a
  tab drawn while acting in X, then a switch) offers no Back.
- **The org was deleted.** `from` names a deleted org. It is not found, so
  the notice says "another organisation" and offers no Back.
- **Pending, removed or declined in the claimed org.** Not active, so not
  named and no Back. Removal still signs you out everywhere until PR 5.
- **No active membership left.** The waiting or removed page shows before the
  check, as today. Its only form is Sign out, which is exempt.
- **Tabs open across the deploy.** Old pages have no org id. Their HTMX POSTs
  and plain forms are refused once and go to Home with the nameless notice.
  The old `app.js` has no guard, so text typed in such a tab at that moment
  is lost. Nobody has two orgs yet, so this is the only cost.
- **Forged claim.** Same as a stale one. It can only make a request fail.
  The claim never chooses the org; the cookie and membership still do. A
  hand-made `?from=` only shows a notice, naming an org only if you are in it.
- **No JS.** Plain urlencoded forms carry no `_org` and fail closed. The
  switcher falls back to the `/orgs` page and plain forms.
- **Two claims.** Header and `_org` both present: the header wins and the
  body is not read.
- **A reload loop.** A page whose own requests were always refused would
  keep moving to Home. Home is drawn for the resolved org, so its requests
  match. No loop is expected. A guard is deferred.

## Tests

`tests/requests/stale_tab.rs` (new). Alice is in Acme and Beta
(`alice_in_two_orgs`), with `current_org` = Beta unless said otherwise.

**Route walks**, over `App::routes(&ctx).collect()`. Path parameters are
filled with `1`; the check runs before the handler reads them.

- `every_non_get_route_refuses_a_header_from_another_org`: every route
  method other than GET, HEAD and OPTIONS, not in `EXEMPT`, gets
  `X-Org-Id: <acme>` and an empty form. Expects 409 with `HX-Redirect:
  /dashboard?from=<acme>` and `no-store`.
- `every_non_get_route_refuses_a_form_from_another_org`: the same walk with
  `_org=<acme>` in a urlencoded body and no header. Expects 303 to
  `/dashboard?from=<acme>`.
- `every_non_get_route_refuses_a_request_with_no_org`: neither. 303 to
  `/dashboard?from=`.
- `every_htmx_get_from_another_org_is_sent_home`: GET routes not in
  `EXEMPT_GET`, with `HX-Request` and the Acme claim. 409 with
  `HX-Redirect`. `EXEMPT_GET` is the routes without `CurrentMember`: `/`,
  `/login`, `/forgot`, `/reset/{token}`, `/signup`, `/join/{slug}`,
  `/admin`, `/_health`, `/_ping`, `/.well-known/*`, and `/oauth/authorize`
  (it checks its query before it looks at the member).
- `the_exempt_lists_match_real_routes`: every entry in `EXEMPT` and
  `EXEMPT_GET` exists in the route list, so the lists can't go stale. The
  two `PinnedMember` routes are listed with that reason.
- `exempt_routes_ignore_the_claim`: each exempt POST with a wrong
  `X-Org-Id` is not refused by the check (login, signup, join, forgot,
  reset, logout, sign out everywhere, switch, admin enter and leave, oauth
  register and token, mcp).

**Claims and responses:**

- `the_right_org_goes_through_by_header_and_by_field`: `POST /tasks` with
  the header (HTMX) and with `_org` (plain) both create the task in Beta.
- `an_acme_tab_cannot_post_a_task_into_beta`: refused, no task in either
  org.
- `a_raw_fetch_with_only_the_org_header_gets_hx_redirect`: no `HX-Request`
  and a wrong `X-Org-Id`. 409 with `HX-Redirect`, not a 303.
- `garbage_claims_are_refused`: `X-Org-Id: abc`, an empty `X-Org-Id`, two
  `X-Org-Id` headers, and `_org=abc`. Each goes to `/dashboard?from=` with
  nothing after it.
- `the_refusal_never_echoes_the_raw_claim`: `X-Org-Id: //evil.example` and
  `_org=%2F%2Fevil` give `/dashboard?from=`.
- `the_content_type_is_matched_by_its_base`:
  `Application/X-WWW-Form-Urlencoded; charset=UTF-8` with `_org` is read.
- `a_body_over_the_limit_is_413_before_reading`: `Content-Length:
  2000001`.
- `a_large_form_is_still_read`: a task with a ~100 KB description and `_org`
  last in the body is created.
- `a_post_without_a_session_is_not_buffered`: `POST /login` with `_org` in
  its body still signs in, and the field is ignored.
- `a_cross_site_post_is_refused_before_the_layer`: a foreign `Origin` gets
  the origin check's 403, not a 409 or 413.
- `polling_badges_move_a_stale_tab`: `/notifications/unread` and
  `/chat/unread` with `HX-Request` and the Acme claim.
- `an_htmx_get_without_a_claim_still_works`.
- `options_is_treated_like_get`.
- `a_page_load_ignores_the_claim`: `GET /tasks`, no `HX-Request`, Acme
  header. 200, showing Beta.
- `a_document_upload_from_another_org_is_refused_and_nothing_is_stored`:
  multipart `POST /documents/folders/{id}/files` with `HX-Request` and the
  Acme header. 409, no `documents` row, nothing in storage.
- `a_document_upload_with_the_right_org_goes_through`.
- `a_multipart_post_with_no_header_fails_closed`.
- `acting_checks_against_the_acting_org`: a super admin acting in Beta. A
  claim for their own org is refused; a claim for Beta goes through.
- `the_chat_socket_upgrade_is_not_checked`: a non-HTMX GET to
  `/chat/{id}/ws` for a Beta conversation, with an Acme header, gets past
  the check (a 400 for "not a WebSocket upgrade").
- `the_allow_page_keeps_its_own_pin`: `POST /oauth/authorize` with a wrong
  header and the right `org_id` issues a code. Deny with no `org_id` still
  sends `access_denied`. The PR 2 OAuth tests pass unchanged.
- `the_token_form_keeps_its_own_pin`: a wrong header and the right `org_id`
  creates a token. A wrong `org_id` re-renders with the typed name and 409.

**The notice on Home:**

- `home_names_the_org_the_tab_was_showing`: `/dashboard?from=<acme>` names
  Acme and Beta, has a Back form posting to `/orgs/<acme>/switch`, says
  nothing was saved, and is `no-store`.
- `home_does_not_name_an_org_you_are_not_in`: a stranger's org gives
  "another organisation", no Back, and the name appears nowhere on the page.
- `home_does_not_name_an_org_you_are_pending_in`.
- `home_does_not_name_a_deleted_org`.
- `home_offers_no_back_after_acting_outside_your_orgs`: a super admin
  refused from a tab drawn while acting in an org they don't belong to.
- `home_with_an_empty_or_garbage_from_shows_the_nameless_notice`.
- `home_from_the_current_org_shows_no_notice`.

`tests/requests/orgs.rs` (new):

- `app_pages_carry_their_org_on_body`: covers every app-layout page with a
  POST form: `/notifications`, `/more`, `/settings/claude`, `/members`,
  `/tasks`, a documents folder, a chat conversation, `/roadmap` and
  `/meetings`. The body has `data-org-id` and `hx-headers` with Beta's id,
  outside `body_attrs`. A documents folder page also keeps
  `hx-history="false"`.
- `acting_pages_carry_the_acting_org`.
- `auth_pages_carry_no_org`: `/login`, `/signup`, the waiting page.
- `one_org_shows_a_plain_name`: no menu button, no More row.
- `two_orgs_show_the_switcher`.
- `one_org_and_a_pending_request_shows_the_switcher`.
- `an_acting_admin_with_no_membership_sees_a_plain_name`.
- `htmx_requests_skip_the_switcher_count`: `org_menu` is false on an HTMX
  partial, checked by counting queries if the harness allows it, otherwise
  through the rendered context.
- `the_org_list_is_active_a_to_z_then_pending`: three active out of order,
  one pending.
- `the_org_list_hides_removed_and_declined`.
- `the_org_list_never_shows_other_peoples_orgs`.
- `the_org_list_over_htmx_is_a_fragment`: no `<html>`.
- `the_org_list_is_no_store_and_varies_on_hx_request`.
- `the_org_list_while_acting_has_acting_in_and_your_organisations`.
- `the_probe_is_204_and_refused_when_stale`: `GET /orgs/current`, both
  `no-store`.
- `switching_lands_on_home_with_the_new_org_named`: `Location:
  /dashboard?switched=1`, and that page's toast names Beta.
- `switching_over_htmx_sends_hx_redirect_to_home`.
- `switching_to_an_org_you_left_is_a_404`: for the menu's toast.
- `the_switched_flag_alone_changes_nothing`: `GET /dashboard?switched=1`
  sets no cookie and names the current org.
- `the_switch_ignores_a_next_parameter`.
- `there_is_no_get_that_switches`: `GET /orgs/{id}/switch` is 405.

`tests/models/memberships.rs`:

- `switcher_counts_counts_active_and_pending_only`.
- `for_user_with_orgs_sorts_active_by_name_then_pending`.

**Updated:** the PR 2 multi-org tests named under "What changes", and every
tenant POST in the suite, through `.as_member(...)`.

**Not covered by `cargo test`.** The repo has no JS test runner. These are
checked by hand (see "Checks"): the submit listener, `orgHeaders`,
`staleRefusal`, the guard and banner, the probes and the menu keyboard.

## Out of scope (PRs 4-7, and deferred)

- **PR 4, create another org.** The first second membership. The menu gets
  "New organisation" then, and may need to show for one org too (mockup
  section 7).
- **PR 5, org-scoped removal.** Events keyed by org and user. Closing only one
  device's sockets on a switch.
- **PR 6, join with an existing account.**
- **PR 7, links and the Claude connector.** Org-aware links in emails and
  notifications.
- **Deferred:**
  - unread dots per org in the switcher;
  - a cross-check of the `X-Org-Id` inside websocket messages against the
    socket's org;
  - `timeout_request` for slow request bodies;
  - a reload-loop guard (no loop is expected; see "Edge cases");
  - `Sec-Fetch-Site` on the switch, and the `__Host-` cookie prefix (PR 2's
    "Later");
  - reopening the notification stream on a bfcache restore (`pagehide`
    closes it and nothing reopens it today);
  - putting a draft back into the form by itself after switching back
    (mockup section 4).

## Risks

- **Test churn.** About 200 request-test POSTs change one line each to
  `.as_member(...)`. Review that those edits change nothing else.
- **A forgotten client path.** A new `fetch` without the header fails closed
  and sends the tab Home. That is a visible bug, not a leak. The hand checks
  list every current path.
- **htmx internals.** The plan relies on three htmx behaviours.
  `HX-Redirect` is handled before the status rule. `htmx:beforeOnLoad` can be
  cancelled before that. `getHeaders` reads inherited `hx-headers`. All are
  checked in the vendored 2.0.11. An htmx upgrade must check them again.
- **Cookie on a manual redirect.** "Back to Acme" from the banner relies on
  the browser storing `Set-Cookie` from a `redirect: "manual"` response.
  Hand check in Chrome, Safari and Firefox.
- **Uploads refused mid-body** may show as a dropped connection. The probe
  before each upload makes it rare. Hand check locally and deployed.
- **Body buffering.** The layer holds up to 2 MB of a urlencoded body in
  memory for signed-in requests, which `Form` already does.
- **The per-page count** adds one indexed query to every full page load.
- **Tabs open across the deploy** are refused once. Harmless before PR 4.
- **A stale tab can show Acme for up to 60 seconds** if nothing triggers a
  probe. Reads only, and the person is still an Acme member.
- **The guard is a heuristic.** It can miss a changed select or checkbox.
  Those are quick to redo.

## Checks

```sh
bin/build-assets
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

By hand, locally, with Alice in two orgs. Insert the second membership by
SQL; there is no create-org UI until PR 4. Open two tabs on Acme, switch to
Beta in one, then do each of these in the other:

- create a task (HTMX dialog), drag a card, add a note, react to a note;
- send a chat message, react to a message, open a message's actions;
- upload a document, rename and delete a file;
- show and copy a login password, unlock with the app password;
- mark all notifications read, open a notification (plain forms);
- create a Claude token (its own message), Allow and Deny on the consent page;
- Getting started show and dismiss;
- wait for the badge poll; leave a chat open and watch the socket close;
- with text typed in the task dialog, wait for the poll: one banner, text
  kept; "Back to Acme", then Save works and the task is in Acme;
- with an upload queued, switch in the other tab: the next upload waits
  behind the banner;
- a ~20 MB upload refused mid-way, locally and on the deployed app;
- back button into a bfcache-restored Acme page;
- the menu by keyboard (mockup section 1), the phone sheet, "Switching…",
  the 404 toast (remove the membership by SQL with the menu open), and no
  JS (the `/orgs` page).

Each refusal lands on Home with the notice (or the banner), and nothing
appears in either org. Then repeat in a fresh tab: everything works.

No migration, so nothing to roll back in the database. Reverting the code is
safe.

## Screenshots needed

Take them from the local app with realistic data: the mockup's people and
orgs (Acme Studio, Northwind Bakery, a pending Himalayan Ritual), with
members and tasks. Store them on the `pr-screenshots` branch under
`multi-org-3/`. Desktop 1440px and phone 390px, light and dark:

- the sidebar with the menu closed and open (current, another active org,
  one pending);
- the phone Home header with the sheet open, and More's row;
- "Switching…" on a row;
- Home after a switch, with the "You're now in" toast;
- Home after a stale refusal, with the notice and "Back to Acme Studio";
- the banner over the task dialog, with the text kept;
- the menu while acting;
- one org: the plain name, as today.
