# Multi-org 2: one membership per organisation, and the current_org cookie

Status: Draft · PR 2 of 7 · Owner: Raj · 19 October 2026

## Summary

PR 1 (#41) stopped the code assuming one membership per user. This PR lets
the database hold more than one, and lets a browser choose which organisation
it is working in.

- A migration replaces the unique index on `memberships.user_id` with a
  unique index on `(organisation_id, user_id)` and a plain index on
  `user_id`.
- A new `current_org` cookie says which organisation this browser is in. It
  is only a hint. Every request checks it again.
- A new endpoint, `POST /orgs/{id}/switch`, is the only thing that sets the
  cookie. It has no button yet. The switcher UI is PR 3.
- The Claude "Allow access" form pins the organisation it was shown for.

Nothing changes for anyone today. Nobody has two memberships yet, so a switch
can only pick the organisation you are already in.

## What sets the cookie in PR 2

**Recommendation: add `POST /orgs/{id}/switch` now, with no UI.**

Why:

- It makes the PR testable end to end. Without it, the cookie-reading code
  has no real writer, and tests would only ever forge the cookie.
- Everything a switch must do (check access, clear `acting_org`, close live
  connections, end an unlock) lands in one place and gets one security
  review, together with the cookie rules.
- PR 3 becomes a pure UI change: a menu that posts to an endpoint that
  already exists and is tested.
- The endpoint can't grant anything. It only accepts an organisation where
  you already have an active membership.

The cost: the OAuth pin must ship in this PR, since a switch now exists. It is
small (one hidden field and one check).

The other option, nothing until PR 3, keeps PR 2 to the migration and a
reader nobody can trigger. It is smaller but proves less, and PR 3 would then
carry both the security rules and the UI.

## What changes, file by file

| File | Change |
|---|---|
| `migration/src/m20261019_xxxxxx_memberships_one_per_org.rs` (new, from `cargo loco generate migration MembershipsOnePerOrg`) | The index swap. See "The migration". |
| `migration/src/lib.rs` | Registered by the generator. |
| `src/models/_entities/memberships.rs` | Regenerated with `cargo loco db entities`. Expect `#[sea_orm(unique)]` to go from `user_id`. Commit whatever codegen produces. |
| `src/extractors/current_member.rs` | Add `CURRENT_ORG_COOKIE = "current_org"`. Read it after the acting check. See "Cookie rules". |
| `src/extractors/session.rs` | Add `current_org_cookie(ctx, org_id)` and `cleared_current_org_cookie(ctx)`, built with `base_cookie` like `acting_org`, plus `Max-Age` equal to the JWT expiration (7 days). Add `signed_in_cookies(ctx, user)`: the session cookie plus cleared `current_org` and `acting_org`. |
| `src/initializers/current_org.rs` (new) | A small layer, added in `after_routes` like `origin_check`. It puts a `StaleCurrentOrg` flag (an `Arc<AtomicBool>`) in the request extensions. If `CurrentMember` sets it, the layer adds a cleared `current_org` cookie to the response. An extractor cannot set cookies itself. |
| `src/app.rs` | Register the new initializer. |
| `src/controllers/orgs.rs` (new, `cargo loco generate controller orgs`) | `POST /orgs/{id}/switch`. See below. PR 4 ("create another org") will live here too. |
| `src/controllers/auth.rs` | `login` uses `signed_in_cookies`. `logout` also clears `current_org`. `sign_out_everywhere` is unchanged (see "Cookie rules"). |
| `src/controllers/signup.rs`, `src/controllers/join.rs` | Use `signed_in_cookies`, so a cookie left by someone else on a shared browser is cleared. |
| `src/models/users.rs` | Add `end_unlock(db, user_id)`: clears `secrets_unlocked_until` and `secrets_unlocked_sid`. Same columns `increment_session_version` clears, without ending sessions. |
| `src/controllers/oauth.rs` | `AuthorizeParams` gets `org_id: Option<i64>`. The GET page adds it to `fields`. POST refuses when it is missing or not `member.org.id`. |
| `assets/views/oauth/authorize.html` | No change needed. It already writes every entry in `fields` as a hidden input. |
| `src/data/chat_hub.rs`, `src/data/notify_hub.rs` | Doc comments only: `SignedOut` now also fires on a switch. No rename, to keep the diff small. |
| `tests/requests/multi_org.rs` | Remove the `DROP INDEX` from `alice_in_two_orgs` and the module comment about it. Add the cookie tests. |
| `tests/models/memberships.rs`, `tests/requests/oauth.rs`, `tests/requests/live_sign_out.rs` | New tests, listed below. |
| `docs/architecture/erd.md` | New indexes. Remove "unique today" and the one-membership rule. |
| `docs/flows/accounts.md` | Sign in, sign out and sign out everywhere: what happens to `current_org`. A short "Choosing an organisation" section. |
| `docs/flows/super-admin.md` | `acting_org` wins over `current_org`. A switch ends acting. |
| `docs/plans/multi-org-1.md` | Order line: Raj decided on 19 October (done in this plan's commit). |

### `POST /orgs/{id}/switch`

1. `CurrentUser`, not `CurrentMember`. Someone on a waiting page with an
   active membership elsewhere can still switch.
2. `memberships::Model::active_in(db, id, user.id)`. `None` gives a 404 (not
   403), so it does not reveal whether the organisation exists. This covers
   a missing org, pending, declined and removed.
3. `users::Model::end_unlock(db, user.id)`.
4. The response redirects to `/dashboard` (`redirect_response`, so HTMX gets
   `HX-Redirect`). It sets `current_org = id` and clears `acting_org`.
5. `users::Model::close_live_connections(ctx, user.id)`.
6. An info log line: user id, from org (if known), to org.

It does not bump `session_version`. Other sessions stay signed in.

A super admin can only switch to organisations they belong to. Entering any
other org stays on `/admin/orgs/{id}/enter`.

CSRF: the auth cookie is `SameSite=Lax`, so a cross-site POST carries no
session. `origin_check` also blocks a POST with a foreign `Origin`.

## The migration

Postgres runs each migration in a transaction (sea-orm-migration 2.0.4,
`should_use_transaction` defaults to true for Postgres). So `up` or `down`
either fully applies or changes nothing. The table is small, so a plain
`CREATE INDEX` is fine. `CONCURRENTLY` can't run in a transaction anyway.

**Up**

```sql
CREATE UNIQUE INDEX "idx-memberships-org-user" ON memberships (organisation_id, user_id);
CREATE INDEX "idx-memberships-user" ON memberships (user_id);
DROP INDEX "idx-memberships-user_id";
```

The plain `user_id` index keeps `for_user`, `default_for_user` and the
`ON DELETE CASCADE` from `users` fast. The unique pair also serves
`active_in` (org and user).

**Down**

```sql
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
  **PR 4, create another org** (an existing user owns a new org). Join with
  an existing account (PR 6) is the second. So `down` is safe until PR 4
  ships, not PR 6.
- Reverting only the app code is safe for data: PR 1's code already handles
  several memberships. But keep this migration file in the revert. Otherwise
  the migrator refuses to run ("Migration file of version ... is missing",
  sea-orm-migration `exec.rs`), and the app migrates on boot.

## Cookie rules

**The cookie.** `current_org=<org id>`. HttpOnly, `SameSite=Lax`, `Path=/`,
`Secure` from `settings.secure_cookies` (true in production, false in
development and test, same as the auth cookie). `Max-Age` equals the session
length (7 days), so it survives a browser restart like the session does. It
is not signed. It is only a hint.

**How `CurrentMember` picks the organisation, in order:**

1. A super admin with a valid `acting_org`: acting, as today. `current_org`
   is ignored but left alone, so leaving admin mode returns to it.
2. A `current_org` that parses as an integer and where
   `active_in(org, user)` returns a membership: that org.
3. Otherwise: `default_for_user`, exactly as today. If a `current_org`
   cookie was present (bad number, no access, any status but active), set
   the `StaleCurrentOrg` flag so the response clears it.

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
starts at the default org. This also means a cookie left by another person
on a shared browser never carries over. The cookie is re-checked anyway, so
this is about predictability, not access. Clearing `acting_org` on sign-in is
a small change for the super admin. Today an expired session while acting
resumes acting at the next sign-in.

**Logout.** Clears the session, `acting_org` and `current_org` on this
browser.

**Sign out everywhere.** Bumps `session_version` and closes live connections,
as today. This browser gets a fresh session cookie and keeps its
`current_org`, so you stay in the org you were in. Other browsers keep their
`current_org` cookies, but those do nothing without a session. Their next
sign-in clears them.

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
  other org's data. PR 3 should reload or redirect such a tab.

Trade-off: this closes the user's sockets on every device, not just this
browser. They reconnect within seconds and reload the feed. Fine for now.
Org-scoped removal (PR 5) needs events keyed by org and user anyway. A
narrower close can come with it.

### OAuth consent pin

- `authorize_page` adds `"org_id": member.org.id` to `fields`.
- `authorize` (POST), after the acting check and before the decision: if
  `params.org_id != Some(member.org.id)`, return `refused_page` (400) with
  "You switched organisation while this page was open. Start again from
  Claude." This applies to Allow and Deny alike.
- The code is still issued for `member.org.id`. The pin only proves it is the
  org the person saw.
- A consent page rendered before the deploy and submitted after it has no
  `org_id`, so it is refused. The person starts again. The page is
  `no-store`, so this is rare.

## Other places that pick "the" org

| Place | What it does | PR 2 |
|---|---|---|
| Dashboard and every tenant page | `CurrentMember` | Follows the cookie. No change. |
| Notifications page, badge, stream | `CurrentMember` | Follows the cookie. Stream is user-keyed (above). |
| Chat socket | `CurrentMember` at upgrade | Closed on switch (above). |
| Claude bearer tokens | `active_in(token org, user)` (PR 1) | Not cookie-based. No change. |
| OAuth consent | `CurrentMember` | Pinned (above). |
| Email links (`src/mailers/mentions.rs`: `app_url` + `/tasks/5` etc.) | No org in the link | No change. A link into an org that is not the cookie's org will 404 once someone has two orgs. PR 7. |
| `/admin` people table | `default_by_user` | About other users, not cookies. No change. Showing every membership is out of scope. |
| `/admin/orgs/{id}/enter`, `/admin/leave` | `acting_org` | No change. Leave returns to `current_org` if set. |
| `cargo loco task super_admin` | `default_for_user` | CLI, no cookie. No change. |
| Logins unlock | Per user and session, not per org | Ended on switch. The unlock is logged in the org where it happened, so it must not carry into another org. |

## Edge cases

- **Removed from the cookie's org mid-session.** Removal still bumps the
  session version (until PR 5), so the person is signed out everywhere. Even
  without that, the next request fails `active_in`, falls back to the
  default org and clears the cookie. Open sockets for the old org close on
  removal, as today.
- **Organisation deleted.** The app has no delete. If a row is deleted in the
  database, its memberships cascade away, `active_in` returns nothing, and the
  request falls back and clears the cookie. An `acting_org` pointing at it is
  already ignored (`find_by_id` fails).
- **Two tabs.** Tab 1 shows Acme. Tab 2 switches to Beta. The cookie is per
  browser, so tab 1's next request runs as Beta.
  - Links and forms that carry an id (`/tasks/5`) are scoped by
    `member.org.id`, so they 404. No data crosses orgs.
  - Forms without an id ("new task", "new message" via HTTP) would create the
    item in Beta while the page shows Acme. Confusing, though not a leak:
    the person is a member of both. Unfixed in PR 2. Proposed for PR 3:
    every page sends its org id in an `X-Org-Id` header (`hx-headers` on
    `body`). `CurrentMember` answers 409 with `HX-Refresh` when it differs.
    Plain (non-HTMX) form posts would need a hidden field.
  - The Claude consent form is pinned, so it can't bind a token to Beta.
  - Tab 1's chat socket closes. Its reconnect is refused.
- **Forged cookie** for an org you are not in. `active_in` fails, the
  request falls back and the cookie is cleared. The response is the same as
  with no cookie, apart from the clearing `Set-Cookie`. Nothing reveals
  whether that org exists. A non-number is treated the same way.
- **Cookie for an org where you are pending or declined**, with an active
  membership elsewhere. Falls back to the active one. You see the waiting
  page only when no membership is active.
- **Acting.** A super admin with both cookies sees the acting org. Leaving
  returns them to `current_org` if it is still valid. Switching ends acting.
  A non-admin with an `acting_org` cookie: ignored, as today.
- **Super admin with no membership.** Switching always 404s. A stray
  `current_org` falls back to nothing, and they go to `/admin`, as today.
- **A switch to the org you are already in.** Allowed. It still closes
  sockets and ends the unlock. Harmless, and simpler than a special case.

## Tests to add

Model (`tests/models/memberships.rs`):

- `one_membership_per_organisation_per_user`: a second membership in the
  same org fails with a unique violation. One in another org succeeds.

Requests (`tests/requests/multi_org.rs`, with the `DROP INDEX` removed):

- `switching_sets_the_cookie_and_pages_follow_it`: switch to Beta, then
  `/dashboard` shows Beta. Switch back, it shows Acme.
- `the_current_org_cookie_is_http_only_lax_and_lasts_a_session`: checks
  `HttpOnly`, `SameSite=Lax`, `Path=/` and `Max-Age`. No `Secure` in test.
- `switching_to_an_org_you_are_not_active_in_is_a_404`: a stranger's org,
  pending, declined, removed, and an id that does not exist. No cookie set.
- `switching_needs_a_signed_in_user`: redirects to `/login`.
- `a_forged_cookie_falls_back_and_is_cleared`: `current_org` set to a
  stranger's org. The page shows the default org, and the response clears the
  cookie.
- `a_garbage_cookie_falls_back_and_is_cleared`: `current_org=abc`.
- `a_cookie_for_an_org_you_were_removed_from_falls_back`: set the Acme
  membership to removed directly. With `current_org=Acme`, pages show Beta.
- `the_waiting_page_shows_only_without_an_active_membership`: pending in
  Acme, active in Beta, `current_org=Acme` shows Beta. Then Beta removed as
  well shows the Acme waiting page.
- `logout_clears_current_org_and_acting_org`.
- `signing_in_clears_a_leftover_current_org`.
- `sign_out_everywhere_keeps_this_browsers_org`: no clearing cookie in the
  response. The next page with the new session and old `current_org` shows
  that org.
- `switching_ends_an_unlock_of_logins`: unlock, switch, then
  `secrets_unlocked` is false.

Requests (`tests/requests/admin.rs`):

- `acting_wins_over_current_org`: a super admin with both cookies sees the
  acting org.
- `switching_ends_acting`: the switch response clears `acting_org`.

Requests (`tests/requests/oauth.rs`):

- `the_allow_page_pins_the_org`: the form has a hidden `org_id` equal to the
  current org.
- `allow_after_a_switch_is_refused`: render the page for Acme, switch to
  Beta, post Allow with Acme's `org_id`. 400, and no code issued.
- `allow_without_an_org_is_refused`.
- The `decide` helper (and the inline POST in
  `a_super_admin_acting_elsewhere_is_asked_to_leave_first`) posts the org's
  `org_id`. `the_whole_flow_from_sign_in_to_a_tool_call` checks the issued
  code's `org_id`.

Live (`tests/requests/live_sign_out.rs`, real server):

- `switching_closes_the_chat_socket_and_notification_stream`.
- `a_chat_socket_for_the_old_org_is_refused_after_a_switch`: needs Alice
  in two orgs on the real server. Insert the second membership directly, as
  `alice_in_two_orgs` does.

The full suite stays green. Existing multi-org tests with no cookie keep
passing unchanged, which proves the default is the same.

## Out of scope (later PRs)

- **PR 3, switcher UI.** The menu, showing which org you are in, unread dots
  for other orgs, reloading tabs whose chat socket was refused, and the
  `X-Org-Id` stale-tab check (if Raj agrees).
- **PR 4, create another org.** The first way to get a second membership.
- **PR 5, org-scoped removal.** Events keyed by org and user, no session
  bump, closing only that org's sockets.
- **PR 6, join with an existing account.** Proving the email, same response
  either way, no re-join as active after removal or decline.
- **PR 7, links and the Claude connector.** Org-aware links in emails and
  notifications, and connector settings per org.
- The `/admin` people table listing every membership.

## Risks

- **Rollback window.** `down` stops working once PR 4 ships. Treat the
  migration as forward-only after that. The PR says so.
- **Email links 404 across orgs.** Once someone has two orgs (PR 4), a
  mention email for Acme opened while the browser is in Beta gives a 404,
  until PR 7. Nobody has two orgs before then.
- **Stale tabs post into the current org** (see "Two tabs"). Not a leak.
  Proposed fix in PR 3.
- **The clearing layer runs on every request.** It is one extension insert
  and one atomic read. If it is missed for a route, the only effect is a
  stale cookie that keeps falling back.
- **Closing every device's sockets on a switch.** Brief reconnects elsewhere.
  Accepted until PR 5.
- **Entity codegen.** `cargo loco db entities` may rewrite more than the
  `unique` attributes. Review the diff. Only the memberships entity should
  change.
- **A hidden "the org" reader.** Grepped every `default_for_user`,
  `for_user`, `CurrentUser` and `acting_org` use. All are covered above.

## Checks

```sh
cargo loco db migrate
cargo loco db entities
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

Also run the migration `down` then `up` locally on a database with real-looking
data, to prove `down` works while everyone has one membership.

Before merging, run the duplicate check query above on production.

No visible change, so no screenshots. The PR says "No visible change". It
also describes the migration and says whether it can be rolled back (yes,
until PR 4 ships).
