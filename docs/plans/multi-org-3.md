# Multi-org 3: the stale-tab org check, and the switcher

Status: Draft · PR 3 of 7 · Owner: Raj · 19 October 2026

## Summary

PR 2 (#43) added the `current_org` cookie and `POST /orgs/{id}/switch`, with
no button. This PR does two things.

**A. The stale-tab org check.** This is the hard gate before PR 4. Today a tab
left open on Acme posts into Beta after a switch in another tab. After this
PR:

- Every app page says which org it was drawn for: `data-org-id` and
  `hx-headers='{"X-Org-Id": "<id>"}'` on `<body>`.
- HTMX requests carry that header. Plain forms get a hidden `_org` field from
  a submit listener in `frontend/app.ts`. The few raw `fetch` calls add the
  header by hand.
- A small layer in `src/initializers/current_org.rs` lifts `_org` out of
  urlencoded bodies into a request extension. `CurrentMember` compares the
  claim with the org it resolved.
- A mismatch is refused with 409. HTMX gets `HX-Refresh: true`, so the tab
  reloads. A plain form gets a "this page is out of date" page.
- A missing value on a POST is refused the same way (fails closed). A
  missing value on a GET is allowed.

**B. The switcher.** The org name at the top of the sidebar opens a menu of
your organisations. It is loaded only when opened, from a new `GET /orgs`.
Picking one posts to the existing switch endpoint, which now lands on
`/dashboard?switched=1` with a "you're now in Beta" toast. The look and
wording come from the mockup at `docs/designs/org-switcher.html` (being drawn
in parallel; section names below are to be confirmed when the designer
reports).

No migration. Nobody has two orgs yet, so in production the switcher shows
one organisation until PR 4.

## Where the check lives

**Decision: the check is in `CurrentMember`. A thin layer moves `_org` from
form bodies into a request extension first.**

`CurrentMember` is the only place that knows the resolved org (acting, then
the cookie, then the default). Every tenant route already runs it, including
HTMX GETs. So the comparison goes there. The one thing an extractor can't do
is read the body without consuming it. The layer does that part, and only for
urlencoded POSTs that have no header.

### The layer (`src/initializers/current_org.rs`)

The existing `current-org` layer already puts `StaleCurrentOrg` in every
request. It also inserts `OrgClaim(Option<String>)`:

1. If the request has an `X-Org-Id` header, that value. The body is not
   touched.
2. Else, if the method is not GET, HEAD or OPTIONS and the content type is
   `application/x-www-form-urlencoded`: buffer the body with
   `axum::body::to_bytes` up to 2 MB (axum's default `Form` limit), read the
   first `_org` with `url::form_urlencoded::parse`, and put the same bytes
   back as the body. Over 2 MB is a 413, which `Form` would give anyway.
3. Else `None`. Multipart and JSON bodies are never buffered.

No new crate: `url` is already a dependency.

### The check (`src/extractors/current_member.rs`)

After the org is resolved, before `Ok(member)`:

| Request | Claim | Result |
|---|---|---|
| GET or HEAD, not HTMX | anything | No check. Page loads, links from emails, bookmarks, file downloads, the chat socket upgrade and the notification stream all pass. |
| GET or HEAD, HTMX (`HX-Request: true`) | missing | Allowed. Tabs drawn before the deploy keep polling. |
| GET or HEAD, HTMX | equals the resolved org | Allowed. |
| GET or HEAD, HTMX | anything else | 409 with `HX-Refresh: true`. |
| Any other method | equals the resolved org | Allowed. |
| Any other method | missing, not a number, or another org | 409. HTMX gets `HX-Refresh: true`. Otherwise the stale page. |

- "The resolved org" is the acting org while a super admin is acting. So a
  tab drawn for the admin's own org is stale while they are inside another.
- The waiting, removed and declined pages and the `/admin` redirect return
  before the check. They have no org to compare.
- One info log line on each refusal: user id, the claim (only if it parses as
  an `i64`), the resolved org, and whether it was HTMX. Numbers only, as in
  the switch log.

**The HTMX response.** `409`, `HX-Refresh: true`, body "This page is out of
date." htmx 2.0.11 handles `HX-Refresh` before it looks at the status, so the
`[45]..` "don't swap" rule in `base.html` does not stop the reload (checked
in `assets/static/vendor/htmx.min.js`, function `Vn`).

**The plain-form response.** A 409 page, `tenancy/stale_org.html`, in the
auth layout like the waiting page. It shows:

- the org you are in now (the resolved org, from the database);
- the org the page was for, only if you are an active member there
  (`active_in(claim, user)`, one query, only on a refusal). Otherwise it
  says "another organisation", so a forged value reveals nothing;
- a link to `/dashboard`, and, when the claimed org is named, a "Switch to
  Acme" form posting to `/orgs/{claim}/switch`;
- that nothing was saved.

It never uses `Referer` or a `next` parameter, so there is no open redirect.
Wording and layout: mockup section "stale-tab refresh".

Why a page and not a redirect with a flash: the person must know the action
did not happen. A toast on Home is easy to miss and looks like success.

### `PinnedMember`: two forms keep their own check

The Claude Allow page (`POST /oauth/authorize`) and the personal token form
(`POST /settings/claude/tokens`) already pin their org with a hidden
`org_id` (PR 2). Their checks are the same rule, with better outcomes:

- Deny on the Allow page still goes back to Claude with `access_denied`.
- The token form keeps the typed name and shows its own message.

The generic check would answer first and lose both. So these two handlers
take a new `PinnedMember(CurrentMember)` extractor. It resolves the org
exactly like `CurrentMember` but skips the claim check. Its doc comment says
it is only for forms that call `shown_for_this_org` themselves. The route
walk test (below) lists both, so a third use needs a deliberate change.

### Alternatives considered

1. **Header only.** Make every request carry `X-Org-Id` by turning plain
   forms into `fetch` calls, or by `hx-boost`ing them.
   - Converting forms means re-implementing redirects, error pages and
     navigation in `app.ts` for about 15 plain forms.
   - `hx-boost` swaps the response into `<body>`. The `[45]..` rule means a
     403 or 409 page would not show. Boosted forms also change history.
   - Both still fail without JS, so the server needs the fail-closed rule
     anyway.
   - Rejected: more moving parts in the browser than a 30-line server layer.
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
  include `X-Org-Id`. The server keeps ignoring them.

What PR 2 left open: a tab whose reconnect is refused stops getting messages
but never reloads. `app.ts` now sends one HTMX probe on `htmx:wsClose` (see
"`GET /orgs/current`"). A stale tab gets 409 and reloads.

**Notification stream: no check needed.** It carries no data, only "something
changed" for this user. The badge it triggers is an HTMX GET, which is
checked. On a switch the stream is closed and the browser reconnects. `app.ts`
now triggers `notifications-changed` when the stream reopens, so a stale tab
refreshes within seconds rather than at the next 60-second poll.

## The switcher

### Data

New `memberships::Model::for_user_with_orgs(db, user_id) ->
Vec<(Model, organisations::Model)>`: one query, memberships joined to
organisations with `find_also_related`, oldest first like `for_user`.

**Cost: none on page loads.** The menu is loaded when opened
(`hx-get="/orgs" hx-trigger="click once"` or the mockup's equivalent). The
query uses `idx-memberships-user` and the organisations primary key, and
returns a handful of rows.

Loading it in the layout instead would mean a query on every request.
`page()` is synchronous, so it would have to run inside `CurrentMember`, for
HTMX partials and downloads too. Only worth it if the mockup needs the list
on every page, for example to hide the menu for one-org users or to show
unread dots for other orgs. Unread dots are out of scope.

### `GET /orgs` (in `src/controllers/orgs.rs`)

- `CurrentMember`, so a stale tab opening the menu refreshes first.
- HTMX: the list fragment `orgs/_list.html`. Otherwise the full page
  `orgs/index.html` in the app layout. The full page is what phones reach
  from More, and it works without JS.
- Each row: org name, your role, your status.
  - The current org is marked (`aria-current`). No button.
  - Other active orgs: a form posting to `/orgs/{id}/switch`.
  - Pending: "Waiting for approval", no button.
  - Removed and declined: hidden (open question 3).
- While acting, no row is current. The banner already says where you are.
  Switching ends acting, as in PR 2.

### `GET /orgs/current`

Returns 204 through `CurrentMember`. It is the probe `app.ts` sends after the
chat socket closes, when the notification stream reopens, and on a bfcache
restore. A stale tab gets 409 and `HX-Refresh`. Cheaper and clearer than
borrowing the feed or a badge URL.

### After a switch

- `switch` redirects to `/dashboard?switched=1` instead of `/dashboard`
  (`HX-Redirect` for HTMX). The target is a constant.
- `dashboard::index` reads `switched` and adds a flash built from
  `member.org.name`, the resolved org. Same pattern as `/members?removed=`
  and `/more?signed_out=1`: the URL only says "show it", the name comes from
  the database.
- So `/dashboard?switched=1` typed by hand shows "You're now in" your current
  org and changes nothing. There is still no GET that switches.
- Wording: mockup section "after a switch".

## What changes, file by file

| File | Change |
|---|---|
| `src/initializers/current_org.rs` | Add `OrgClaim` and the body lifting (above). Module comment covers both jobs. |
| `src/extractors/current_member.rs` | Constants `ORG_HEADER = "x-org-id"` and `ORG_FIELD = "_org"`. Move today's body of `from_request_parts` into `async fn resolve(parts, ctx)`. `CurrentMember` = `resolve` + `check_claim`. New `PinnedMember` = `resolve` only. `page()` adds `"current_org_id": self.org.id`. The 409 responses. |
| `src/controllers/oauth.rs` | `authorize` (POST) takes `PinnedMember`. The GET page is unchanged. |
| `src/controllers/claude.rs` | `create_token` takes `PinnedMember`. |
| `src/controllers/orgs.rs` | `GET /orgs`, `GET /orgs/current`. `switch` redirects to `/dashboard?switched=1`. |
| `src/controllers/dashboard.rs` | `Query` with `switched: Option<String>`, and the flash. |
| `src/models/memberships.rs` | `for_user_with_orgs`. |
| `assets/views/layouts/base.html` | On `<body>`, only when `current_org_id` is set: `data-org-id` and `hx-headers`. A separate key, not `org.id`, because the join and waiting pages pass their own `org`. The id is an integer, so the JSON needs no escaping. |
| `assets/views/layouts/app.html` | The sidebar's org link (line 7) becomes the switcher trigger and menu. Mobile entry per the mockup. |
| `assets/views/orgs/index.html`, `assets/views/orgs/_list.html` (new) | The list. |
| `assets/views/tenancy/stale_org.html` (new) | The 409 page. |
| `assets/views/more/index.html` | An "Organisation" row linking to `/orgs`, if the mockup puts it there. |
| `frontend/app.ts` | See below. |
| `frontend/styles/app.css` | Switcher styles from the mockup. |
| `tests/requests/prepare_data.rs` | `in_org(org_id)` helper on `TestRequest` that adds `X-Org-Id`, and `org_id(ctx, slug)`. |
| `tests/requests/stale_tab.rs` (new), `tests/requests/orgs.rs` (new), `tests/requests/mod.rs` | New tests. |
| Every request test that POSTs to a tenant route | Add `.in_org(...)`. About 200 call sites, mechanical. Nothing else in those tests changes. |
| `tests/requests/multi_org.rs` | `with_a_beta_cookie_acme_tasks_404_and_new_tasks_go_to_beta` now expects 409 and no task. Switch tests expect `/dashboard?switched=1`. |
| `tests/models/memberships.rs` | `for_user_with_orgs` test. |
| `docs/flows/accounts.md` | Replace "Until the stale-tab check ships" with a "Stale tabs" section. The switch button exists now. |
| `docs/flows/super-admin.md` | The check compares against the acting org. |
| `docs/flows/chat.md` | Why the socket needs no check; the probe on close. |
| `docs/flows/claude-connector.md` | The Allow page keeps its own pin and skips the generic check. |

### `frontend/app.ts`

1. **Submit listener**, capture phase, on `document`. For a form with
   `method="post"` and no `hx-post`, `ws-send` or `data-unlock-form`: set a
   hidden `_org` input to `document.body.dataset.orgId`, creating it if
   needed. Multipart forms too. Only when the body has an org id. The upload
   form is HTMX, so its header carries the org. The listener's `_org` only
   matters if it is ever made plain.
2. **Raw `fetch` calls** add the header through one helper, `orgHeaders()`:
   the chat message actions GET (line 107), the reaction POST (line 248)
   and `postForm` (line 1353: reveal, copy, unlock).
3. **409 from a raw fetch** with `HX-Refresh: true`: the same handling as
   HTMX (item 4).
4. **Typed text guard.** On `htmx:beforeOnLoad` with status 409 and
   `HX-Refresh`: if any text field in a form differs from its
   `defaultValue` (ignoring hidden fields and `[data-filter]` search boxes),
   cancel the event and show the stale banner from the mockup, with a Reload
   button. Otherwise let htmx reload. This stops a 60-second badge poll from
   wiping a half-written task. Cancelling `htmx:beforeOnLoad` returns before
   `HX-Refresh` is handled (checked in htmx 2.0.11, `Vn`).
5. **Probes** with `htmx.ajax("GET", "/orgs/current", { swap: "none" })`:
   on `htmx:wsClose`, when the notification `EventSource` fires `open` after
   its first, and on `pageshow` with `persisted`.
6. **Switcher open and close**, if the mockup needs more than a `<dialog>`
   or `<details>`.

`htmx.ajax` calls (task drag, guide welcomed, feed reload, login sheet) use
`<body>` as their source, so they inherit `hx-headers` with no change. htmx's
`getHeaders` reads `hx-headers` up the tree (checked in 2.0.11, `yn`).

## Edge cases

- **Two tabs.** Tab 1 shows Acme. Tab 2 switches to Beta.
  - Tab 1's next HTMX request, its badge poll (60 s), its notification stream
    reopening, or its chat socket closing returns 409. It reloads as Beta,
    unless text is typed (banner instead).
  - A plain form from tab 1 gets the stale page. Nothing is saved.
  - A link in tab 1 is a page load, unchecked. Id links (`/tasks/5`) 404 in
    Beta, as today. Until PR 7.
  - Reads in tab 1 before it refreshes show Acme data. The person is still an
    Acme member, so this is not a leak.
- **Reload lands on a 404.** A refresh of `/tasks/5` (an Acme task) in Beta
  gives the not-found page. Accepted until PR 7 (org-aware links). Open
  question 6 offers sending the tab to Home instead.
- **Back button and bfcache.** A restored page still has Acme's id. The
  `pageshow` probe catches it straight away. Without bfcache the page is
  fetched again and is simply Beta.
- **A form being typed in.** HTMX submit or a poll: the guard shows the
  banner and keeps the text. A plain form: the stale page; the browser's
  back button usually restores the fields. Copying the text out is left to
  the person.
- **Acting.** The admin enters org X in tab 2. Tab 1 (their own org A) is
  stale. "Switch to A" on the stale page ends acting (the switch clears
  `acting_org`). The banner's Leave form is exempt.
- **The org was deleted.** The claim names a deleted org. Refused. The stale
  page doesn't name it and offers no switch back.
- **Pending, removed or declined in the claimed org.** Refused. Not named, no
  switch back. Removal still signs you out everywhere until PR 5.
- **No active membership left.** The waiting or removed page shows before the
  check, as today. Its only form is Sign out, which is exempt.
- **Tabs open across the deploy.** Old pages have no org id. Their HTMX POSTs
  get 409 and reload once. Their plain forms get the stale page once. Their
  polls keep working. Nobody has two orgs yet, so this is the only cost.
- **Forged claim.** Same as a stale one. It can only make a request fail.
  The claim never chooses the org; the cookie and membership still do.
- **No JS.** Plain forms carry nothing and fail closed with the stale page.
  The app already needs JS for most screens. Accepted.
- **Two claims.** Header and `_org` both present: the header wins and the
  body is not read.

## Tests

`tests/requests/stale_tab.rs` (new). Alice is in Acme and Beta
(`alice_in_two_orgs`), with `current_org` = Beta unless said otherwise.

- `every_post_route_refuses_a_header_from_another_org`: walks
  `App::routes(&ctx).collect()`. Every POST route not in `EXEMPT` gets
  `X-Org-Id: <acme>` and an empty form. Expects 409. Path parameters are
  filled with `1`; the check runs before the handler reads them.
- `every_post_route_refuses_a_form_from_another_org`: the same walk with
  `_org=<acme>` in a urlencoded body and no header.
- `every_post_route_refuses_a_request_with_no_org`: the same walk with
  neither.
- `every_htmx_get_from_another_org_refreshes_the_tab`: walks GET routes not
  in `EXEMPT_GET`, with `HX-Request` and the Acme claim. Expects 409 and
  `HX-Refresh: true`. `EXEMPT_GET` is the routes without `CurrentMember`:
  `/`, `/login`, `/forgot`, `/reset/{token}`, `/signup`, `/join/{slug}`,
  `/admin`, `/_health`, `/_ping`, `/.well-known/*`, and `/oauth/authorize`
  (it checks its query before it looks at the member).
- `the_exempt_lists_match_real_routes`: every entry in `EXEMPT` and
  `EXEMPT_GET` exists in the route list, so the lists can't go stale.
- `exempt_routes_ignore_the_claim`: each exempt POST with a wrong
  `X-Org-Id` is not a 409 (login, signup, join, forgot, reset, logout, sign
  out everywhere, switch, admin enter and leave, oauth register and token,
  mcp).
- `the_right_org_goes_through_by_header_and_by_field`: `POST /tasks` with
  the header (HTMX) and with `_org` (plain) both create the task in Beta.
- `an_acme_tab_cannot_post_a_task_into_beta`: 409 and no task in either org.
- `a_plain_form_from_another_org_gets_the_stale_page`: `POST
  /notifications/read`, 409 HTML naming Beta and Acme, a switch form for
  Acme, a link to `/dashboard`, no `HX-Refresh`.
- `the_stale_page_does_not_name_an_org_you_are_not_in`: claim a stranger's
  org. Neither its name nor a switch form appears.
- `the_stale_page_does_not_name_an_org_you_are_pending_in`.
- `a_claim_for_a_deleted_org_is_refused_and_not_named`.
- `a_garbage_claim_is_refused`: `X-Org-Id: abc` and `_org=abc`.
- `polling_badges_refresh_a_stale_tab`: `/notifications/unread` and
  `/chat/unread` with `HX-Request` and the Acme claim.
- `an_htmx_get_without_a_claim_still_works`.
- `a_page_load_ignores_the_claim`: `GET /tasks`, no `HX-Request`, Acme
  header. 200, showing Beta.
- `a_document_upload_from_another_org_is_refused_and_nothing_is_stored`:
  multipart `POST /documents/folders/{id}/files` with `HX-Request` and the
  Acme header. 409, no `documents` row, nothing in storage.
- `a_document_upload_with_the_right_org_goes_through`.
- `a_multipart_post_with_no_header_fails_closed`.
- `a_large_urlencoded_body_is_still_read`: a task with a long description
  (about 100 KB) and `_org` last in the body is created.
- `acting_checks_against_the_acting_org`: a super admin acting in Beta. A
  claim for their own org is refused; a claim for Beta goes through.
- `the_chat_socket_upgrade_is_not_checked`: a non-HTMX GET to
  `/chat/{id}/ws` for a Beta conversation, with an Acme header, gets past
  the check (a 400 for "not a WebSocket upgrade", not 409).
- `the_allow_page_keeps_its_own_pin`: `POST /oauth/authorize` with a wrong
  header and the right `org_id` issues a code. Deny with no `org_id` still
  sends `access_denied`. The PR 2 OAuth tests pass unchanged.
- `the_token_form_keeps_its_own_pin`: a wrong header and the right `org_id`
  creates a token. A wrong `org_id` re-renders with the typed name and 409.

`tests/requests/orgs.rs` (new):

- `app_pages_carry_their_org_on_body`: `/dashboard` has `data-org-id` and
  `hx-headers` with Beta's id.
- `acting_pages_carry_the_acting_org`.
- `auth_pages_carry_no_org`: `/login`, `/signup`, the waiting page.
- `the_org_list_shows_each_membership_with_role_and_status`: Acme owner,
  Beta member (current), Gamma pending without a button.
- `the_org_list_hides_removed_and_declined`.
- `the_org_list_never_shows_other_peoples_orgs`.
- `the_org_list_over_htmx_is_a_fragment`: no `<html>`.
- `the_org_list_marks_none_current_while_acting`.
- `the_probe_is_204_and_409_when_stale`: `GET /orgs/current`.
- `switching_lands_on_home_with_the_new_org_named`: `Location:
  /dashboard?switched=1`, and that page's flash names Beta.
- `the_switched_flag_alone_changes_nothing`: `GET /dashboard?switched=1`
  sets no cookie and names the current org.
- `the_switch_ignores_a_next_parameter`: `POST
  /orgs/{id}/switch?next=https://example.com` still goes to
  `/dashboard?switched=1`.
- `there_is_no_get_that_switches`: `GET /orgs/{id}/switch` is 405.

`tests/models/memberships.rs`:

- `for_user_with_orgs_returns_each_membership_with_its_org_oldest_first`.

Updated: the PR 2 multi-org tests named under "What changes", and every
tenant POST in the suite gains `.in_org(...)`.

**Not covered by `cargo test`.** The repo has no JS test runner. The submit
listener, the raw fetch headers, the typed-text guard and the probes are
checked by hand (see "Checks").

## Out of scope (PRs 4-7)

- **PR 4, create another org.** The first second membership. The menu gets
  "Create organisation" then.
- **PR 5, org-scoped removal.** Events keyed by org and user. Closing only one
  device's sockets on a switch.
- **PR 6, join with an existing account.**
- **PR 7, links and the Claude connector.** Org-aware links in emails and
  notifications, which also fixes "reload lands on a 404".
- Unread dots for other orgs in the switcher.
- Reopening the notification stream on a bfcache restore. Today `pagehide`
  closes it and nothing reopens it. The `pageshow` probe covers the org
  check; badges still poll.
- `Sec-Fetch-Site` on the switch and the `__Host-` cookie prefix (PR 2's
  "Later").

## Risks

- **Test churn.** About 200 request-test POSTs gain `.in_org(...)`. The diff
  is big but mechanical. Review that those edits add only the header.
- **A forgotten client path.** A new `fetch` without the header fails closed
  with a 409. That is a visible bug, not a leak. The hand checks list every
  current path.
- **htmx internals.** The plan relies on `HX-Refresh` being handled before
  the status rule, on `htmx:beforeOnLoad` being cancellable before that, and
  on `getHeaders` reading inherited `hx-headers`. All checked in the
  vendored 2.0.11. An htmx upgrade must check them again.
- **Body buffering.** The layer holds up to 2 MB of a urlencoded body in
  memory, which `Form` already does. Multipart is never buffered.
- **Tabs open across the deploy** get one 409. Harmless before PR 4.
- **A stale tab can show Acme for up to 60 seconds** if nothing triggers a
  probe. Reads only, and the person is still an Acme member.
- **The typed-text guard is a heuristic.** It can miss a changed select or
  checkbox. Those are quick to redo.

## Open questions for Raj

Each with a recommendation.

1. **The Allow page and token form skip the generic check** and keep their
   own pin (`PinnedMember`). Recommend yes: same safety, and Deny and the
   typed name keep working.
2. **Typed-text guard.** On a stale 409 with typed text, show a banner
   instead of reloading. A small step beyond the approved "409 plus
   `HX-Refresh`". Recommend yes.
3. **Removed and declined orgs in the switcher.** Recommend hidden. Pending
   shows as "Waiting for approval".
4. **Ship the switcher before PR 4,** when everyone sees one org. Recommend
   yes. It is the org name already at the top of the sidebar, and it is
   ready for PR 4.
5. **Strict fail closed** costs about 200 test edits. The alternative is to
   fail closed only when the request has an `Origin` header (browsers always
   send it on POST; unverified for every browser version). Recommend strict,
   as approved.
6. **Where a stale tab lands.** Approved: reload in place (`HX-Refresh`),
   which can land on a 404 for id pages until PR 7. Alternative: send it to
   Home with a "this tab was showing Acme" toast. Recommend keeping the
   approved reload unless the mockup prefers Home.

## Checks

```sh
bin/build-assets
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

By hand, locally, with Alice in two orgs (second membership inserted by SQL;
there is no create-org UI until PR 4). Two tabs on Acme, switch to Beta in
one, then in the other:

- create a task (HTMX dialog), drag a card, add a note, react to a note;
- send a chat message, react to a message, open a message's actions;
- upload a document, rename and delete a file;
- show and copy a login password, unlock with the app password;
- mark all notifications read, open a notification (plain forms);
- create a Claude token (its own message), Allow and Deny on the consent page;
- Getting started show and dismiss;
- wait for the badge poll; leave a chat open and watch the socket close;
- with text typed in the task dialog, wait for the poll: banner, text kept;
- back button into a bfcache-restored Acme page.

Each is refused with a refresh or the stale page, and nothing appears in
either org. Then the same in a fresh tab: everything works.

No migration, so nothing to roll back in the database. Reverting the code is
safe.

## Screenshots needed

From the local app with realistic data: two or three organisations with
real-sounding names, a few members and tasks. Stored on the
`pr-screenshots` branch under `multi-org-3/`. Desktop 1440px and phone 390px,
light and dark:

- the sidebar with the switcher closed and open (current, another active
  org, one pending);
- `/orgs` on phone, and its entry point (More or wherever the mockup puts
  it);
- Home after a switch, with the "You're now in" toast;
- the stale page after a plain form, with "Switch to Acme";
- the typed-text banner over the task dialog.
