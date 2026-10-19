# Multi-org 2: one membership per organisation, and the current_org cookie

Status: Approved by Raj 19 October 2026, all five decisions as recommended (staff: approve with changes, applied; security: approve with changes, applied) · PR 2 of 7 · Owner: Raj · 19 October 2026

## Summary

PR 1 (#41) stopped the code assuming one membership per user. This PR lets
the database hold more than one, and lets a browser choose which organisation
it is working in.

- A migration replaces the unique index on `memberships.user_id` with a
  unique index on `(organisation_id, user_id)` and a plain index on
  `user_id`.
- A second migration ties the logins unlock to one organisation.
- A new `current_org` cookie says which organisation this browser is in. It
  is only a hint. Every request checks it again.
- A new endpoint, `POST /orgs/{id}/switch`, is the only thing that sets the
  cookie. It has no button yet. The switcher UI is PR 3.
- The Claude "Allow access" form and the personal token form pin the
  organisation they were shown for.

Nothing changes for anyone today. Nobody has two memberships yet, so a switch
can only pick the organisation you are already in.

## What sets the cookie in PR 2

**Decision: add `POST /orgs/{id}/switch` now, with no UI.**

Why:

- It makes the PR testable end to end. Without it, the cookie-reading code
  has no real writer, and tests would only ever forge the cookie.
- Everything a switch must do (check access, clear `acting_org`, close live
  connections, end an unlock) lands in one place and gets one security
  review, together with the cookie rules.
- PR 3 becomes the switcher UI plus the stale-tab check, on an endpoint that
  already exists and is tested.
- The endpoint can't grant anything. It only accepts an organisation where
  you already have an active membership.

The cost: the token pins (OAuth and personal) must ship in this PR, since a
switch now exists. Each is one hidden field and one check.

The other option, nothing until PR 3, keeps PR 2 to the migration and a
reader nobody can trigger. It is smaller but proves less.

## What changes, file by file

| File | Change |
|---|---|
| `migration/src/m20261019_xxxxxx_memberships_one_per_org.rs` (new, `cargo loco generate migration MembershipsOnePerOrg`) | The index swap. See "The migrations". |
| `migration/src/m20261019_xxxxxx_add_secrets_unlocked_org_id_to_users.rs` (new, `cargo loco generate migration AddSecretsUnlockedOrgIdToUsers`) | Nullable `users.secrets_unlocked_org_id`. Its own migration so each rolls back alone. |
| `migration/src/lib.rs` | Both registered by the generator. |
| `src/models/_entities/memberships.rs`, `organisations.rs`, `users.rs` | Regenerated with `cargo loco db entities`. Expected: `#[sea_orm(unique)]` goes from `memberships.user_id`, and `organisations` changes `has_one = memberships` to `has_many`. Keep these changes. `users` gains the new column. |
| `src/extractors/current_member.rs` | Add `CURRENT_ORG_COOKIE = "current_org"`. Read it after the acting check. See "Cookie rules". |
| `src/extractors/session.rs` | Add `current_org_cookie(ctx, org_id)` and `cleared_current_org_cookie(ctx)`, built with `base_cookie` like `acting_org`, plus `Max-Age` equal to the JWT expiration (7 days). Add `signed_in_cookies(ctx, user)`: the session cookie plus cleared `current_org` and `acting_org`. |
| `src/initializers/current_org.rs` (new) | A small layer, added in `after_routes` like `origin_check`. It puts a `StaleCurrentOrg` flag (an `Arc<AtomicBool>`) in the request extensions. If `CurrentMember` sets it, the layer adds a cleared `current_org` cookie to the response. An extractor cannot set cookies itself. |
| `src/app.rs` | Register the new initializer. |
| `src/controllers/orgs.rs` (new, `cargo loco generate controller orgs`) | `POST /orgs/{id}/switch`. See below. PR 4 ("create another org") will live here too. |
| `src/controllers/auth.rs` | `login` uses `signed_in_cookies`. `logout` also clears `current_org`. `sign_out_everywhere` is unchanged (see "Cookie rules"). |
| `src/controllers/signup.rs`, `src/controllers/join.rs` | Use `signed_in_cookies`, so a cookie left by someone else on a shared browser is cleared. |
| `src/models/users.rs` | `unlock_secrets` (line 433) takes `org_id` and sets `secrets_unlocked_org_id`. `secrets_unlocked(sid, org_id)` also requires the org to match. `increment_session_version` clears the new column. New `end_unlock(db, user_id)` clears all three unlock columns without ending sessions. |
| `src/controllers/logins.rs` | Pass `member.org.id` to `secrets_unlocked` at lines 139 and 478, and to `unlock_secrets` at line 441. |
| `src/controllers/oauth.rs` | `AuthorizeParams` gets `org_id: Option<String>`, parsed by hand (see "Token pins"). The GET page adds it to `fields`. |
| `assets/views/oauth/authorize.html` | No change. It already writes every entry in `fields` as a hidden input. |
| `src/controllers/claude.rs`, `assets/views/claude/index.html` | The token form (line 50) gets a hidden `org_id`. `create_token` refuses on a mismatch. |
| `src/data/chat_hub.rs`, `src/data/notify_hub.rs` | Doc comments only: `SignedOut` now also fires on a switch. No rename, to keep the diff small. |
| `tests/requests/multi_org.rs` | Remove the `DROP INDEX` from `alice_in_two_orgs` and the module comment about it. Add the cookie tests. |
| `tests/models/memberships.rs`, `tests/models/users.rs`, `tests/requests/oauth.rs`, `tests/requests/claude.rs`, `tests/requests/admin.rs`, `tests/requests/live_sign_out.rs` | New tests, listed below. |
| `docs/architecture/erd.md` | New indexes and the new column. Remove "unique today" and the one-membership rule. |
| `docs/flows/accounts.md` | Sign in, sign out and sign out everywhere: what happens to `current_org`. A short "Choosing an organisation" section. |
| `docs/flows/super-admin.md` | `acting_org` wins over `current_org`. A switch ends acting. Signing in clears `acting_org`. |
| `docs/plans/multi-org-1.md` | Done in the plan commits: Raj's order decision, the rollback window closes at PR 4, the OAuth pin moved to PR 2, and the PR 3 stale-tab gate. |

### `POST /orgs/{id}/switch`

1. `CurrentUser`, not `CurrentMember`. Someone on a waiting page with an
   active membership elsewhere can still switch.
2. `memberships::Model::active_in(db, id, user.id)`. `None` gives a 404 (not
   403), so it does not reveal whether the organisation exists. This covers
   a missing org, pending, declined and removed.
3. `users::Model::end_unlock(db, user.id)`. The org-tied unlock already
   blocks reveals in another org. This is a backstop.
4. The response redirects to `/dashboard` with `redirect_response`, so HTMX
   gets `HX-Redirect`. Either way it sets `current_org = id` and clears
   `acting_org`.
5. `users::Model::close_live_connections(ctx, user.id)`.
6. An info log line: user id, from org and to org. The "from org" comes from
   the request's `current_org` cookie. It is logged only after it parses as
   an `i64`, so a hand-edited cookie can't write arbitrary text into the logs.
   Otherwise it is logged as absent.

It does not bump `session_version`. Other sessions stay signed in.

A super admin can only switch to organisations they belong to. Entering any
other org stays on `/admin/orgs/{id}/enter`.

CSRF: the auth cookie is `SameSite=Lax`, so a cross-site POST carries no
session. `origin_check` also blocks a POST with a foreign `Origin`.

Never add a GET that changes the org. Links, emails and redirects must not
be able to switch someone.

**Switch race.** A chat socket or SSE stream whose upgrade resolved
`CurrentMember` just before a switch can subscribe just after the close
event. It then stays on the old org until it next reconnects. This is not a
leak: the person is still an active member of that org.
Another tab's chat socket can also reconnect (0 to 1 second jitter) with the
old cookie before the browser has the new one, and stay on the old org. Also
not a leak, for the same reason.

## The migrations

There are two migrations, so each can be rolled back on its own. Postgres
runs each migration in a transaction (sea-orm-migration 2.0.4,
`should_use_transaction` defaults to true for Postgres). So `up` or `down`
either fully applies or changes nothing. Each `up` and `down` starts with
`SET LOCAL lock_timeout = '5s'`. If a long query holds the table, the
migration fails fast and doesn't queue every request behind its lock. The
tables are small, so a plain `CREATE INDEX` is fine. `CONCURRENTLY` can't run
in a transaction anyway.

### 1. Memberships: one per organisation per user

**Up**

```sql
SET LOCAL lock_timeout = '5s';
CREATE UNIQUE INDEX "idx-memberships-org-user" ON memberships (organisation_id, user_id);
CREATE INDEX "idx-memberships-user" ON memberships (user_id);
DROP INDEX "idx-memberships-user_id";
```

The plain `user_id` index keeps `for_user`, `default_for_user` and the
`ON DELETE CASCADE` from `users` fast. The unique pair also serves
`active_in` (org and user).

**Down**

```sql
SET LOCAL lock_timeout = '5s';
CREATE UNIQUE INDEX "idx-memberships-user_id" ON memberships (user_id);
DROP INDEX "idx-memberships-user";
DROP INDEX "idx-memberships-org-user";
```

The unique index is created first, so if it fails, nothing else has changed.

**Can production data break the new index?** No. Today
`idx-memberships-user_id` allows at most one row per `user_id`. Two rows
with the same `(organisation_id, user_id)` would share a `user_id`, which
that index already rejects. As a belt-and-braces check before merging, run
this on production. It should return no rows:

```sql
SELECT organisation_id, user_id, count(*) FROM memberships
GROUP BY 1, 2 HAVING count(*) > 1;
```

**Rollback note (the PR must say this).**

- `down` works only while every user has at most one membership. Once
  anyone has two, it fails with a unique violation and changes nothing.
  Rolling back after that would mean deleting memberships.
- Nothing in PR 2 creates a second membership. The first PR that can is
  **PR 4, create another org**, which makes the first second membership in
  production. So `down` is safe until PR 4 ships.
- Reverting only the app code is safe for data: PR 1's code already handles
  several memberships. But keep this migration file in the revert. Otherwise
  the migrator refuses to run ("Migration file of version ... is missing",
  sea-orm-migration `exec.rs`), and the app migrates on boot.

### 2. Users: `secrets_unlocked_org_id`

**Up**

```sql
SET LOCAL lock_timeout = '5s';
ALTER TABLE users ADD COLUMN secrets_unlocked_org_id bigint
  CONSTRAINT "fk-users-secrets_unlocked_org_id-to-organisations"
  REFERENCES organisations (id) ON DELETE SET NULL ON UPDATE CASCADE;
```

**Down**

```sql
SET LOCAL lock_timeout = '5s';
ALTER TABLE users DROP COLUMN secrets_unlocked_org_id;
```

This one can always be rolled back. Unlocks in progress at deploy time have
no org. A null org never matches, so the person unlocks again. That costs at
most 5 minutes.

**Why it is in PR 2.** The cookie is user-controlled. Without the column, a
person could unlock logins in Acme, hand-edit `current_org` to Beta, and the
unlock would still count, revealing Beta's logins. The reveal would be logged
in Beta with no unlock event there. With the column, the unlock only counts
in the org where it happened. `end_unlock` on switch stays as a backstop.

## Cookie rules

**The cookie.** `current_org=<org id>`. HttpOnly, `SameSite=Lax`, `Path=/`,
`Secure` from `settings.secure_cookies` (true in production, false in
development and test, same as the auth cookie). `Max-Age` equals the session
length (7 days), so it survives a browser restart like the session does. It
is not signed. It is only a hint.

**How `CurrentMember` picks the organisation, in order:**

1. A super admin with a valid `acting_org`: acting, as today. `current_org`
   is ignored and left alone, with no clearing cookie, so leaving admin mode
   returns to it.
2. A `current_org` that parses as an integer and where
   `active_in(org, user)` returns a membership: that org. No `Set-Cookie`.
3. Otherwise: `default_for_user`, exactly as today. If a `current_org`
   cookie was present (bad number, no access, deleted org, any status but
   active), set the `StaleCurrentOrg` flag so the response clears it. That
   includes the waiting, removed and declined pages and the `/admin`
   redirect for a super admin with no membership.

Step 2 replaces the `default_for_user` query, so a valid cookie costs no
extra query. A stale one costs one.

**Pending, removed and declined pages.** These show only when the user has
no active membership. `default_for_user` already sorts active memberships
first, and step 2 only accepts an active membership. So a cookie pointing at
an org where you are pending falls through to your active org. Nothing else
to change. One test pins it.

**Acting.** `acting_org` wins while set. A switch clears it. Logout clears
both, as today plus `current_org`.

**Sign in, sign up, join.** All three set a fresh session with
`signed_in_cookies`, which clears `current_org` and `acting_org`. Each sign-in
starts at the default org. A cookie left by another person on a shared
browser never carries over. The cookie is re-checked anyway, so this is
about predictability, not access. For the super admin, sign-in now ends
acting. `acting_org` already has no `Max-Age`, so it ends when the browser
closes. Note this in `docs/flows/super-admin.md`.

**Logout.** Clears the session, `acting_org` and `current_org` on this
browser.

**Sign out everywhere.** Bumps `session_version` and closes live connections,
as today. The bump also clears the unlock and its org. This browser gets a
fresh session cookie and keeps its `current_org`, so you stay in the org you
were in. Other browsers keep their `current_org` cookies, but those do
nothing without a session. Their next sign-in clears them.

**A switch.** Sets `current_org`, clears `acting_org`, ends any logins unlock,
closes the user's chat sockets and notification streams.

### Sockets and streams

How they pick the org today:

- **Chat** (`src/controllers/chat_ws.rs`, `connect`): `CurrentMember` runs at
  upgrade. `member.org.id` and the conversation are fixed in `Session` for
  the socket's life. `still_allowed` checks the session version and
  conversation membership once, after subscribing. A `SignedOut { user_id }`
  event closes the socket.
- **Notifications** (`src/controllers/notifications.rs`, `stream`):
  `CurrentMember` runs at connect, but only `member.user.id` is used.
  `NotifyEvent` is keyed by user, not org. A notice in any org makes every
  open page of that user reload its badge. The badge request
  (`/notifications/unread`) uses `CurrentMember`, so it counts the cookie's
  org.

On a switch, call `users::Model::close_live_connections(ctx, user.id)`. It
publishes `ChatEvent::SignedOut` and `NotifyEvent::SignedOut` without
bumping the session version. The browser reconnects:

- The notification stream reconnects through `CurrentMember` and follows the
  new org.
- A chat socket reconnects to `/chat/{id}/ws`. If that conversation is in
  the old org, `find_for_member(new_org, id, user)` fails, so the upgrade is
  refused. The old tab stops getting live messages. It does not get the
  other org's data. PR 3 should reload such a tab.

Trade-off: this closes the user's sockets on every device, not just this
browser. They reconnect within seconds and reload the feed. Accepted until
PR 5.

### Token pins

**OAuth consent.**

- `authorize_page` adds `"org_id": member.org.id` to `fields`.
- `authorize` (POST), after the acting check:
  - **Deny** goes through without the pin and sends `access_denied` back to
    the client, as today. Denying in the wrong org grants nothing.
  - **Allow** parses `org_id` as an `i64`. If it is missing, empty, not a
    number, or not `member.org.id`, it returns `refused_page` (400) with
    "You switched organisation while this page was open. Start again from
    Claude." No code is issued.
- `org_id` is a `String` in the form struct and parsed by hand, so a bad
  value gets this page rather than a generic form rejection.
- The code is still issued for `member.org.id`. The pin only proves it is the
  org the person saw.
- A consent page rendered before the deploy and submitted after it has no
  `org_id`, so Allow is refused and the person starts again. The page is
  `no-store`, so this is rare.

**Personal Claude token** (More, then Connect Claude).

- The create form (`assets/views/claude/index.html`, line 50) gets a hidden
  `org_id` set to `member.org.id`.
- `create_token` in `src/controllers/claude.rs` reads it next to the name,
  with the same parsing. On a mismatch it re-renders the page with a 409 and
  the same message, and creates no token.

## Other places that pick "the" org

| Place | What it does | PR 2 |
|---|---|---|
| Dashboard and every tenant page | `CurrentMember` | Follows the cookie. No change. |
| Notifications page, badge, stream | `CurrentMember` | Follows the cookie. Stream is user-keyed (above). |
| Chat socket | `CurrentMember` at upgrade | Closed on switch (above). |
| Claude bearer tokens and `/mcp` | `active_in(token org, user)` (PR 1) | Not cookie-based. A cookie sent alongside is ignored. No change. |
| OAuth consent, personal tokens | `CurrentMember` | Pinned (above). |
| Mention email item links (`src/mailers/mentions.rs`: `app_url` + `/tasks/5` etc.) | No org in the link | No change. Opened in another org, they 404. PR 7. |
| Mention email "all notifications" link (`mentions.rs:56`, `/notifications`) | No org in the link | Silently opens the cookie's org, not the email's. PR 7. |
| Security email activity link (`security.rs:112`, `/documents/activity`) | No org in the link | Silently opens the cookie's org's activity, not the org the alert is about. PR 7. |
| `/admin` people table | `default_by_user` | About other users, not cookies. No change. Showing every membership is out of scope. |
| `/admin/orgs/{id}/enter`, `/admin/leave` | `acting_org` | No change. Leave returns to `current_org` if set. |
| `cargo loco task super_admin` | `default_for_user` | CLI, no cookie. No change. |
| Logins unlock | Per user, session and now org | Tied to the org (second migration) and ended on switch. |

## Edge cases

- **Removed from the cookie's org mid-session.** Removal still bumps the
  session version (until PR 5), so the person is signed out everywhere. Even
  without that, the next request fails `active_in`, falls back to the
  default org and clears the cookie. Open sockets for the old org close on
  removal, as today.
- **Organisation deleted.** The app has no delete. If a row is deleted in the
  database, its memberships cascade away. `active_in` returns nothing, and the
  request falls back and clears the cookie. An `acting_org` pointing at it is
  already ignored (`find_by_id` fails). `secrets_unlocked_org_id` is set to
  null.
- **Two tabs: a cross-org disclosure until PR 3.** Tab 1 shows Acme. Tab 2
  switches to Beta. The cookie is per browser, so tab 1's next request runs
  as Beta.
  - Links and forms that carry an id (`/tasks/5`) are scoped by
    `member.org.id`, so they 404.
  - Forms without an id post into Beta while the page shows Acme. A login,
    document, message or task typed for Acme lands in Beta, where Beta's
    members can see it. This is a cross-org disclosure, not just confusion.
  - Before PR 4 nobody has two orgs, so this cannot happen in production.
    **The org check in PR 3 is a hard gate before PR 4 ships:**
    - `hx-headers` on `<body>` with the page's org id, and a submit listener
      in `frontend/app.ts` that adds a hidden `_org` field to plain
      urlencoded forms. Multipart forms are HTMX and carry the header; a
      no-JS multipart form fails closed.
    - When the value differs from the resolved org, `CurrentMember` sends
      the tab to `/dashboard?from=<id>`: 409 with `HX-Redirect` for HTMX and
      `fetch`, 303 for plain forms. Home says what happened. This applies
      while acting and to HTMX GETs too. See `docs/plans/multi-org-3.md`.
    - Once every form carries it, a missing value fails closed for non-GET
      requests.
  - The Claude consent and personal token forms are pinned in PR 2, so they
    can't bind a token to Beta.
  - Tab 1's chat socket closes. Its reconnect is refused.
- **Forged cookie** for an org you are not in. `active_in` fails, the
  request falls back and the cookie is cleared. The response is the same as
  with no cookie, apart from the clearing `Set-Cookie`. Nothing reveals
  whether that org exists. A non-number is treated the same way.
- **Forged cookie plus an unlock.** An Acme unlock does not count in Beta,
  because the unlock now carries its org.
- **Cookie for an org where you are pending or declined**, with an active
  membership elsewhere. Falls back to the active one. You see the waiting
  page only when no membership is active.
- **Acting.** A super admin with both cookies sees the acting org, and a
  stale `current_org` is not cleared while acting. Leaving returns them to
  `current_org` if it is still valid. Switching ends acting. A non-admin with
  an `acting_org` cookie: ignored, as today.
- **Super admin with no membership.** Switching always 404s. A stray
  `current_org` falls back to nothing, and they go to `/admin` with the
  clearing cookie.
- **A switch to the org you are already in.** Allowed. It still closes
  sockets and ends the unlock. Harmless, and simpler than a special case.
- **Switch race.** See "Switch race" above. Not a leak.

## Tests to add

Model (`tests/models/memberships.rs`):

- `one_membership_per_organisation_per_user`: a second membership in the
  same org fails with a unique violation. One in another org succeeds.

Model (`tests/models/users.rs`):

- `an_unlock_counts_only_in_its_own_org`: unlock for Acme, so
  `secrets_unlocked(sid, acme)` is true and `secrets_unlocked(sid, beta)` is
  false.
- `a_session_bump_clears_the_unlock_org`.

Requests (`tests/requests/multi_org.rs`, with the `DROP INDEX` removed):

- `switching_sets_the_cookie_and_pages_follow_it`: switch to Beta, then
  `/dashboard` shows Beta. Switch back, it shows Acme.
- `switching_over_htmx_returns_hx_redirect_with_the_cookies`: `HX-Redirect`
  to `/dashboard`, setting `current_org` and clearing `acting_org`.
- `the_current_org_cookie_is_http_only_lax_and_lasts_a_session`: checks
  `HttpOnly`, `SameSite=Lax`, `Path=/` and `Max-Age`. No `Secure` in test.
- `a_valid_cookie_sends_no_clearing_cookie`: no `Set-Cookie` for
  `current_org` on a normal page.
- `switching_to_an_org_you_are_not_active_in_is_a_404`: a stranger's org,
  pending, declined, removed, and an id that does not exist. No cookie set.
- `switching_needs_a_signed_in_user`: redirects to `/login`.
- `a_forged_cookie_falls_back_and_is_cleared`: `current_org` set to a
  stranger's org. The page shows the default org, and the response clears the
  cookie.
- `a_garbage_cookie_falls_back_and_is_cleared`: `current_org=abc`.
- `a_cookie_for_a_deleted_org_falls_back`: delete Beta's row directly.
- `a_cookie_for_an_org_you_were_removed_from_falls_back`: set the Acme
  membership to removed directly. With `current_org=Acme`, pages show Beta.
- `the_waiting_page_shows_only_without_an_active_membership`: pending in
  Acme, active in Beta, `current_org=Acme` shows Beta. Then Beta removed as
  well shows the Acme waiting page, with the clearing cookie.
- `with_a_beta_cookie_acme_tasks_404_and_new_tasks_go_to_beta`: GET of an
  Acme task is 404. POST of a new task creates it in Beta. This pins today's
  behaviour, which PR 3's check will change.
- `an_acme_token_acts_in_acme_whatever_the_cookie`: `/mcp` with an Acme
  bearer token and `current_org=Beta` lists Acme's projects.
- `logout_clears_current_org_and_acting_org`.
- `signing_in_clears_a_leftover_current_org`.
- `sign_out_everywhere_keeps_this_browsers_org`: no clearing cookie in the
  response. The next page with the new session and old `current_org` shows
  that org.
- `switching_ends_an_unlock_of_logins`: unlock, switch, then
  `secrets_unlocked` is false.
- `an_acme_unlock_does_not_reveal_beta_logins`: unlock in Acme, set
  `current_org=Beta` by hand, and a reveal in Beta is refused.

Requests (`tests/requests/admin.rs`):

- `acting_wins_over_current_org`: a super admin with both cookies sees the
  acting org.
- `acting_with_a_stale_current_org_sends_no_clearing_cookie`.
- `switching_ends_acting`: the switch response clears `acting_org`.
- `a_super_admin_without_a_membership_gets_the_clearing_cookie_on_the_htmx_redirect`:
  an HTMX request with a stale `current_org` gets `HX-Redirect: /admin` and
  the clearing cookie.

Requests (`tests/requests/oauth.rs`):

- `the_allow_page_pins_the_org`: the form has a hidden `org_id` equal to the
  current org.
- `allow_after_a_switch_is_refused`: render the page for Acme, switch to
  Beta, post Allow with Acme's `org_id`. 400, and no code issued.
- `allow_with_a_missing_empty_or_non_numeric_org_is_refused`.
- `deny_without_an_org_still_goes_back_with_access_denied`.
- The `decide` helper (and the inline POST in
  `a_super_admin_acting_elsewhere_is_asked_to_leave_first`) posts the org's
  `org_id`. `the_whole_flow_from_sign_in_to_a_tool_call` checks the issued
  code's `org_id`.

Requests (`tests/requests/claude.rs`):

- `the_token_form_pins_the_org`.
- `a_token_for_another_org_is_refused`: post with Acme's `org_id` while the
  cookie says Beta. No token is created.

Live (`tests/requests/live_sign_out.rs`, real server):

- `switching_closes_the_chat_socket_and_notification_stream`.
- `a_chat_socket_for_the_old_org_is_refused_after_a_switch`: needs Alice
  in two orgs on the real server. Insert the second membership directly, as
  `alice_in_two_orgs` does.

The full suite stays green. Existing multi-org tests with no cookie keep
passing unchanged, which proves the default is the same.

## Out of scope (later PRs)

- **PR 3, switcher UI and the stale-tab org check.** The menu, showing which
  org you are in, unread dots for other orgs, reloading tabs whose chat
  socket was refused. The org check is required and is a hard gate before
  PR 4 ships.
- **PR 4, create another org.** The first way to get a second membership.
- **PR 5, org-scoped removal.** Events keyed by org and user, no session
  bump. Org-scoped close events must stay: once removal stops bumping the
  session, they are the only thing that ends an old-org socket.
- **PR 6, join with an existing account.** Proving the email, same response
  either way, no re-join as active after removal or decline.
- **PR 7, links and the Claude connector.** Org-aware links in emails and
  notifications, including the `/notifications` link in mention emails and
  the `/documents/activity` link in security emails. Connector settings per
  org. Ship the email-link part soon after PR 4.
- The `/admin` people table listing every membership.

## Later / optional

- A `Sec-Fetch-Site: same-origin` check on the switch endpoint, as a third
  layer after `SameSite=Lax` and `origin_check`.
- The `__Host-` cookie prefix for `current_org` (and the other cookies). It
  forces `Secure`, `Path=/` and no `Domain`, so a sibling subdomain can't
  set the cookie. The app now serves at `collab.zen-scale.io`, so any
  `*.zen-scale.io` host could toss cookies at it today. The cookie is only a
  hint and is re-checked, so the risk is low. Dev and test run without
  `Secure`, so the prefix needs a per-environment name.
- A per-device close on switch in PR 5, instead of closing every device's
  sockets.

## Risks

- **Rollback window.** The index migration's `down` stops working once PR 4
  ships. Treat it as forward-only after that. The PR says so. The unlock
  migration can always roll back.
- **Stale tabs disclose across orgs** (see "Two tabs"). It can't happen
  before PR 4, because nobody has two orgs. PR 3's org check is a hard gate
  before PR 4 ships.
- **Email links.** Once someone has two orgs (PR 4), a mention email item for
  Acme opened in Beta gives a 404 until PR 7. Accepted. The `/notifications`
  and `/documents/activity` links silently show the cookie's org. These are
  in PR 7's scope, which should ship soon after PR 4.
- **The clearing layer runs on every request.** It is one extension insert
  and one atomic read. If it is missed for a route, the only effect is a
  stale cookie that keeps falling back.
- **Closing every device's sockets on a switch.** Brief reconnects elsewhere.
  Accepted until PR 5.
- **Entity codegen.** `cargo loco db entities` changes `memberships.rs`
  (unique attributes), `organisations.rs` (`has_one` becomes `has_many` for
  memberships) and `users.rs` (new column). These are expected. Keep them.
  Review the rest of the diff for anything else.
- **A hidden "the org" reader.** Grepped every `default_for_user`,
  `for_user`, `CurrentUser`, `acting_org` and mailer link. All are covered
  above.

## Decisions for Raj

Each comes with the reviewers' recommendation.

1. **The rollback window closes at PR 4, not PR 6.** Recommend: accept.
2. **Email-link 404s between PR 4 and PR 7.** Recommend: accept, and ship
   PR 7's email-link part soon after PR 4.
3. **The stale-tab org check.** Recommend: required in PR 3, as a hard gate
   before PR 4.
4. **Super admin sign-in drops acting.** Recommend: accept. `acting_org`
   already ends when the browser closes. Note it in
   `docs/flows/super-admin.md`.
5. **A switch closes every device's sockets.** Recommend: accept until PR 5.

## Checks

```sh
cargo loco db migrate
cargo loco db entities
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

Also run each migration's `down` then `up` locally on a database with
real-looking data, to prove `down` works while everyone has one membership.

Before merging, run the duplicate check query above on production.

No visible change, so no screenshots. The PR says "No visible change". It
also describes both migrations and says whether each can be rolled back:
the index one until PR 4 ships, and the unlock one always.
