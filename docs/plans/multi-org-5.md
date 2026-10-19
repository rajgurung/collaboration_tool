# Multi-org 5: org-scoped removal

Status: Draft · PR 5 of 7 · Owner: Raj · 19 October 2026

## Summary

Since PR 4 (#45), a real person can be active in two organisations. Removing
someone from Beta today does two account-wide things in
`memberships::Model::remove` (`src/models/memberships.rs:368`):

- `users::Model::increment_session_version` signs them out of every org,
  Acme included.
- `users::Model::close_live_connections` closes every chat socket and
  notification stream they have, in every org.

So Beta's owner can disrupt the person's work in Acme. After this PR, removal
from Beta only touches Beta.

**What removal does after this PR**, in the same transaction as today:

- The membership becomes `removed`, with `removed_at` and `removed_by_id`.
- Their Claude tokens and unused OAuth codes in Beta are revoked (as today).
- They leave every Beta conversation (as today).
- Their unsent Beta mention emails are cancelled (as today).
- A `member_removed` row goes into Beta's login activity (as today).
- New: any logins unlock they hold for Beta ends. An unlock for Acme stays.
- Gone: the session version bump.

After the commit, only their Beta chat sockets and Beta notification streams
close, on every device. Acme sockets, streams, sessions, tokens and unlocks
are untouched.

**What stops Beta being served.** The per-request `active_in` check that
`CurrentMember` already runs. The removed person's Beta tab fails on its next
request:

| Path | What stops it |
|---|---|
| Any page, HTMX call or form | `CurrentMember`: the `current_org` cookie (Beta) fails `active_in`, falls back to the default active org and clears the cookie. A request that claims Beta (`X-Org-Id` or `_org`) is then refused by the stale-tab check (PR 3). Nothing is saved. |
| No active org left | `CurrentMember` returns the removed page, as today. New: HTMX and `fetch` calls get a 409 with `HX-Redirect: /dashboard` instead of a 403 they can't show. |
| Chat socket bound to Beta at upgrade | The new org-scoped close event. Backed by `still_allowed`, which gains an `active_in` check, and by conversation membership, which removal deletes. |
| Notification stream | The org-scoped close event. The stream now knows its org. It also re-checks `active_in` once after subscribing, to close the same race chat already closes. |
| Claude Allow page and token form (`PinnedMember`) | The cookie falls back, so their `org_id` pin no longer matches. Refused as today. |
| Logins unlock bound to Beta | Cleared in the removal transaction. It could not be used anyway (`secrets_unlocked` needs the org, and `CurrentMember` no longer resolves Beta), but a rejoin in PR 6 must not inherit it. |
| Claude bearer tokens | Revoked, and `bearer.rs` checks `active_in` on every call (PR 1). |

**The org-scoped close events are now the only thing that ends a Beta socket
or stream.** Before, the session bump was a second net: `still_allowed` and
`CurrentUser` would catch a missed event. This PR keeps both nets for
account-wide events and adds `active_in` to `still_allowed` and to the stream
for the org-scoped one. A test pins that removal closes Beta's socket and
stream without any session change.

**What the person sees.**

- **Still in another org.** Their Beta tabs move to Home in Acme within
  seconds: the socket or stream closes, `app.ts` probes `/orgs/current`, the
  probe is refused, and the tab follows `HX-Redirect`. Home shows a new
  removal notice: "You no longer have access to Beta. You're now in Acme."
  and "An owner or admin removed you. Nothing from this tab was saved." No
  "Back to Beta". A tab holding typed text stays put and shows a removal
  banner instead of the switch banner (see "The removal banner").
- **Beta was their only active org.** Their next request shows the existing
  removed page, "You were removed from Beta", while they stay signed in. Its
  only action is Sign out, as today. An open tab gets there through the new
  409 path. Today they were signed out first and saw it after signing in.
- **A fresh page load** (typing an address, a bookmark) with the Beta cookie
  lands in Acme with no notice. The cookie is cleared. Only probes and
  refused requests carry `?from=`. Accepted (see "Edge cases").

**Decisions in this plan, each with a recommendation for Raj** (see
"Decisions for Raj"):

1. Close events keyed by scope: the whole account, one org, or one browser.
2. Narrower close on switch: do it here, per browser (by sign-in id).
3. Notification pings keyed by org too, so a notice in Acme no longer pings a
   Beta page.
4. An account lock: tell the lock org's owners (as today) and, new, the
   person. Not the other orgs' owners.
5. Fold in "password reset revokes Claude tokens".
6. Leave an organisation: out of scope.
7. No migration.

## What changes, file by file

### Close events

| File | Change |
|---|---|
| `src/data/live.rs` (new, in `src/data/mod.rs`) | `pub enum CloseScope { Account { user_id }, Org { org_id, user_id }, Browser { user_id, session_id: Arc<str> } }` and one pure function, `CloseScope::closes(&self, org_id, user_id, session_id: Option<&str>) -> bool`. `Account` matches every socket of that user. `Org` matches only sockets bound to that org. `Browser` matches only sockets opened with that sign-in id. A socket with no sign-in id matches `Account` only. Unit tests sit here. |
| `src/data/chat_hub.rs` | `ChatEvent::SignedOut { user_id }` becomes `ChatEvent::Close(CloseScope)`. Doc comment lists who sends which scope. |
| `src/data/notify_hub.rs` | `NotifyEvent::SignedOut(i64)` becomes `NotifyEvent::Close(CloseScope)`. `Changed(i64)` becomes `Changed { org_id, user_id }`. `publish(&self, org_id, user_ids)`. `sign_out(user_id)` becomes `close(scope)`. Drops `Copy` (the scope holds an `Arc<str>`); keeps `Clone`, `PartialEq`. |
| `src/models/users.rs` | `close_live_connections(ctx, user_id)` keeps its name and sends `CloseScope::Account`: sign out everywhere, reset and lock still close everything. New `close_in_org(ctx, org_id, user_id)` sends `Org`. New `close_in_browser(ctx, user_id, session_id: Option<&str>)` sends `Browser`, or `Account` when there is no sign-in id (cookies from before sign-in ids). New `end_unlock_in(db, user_id, org_id)`: clears the three unlock columns only where `secrets_unlocked_org_id = org_id`. One `UPDATE`. |
| `src/controllers/chat_ws.rs` | `Session` gains `session_id: Option<String>` from `member.session_id`. The loop matches `ChatEvent::Close(scope) if scope.closes(org_id, user_id, sid)`. `wants` treats `Close` as not wanted, as for `SignedOut` today. `still_allowed` adds `memberships::Model::active_in(org_id, user_id)` after the session version check. One more indexed query per socket open. |
| `src/controllers/notifications.rs` | `send` publishes with its `org_id`. `stream` passes `member.org.id` and `member.session_id` to `events_for`, which forwards only `Changed` for its org and user and ends on a matching `Close`. After subscribing, `stream` re-checks `active_in(member.org.id, member.user.id)` and returns an empty, ended stream if it fails. Skipped while acting (an acting admin has no membership there). |

### Removal

| File | Change |
|---|---|
| `src/models/memberships.rs` | `remove`: drop `increment_session_version`. Add `users::Model::end_unlock_in(&txn, user_id, org_id)`. After the commit, `users::Model::close_in_org(ctx, org_id, user_id)` instead of `close_live_connections`. Add an info log after the commit, numbers only: `org_id`, `user_id`, `remover_id`, "removed member". Doc comment rewritten: removal ends access to this org only. New `in_org_for_user(db, org_id, user_id)`: the membership in that org with any status, for the notice and the banner. |
| `src/extractors/current_member.rs` | Two changes, below. |
| `src/controllers/dashboard.rs` | `stale_notice`: when `active_in(from)` fails, `in_org_for_user(from)`. If its status is `removed`, return `{ "id": null, "name": org.name, "removed": true }`. Pending, declined, no row, a deleted org or an error: the nameless notice, as today. |
| `assets/views/dashboard/index.html` | The notice gets a `stale.removed` branch: "You no longer have access to {{ stale.name }}. You're now in {{ org.name }}." and "An owner or admin removed you. Nothing from this tab was saved." No Back form. Same card, icon and Dismiss. |
| `assets/views/layouts/app.html` | A second template, `#gone-banner-template`, next to `#stale-banner-template`: "You no longer have access to {{ org.name }}." and "Copy anything you need from this tab, then go to Home." Only a "Go to Home" button. `org.name` is the page's own org, already on the page. |
| `frontend/app.ts` | `goHomeOrHold(home, gone)`: when holding, clone `#gone-banner-template` if `gone`, else the stale one. The `htmx:beforeOnLoad` handler and `staleRefusal` pass `gone` from the response's `X-Org-Gone` header. Nothing else changes. |

**`current_member.rs`, change 1: mark refusals for a removed org.** When
`check_claim` refuses a claim `Org(id)`, `from_request_parts` looks up
`in_org_for_user(id, user)`. If its status is `removed`, the refusal adds
`X-Org-Gone: 1`. One query, only on the refusal path. `check_claim` stays a
pure comparison: the lookup only picks the banner's words, never whether to
refuse. Missing and garbage claims get no header.

**`current_member.rs`, change 2: no active org, over HTMX or `fetch`.** In
`resolve_org`, when the user has no active membership and the request has
`HX-Request` or `X-Org-Id` (the same test as `stale_tab_refusal`), answer 409
with `HX-Redirect: /dashboard`, `Cache-Control: no-store`, and `X-Org-Gone: 1`
when the claimed org's membership is `removed`. Plain page loads and plain
forms still get the waiting, removed or declined page with a 403, as today.
A super admin with no membership still goes to `/admin`. Without this, an
open tab whose last org was removed gets 403s that HTMX doesn't swap, and
nothing on screen changes.

### Switch: close this browser only

| File | Change |
|---|---|
| `src/controllers/orgs.rs` | `enter_org` takes the request's sign-in id and calls `users::Model::close_in_browser(ctx, user_id, sid)` instead of `close_live_connections`. `switch` and `create` read the sid from the `SessionId` request extension that `CurrentUser` sets. |

Why per browser and not per org: the cookie is per browser, so a switch
changes nothing for the person's other browsers. Closing the old org's sockets
everywhere would still close a laptop that is legitimately in the old org.
The sign-in id is per browser, so `Browser` closes exactly the tabs whose
cookie just changed.

### Copy

| File | Today | After |
|---|---|---|
| `assets/views/members/_row.html` (remove dialog) | "They are signed out on every device straight away." | "They lose access to {{ org.name }} straight away, on every device." |
| `assets/views/members/rotate.html` | "{{ former.username }} has been removed and signed out." | "{{ former.username }} has been removed." |
| `src/mailers/security/password_changed/text.t`, `html.t` | "Every device that was signed in has been signed out." | Adds: "Claude was disconnected too. If you use it, connect it again from More, then Connect Claude." |

The remove dialog's other lines stay. "Their Claude connections stop working"
is still true and only for this org.

### Account lock and password reset

| File | Change |
|---|---|
| `src/models/users.rs` | `reset_password_everywhere` calls `access_tokens::Model::revoke_all_for_user(&txn, user.id)` inside its transaction, after the session bump. `lock_account`'s doc comment names who is told. |
| `src/mailers/security.rs` | New `SecurityMailer::your_account_locked(ctx, user, org)`: to the person, from a new `src/mailers/security/your_account_locked/` (subject, text, html). Only if Raj accepts decision 4. |
| `src/controllers/logins.rs` | `lock`: after `account_locked` to the owners, `your_account_locked` to the person. Each failure is logged on its own, as `tell_about_reset` does. Only once per lock (inside the existing `if`). |

The person's email: subject "Your Collaboration Tool account was locked".
Body: "Someone typed a wrong app password 5 times while trying to see logins
in Acme, so your account is locked and signed out everywhere. Claude was
disconnected. To unlock it, reset your password: {{ forgot_url }}. If it
wasn't you, someone may be using your phone or computer." It names only the
org where the lock happened, which the person belongs to.

### Docs

| File | Change |
|---|---|
| `docs/flows/accounts.md` | "Removing a member": no session bump; only this org's sockets and streams close; the unlock for this org ends; what the person sees. The "Sign out everywhere" closing list drops "removal from an organisation". "Choosing an organisation": a switch closes this browser's sockets only. "Password reset": tokens revoked in every org. A short sequence diagram for removal with two orgs. |
| `docs/flows/chat.md` | Line 60: sockets close on sign out everywhere, reset or lock (every org), removal (that org) and a switch (that browser). `still_allowed` checks the membership too. |
| `docs/flows/notifications.md` | The stream follows one org. A notice in another org doesn't ping it. It ends on removal from its org. |
| `docs/flows/claude-connector.md` | A password reset revokes tokens in every org. |
| `docs/flows/documents.md` | Removal ends an unlock for that org only. Who is told about a lock. |
| `docs/architecture/erd.md` | `session_version`: bumped by sign out everywhere, reset and lock, no longer by removal. |
| `docs/plans/multi-org-1.md` | "Later PRs": mark PR 5's constraints as done, and add the PR 6 constraints below. |

No migration. No new crate.

## Edge cases

- **Beta was their only active org.** Removed page on the next request, still
  signed in. Open tabs reach it through the 409. The SSE reconnect gets a 403
  and stops. The chat socket's reconnect is refused, and its close already
  ran the probe.
- **No active org, but pending in Charlie.** `default_for_user` picks the
  oldest non-active membership, so they may see Charlie's waiting page instead
  of Beta's removed page. Same rule as today. Accepted.
- **Removed from two orgs, no active one.** The removed page names the
  oldest. Accepted.
- **A fresh page load with the Beta cookie.** Lands in Acme silently, and the
  cookie is cleared. The open-tab path, which is the common one, shows the
  notice. A notice on plain loads would need the stale-cookie path to redirect
  every GET. Deferred.
- **A Beta item link** (`/tasks/58`) opened after removal: the PR 4 404 page,
  "We couldn't find that in Acme. It may be in another of your
  organisations." Slightly off for a removal, but it reveals nothing.
  Accepted until PR 7.
- **Another device in Acme.** Untouched. Its cookie is its own, and only Beta
  sockets close.
- **Another device in Beta.** Its Beta sockets and stream close (`Org` scope
  matches every device). Its next request falls back as above.
- **Two tabs, the browser on Acme, an old tab still drawn for Beta.** That tab
  was already stale. Its next request is refused with `X-Org-Gone`, so it
  shows the removal copy.
- **The subscribe race.** A Beta socket upgrading while the removal commits.
  `run` subscribes, then `still_allowed` runs. The close event is published
  after the commit. Either `still_allowed` sees the commit (no conversation
  membership, not active) and closes, or the socket subscribed before the
  publish and gets the event. Same for the stream, with the new re-check.
- **A request in flight at the commit.** One that passed `CurrentMember` just
  before the commit finishes into Beta, including an upload in progress. Same
  as today: the session bump never stopped in-flight requests either.
- **A lagged hub.** A socket or stream that falls behind closes, as today, and
  reconnects through `CurrentMember`.
- **Acting super admin.** Can't remove anyone (`require_own_membership`), as
  today. If an admin who is also a member of Beta is removed while acting in
  Beta, their acting stream closes and reconnects; acting doesn't depend on
  membership. The stream's `active_in` re-check skips acting.
- **Sign out everywhere, reset, lock.** Still `Account` scope: every socket
  and stream, every org. Unchanged.
- **A switch.** Closes only this browser's sockets and streams. A cookie with
  no sign-in id falls back to `Account`, which is today's behaviour.
- **Notices across orgs.** A mention in Acme no longer pings a page showing
  Beta. The badge counts Beta only, so nothing changes on screen. The 60
  second badge poll stays as the fallback.
- **The last owner, and removing yourself.** Unchanged: refused with "The last
  owner can't be removed." and "You can't remove yourself."
- **Removed while unlocked in Acme.** The Acme unlock keeps running out its 5
  minutes. Test pins it.
- **Login activity.** One `member_removed` row in Beta, with the remover and
  the removed username, as today. Nothing in Acme: Acme had no event. No
  `sign_out_everywhere` row, since no session ends.
- **Notifications about the removal.** None, as today: no email and no in-app
  notice to the removed person or anyone else
  (`docs/flows/notifications.md:70`). The removed page and the Home notice
  tell the person.
- **Lock caused in Acme.** Locks the whole account and signs it out
  everywhere, as today. Acme's owners and the person are emailed (decision 4).
  Beta's owners are not: Beta's logins were never at risk (the unlock is tied
  to Acme), and telling them would show Beta's owners that the person is in
  another org.
- **Rejoin after removal (PR 6).** Constraints for PR 6, written into
  multi-org-1.md:
  - The removed row stays, and `(organisation_id, user_id)` is unique, so a
    rejoin updates that row. It never inserts a second one.
  - No path from `removed` straight to `active`. A rejoin goes to `pending`,
    and an owner approves it. `approve` re-adds `#general`.
  - Revoked tokens stay revoked. A Beta unlock is already gone.
  - Once re-approved, open sessions regain Beta without signing in again,
    since removal no longer ends them. That is expected.
- **Deploy.** The hubs are in memory, and the app runs as one instance. The
  restart closes every socket, and they reconnect on the new code. No mixed
  event versions.

## Tests

Unit (`src/data/live.rs`):

- `account_scope_closes_every_socket_of_that_user`: any org, any sign-in id,
  none, but not another user.
- `org_scope_closes_only_that_orgs_sockets`.
- `browser_scope_closes_only_that_sign_in`: same user, another sign-in id, is
  left open; a socket with no sign-in id is left open.

Unit (`src/controllers/notifications.rs`, replacing
`the_stream_ends_when_its_user_is_signed_out`):

- `the_stream_forwards_only_its_own_orgs_notices`.
- `the_stream_ends_on_removal_from_its_org_only`.
- `the_stream_ends_when_the_account_is_signed_out`.

Unit (`src/controllers/chat_ws.rs`): the `SignedOut` cases in
`main_feed_sockets_take_only_main_feed_events` and
`thread_sockets_take_only_their_own_threads_events` become `Close`.

Model (`tests/models/memberships.rs`, `tests/models/users.rs`):

- `removal_keeps_the_session_version`.
- `removal_ends_an_unlock_for_that_org`.
- `removal_keeps_an_unlock_for_another_org`.
- `removal_revokes_tokens_in_that_org_only`: tokens and an unused code in
  Acme and Beta; remove from Beta; Acme's still work.
- `in_org_for_user_returns_any_status`: active, pending, removed, none.
- `end_unlock_in_clears_only_that_org`.
- `a_password_reset_revokes_tokens_in_every_org`.

Requests (`tests/requests/org_removal.rs`, new, in `tests/requests/mod.rs`).
Alice is in Acme and Beta (`alice_in_two_orgs`); Bob owns Beta and removes
her:

- `removal_from_beta_keeps_alice_signed_in_to_acme`: her old cookie still
  loads Acme pages.
- `after_removal_a_beta_cookie_falls_back_and_is_cleared`.
- `a_beta_tab_is_refused_with_the_gone_header`: an HTMX POST with
  `X-Org-Id: Beta` gets 409, `HX-Redirect: /dashboard?from=<beta>`,
  `X-Org-Gone: 1`, and saves nothing.
- `a_plain_beta_form_is_refused_after_removal`: 303 to Home, nothing saved.
- `the_home_notice_names_beta_after_removal`: "You no longer have access to
  Beta", no Back form, `no-store`.
- `the_home_notice_does_not_name_an_org_you_were_never_removed_from`: a
  `?from=` for a pending, declined, foreign or deleted org gives the nameless
  stale notice.
- `a_stale_tab_for_an_org_you_are_still_in_has_no_gone_header`.
- `a_beta_token_stops_and_an_acme_token_keeps_working`.
- `the_beta_allow_page_is_refused_after_removal`: the pinned `org_id` no
  longer matches.
- `beta_logins_cannot_be_revealed_after_removal`: unlocked in Beta, removed,
  then a reveal with the Beta claim is refused and the unlock columns are
  clear.
- `removal_from_your_only_org_shows_the_removed_page`: still signed in, 403,
  "You were removed from Beta".
- `htmx_with_no_active_org_is_sent_home`: 409, `HX-Redirect: /dashboard`,
  `X-Org-Gone: 1`, `no-store`.
- `removal_is_logged_in_beta_only`: one `member_removed` in Beta, nothing in
  Acme, no `sign_out_everywhere`.
- `removal_sends_no_email_and_no_notification`.

Requests, existing files:

- `tests/requests/remove_member.rs`, `removing_someone_locks_them_out_of_everything`:
  carol's next `GET /dashboard` is now a 403 removed page, not a 303 to
  `/login`. The hub event is `Close(Org { .. })`. The page copy is "carol has
  been removed." Her POSTs with `as_member` (which sends `X-Org-Id`) become
  409s instead of 403s; the "nothing saved" counts stay.
- `tests/requests/members.rs`: `the_remove_dialog_says_access_ends_in_this_org`.
- `tests/requests/auth.rs`:
  `a_password_reset_revokes_claude_tokens_in_every_org` and
  `the_password_changed_email_says_to_reconnect_claude`.
- `tests/requests/logins.rs`: `locking_emails_the_person_and_the_lock_orgs_owners_only`
  (Alice in Acme and Beta, both with another owner; the lock in Acme emails
  Acme's owner and Alice, not Beta's owner). Only with decision 4.
- `tests/requests/stale_tab.rs`: no edits expected. The route walks must
  still pass.

Live (`tests/requests/live_sign_out.rs`, real server):

- `removal_from_beta_closes_only_beta_sockets_and_streams`: Alice signs in
  twice. Browser 1 switches to Beta and opens Beta's `#general` socket and a
  stream. Browser 2 stays in Acme with Acme's `#general` socket and a stream.
  Bob removes Alice from Beta. Browser 1's socket and stream close. Browser 2's
  stay open, and a message posted in Acme still arrives on its socket.
- `removal_closes_the_beta_socket_without_a_session_change`: the user's
  `session_version` is unchanged after removal, and the Beta socket still
  closes. This pins that the close event alone does it.
- `switching_closes_only_this_browsers_sockets`: two sign-ins, both in Acme
  with sockets. Browser 1 switches. Its socket and stream close; browser 2's
  stay open.
- Existing `switching_closes_the_chat_socket_and_notification_stream` and
  `creating_an_org_closes_the_chat_socket_and_notification_stream` keep
  passing: same browser.
- Existing `ending_sessions_closes_the_chat_socket_and_the_notification_stream`
  keeps passing: `Account` scope.

**Not covered by `cargo test`:** the subscribe race (reasoned in "Edge
cases"), the removal banner and notice in the browser, the probe chain from a
closed socket to Home. Checked by hand.

## Out of scope

- **Leave an organisation.** Deferred, as in PR 4. It is adjacent, and this PR
  makes the core org-scoped, so a later "Leave" can call the same removal
  steps with the person as the remover. It still needs its own design: the
  last-owner rule (an owner can't leave without another owner, so it pairs
  with transferring ownership), a confirmation, where they land, and the
  `member_removed` log wording. Recommend a small PR after PR 6.
- **Transferring ownership, deleting an organisation.** Deferred.
- **An email or notification to the removed person.** None today; none here.
- **A notice on a fresh page load** after removal (see "Edge cases").
- **Naming the right org on the removed page** when the person has several
  non-active memberships.
- **"Create your own organisation"** on the removed page (PR 4's deferred
  list).
- **Unread dots per org** in the switcher.
- **PR 6, join with an existing account**, and **PR 7, the organisation in the
  address**.

## Risks

- **One safety net fewer.** Removal no longer ends sessions, so Beta access
  now rests on `active_in` in `CurrentMember` and `bearer.rs`, the org-scoped
  close events, `still_allowed`, the stream re-check and conversation
  membership. Every long-lived path is listed in the Summary table. A path
  that caches membership across requests and isn't in that table would keep
  serving Beta. Grepped: the only long-lived things are the chat socket and
  the SSE stream. Workers (`downloader`, `resend_email`) don't act for a
  member. Mention emails for Beta are cancelled in the transaction.
- **A future long-lived connection** (say, a second socket type) must handle
  `Close(Org)`. The `CloseScope` doc comment says so.
- **Event shape change.** Every match on `SignedOut` changes. The compiler
  finds them all. The `wants` and `events_for` unit tests cover the matching.
- **Per-browser close relies on the sign-in id.** Cookies issued before sign-in
  ids have none, and fall back to closing everything. That is today's
  behaviour, so it fails safe.
- **Pings keyed by org** could hide a ping someone relied on. The badge only
  counts the current org, so there was nothing to show.
- **Reset revoking tokens** disconnects Claude after every reset, including
  ones people do just because they forgot their password. The email says how
  to reconnect.
- **The 409 for HTMX with no active org** changes what HTMX gets on the
  waiting and declined paths too. No waiting, declined or removed page makes
  HTMX calls (checked: `assets/views/tenancy/` has no `hx-` attributes), so
  only tabs that lost their last org hit it.

## Rollback

No migration. Reverting the PR is safe for data:

- Removal goes back to signing the person out everywhere and closing every
  socket. More disruptive, but never less safe.
- Switches go back to closing every device's sockets.
- A password reset stops revoking Claude tokens.
- The lock stops emailing the person.

One rule: revert PR 5 before PR 6 ships, or with it. PR 6 may rely on removal
not ending sessions. The PR body says so.

## Checks

```sh
bin/build-assets
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

By hand, locally, with realistic data: Priya owns Acme Studio, Dev owns
Northwind Bakery, and Sam is a member of both. Two browsers for Sam (Chrome
and Safari).

- Chrome in Northwind with `#general` open; Safari in Acme with `#general`
  open. Dev removes Sam from Northwind. Chrome's tab moves to Home in Acme with the removal notice.
  Safari doesn't flicker, and live messages keep arriving.
- The same, with text typed in Chrome's composer: the removal banner shows,
  the text stays, "Go to Home" works.
- Remove Sam from his only org: open tabs move to the removed page; he is
  still signed in; Sign out works.
- Unlock logins in Northwind, get removed, try to reveal: refused.
- A switch in Chrome closes Chrome's socket only (watch Safari's network tab).
- Reset Sam's password: his Claude token stops working; the email says to
  reconnect.
- Lock Sam in Acme with 5 wrong app passwords: Acme's owner and Sam get
  emails; Northwind's owner doesn't.
- The remove dialog and the "removed" line on Change these at the source.

## Screenshots needed

From the local app with the data above. Store them on the `pr-screenshots`
branch under `multi-org-5/`. Desktop 1440px and phone 390px. Light and dark
for the new notice and banner; light for the copy changes:

- the Home removal notice;
- the removal banner over a composer with text;
- the removed page reached from an open tab (unchanged page, new route to it);
- the remove dialog with its new line;
- "Change these at the source" with "has been removed.";
- the password changed email and, with decision 4, the lock email to the
  person (text, from the dev mailer).

## Decisions for Raj

1. **Close events keyed by scope** (`Account`, `Org`, `Browser`), replacing
   `SignedOut`. Recommend: yes. One type, one pure matcher, unit tested.
2. **Narrower close on switch, deferred from PR 2.** Recommend: do it here,
   per browser by sign-in id, not per org. It is a few lines on top of
   decision 1, and a switch is the most common close by far. Per org would
   still close the person's other devices in the old org.
3. **Notification pings keyed by org.** Recommend: yes. The stream needs its
   org for removal anyway, and a ping from another org only reloads a badge
   that can't change.
4. **Who is told about an account lock.** Options: (a) the lock org's owners
   only, as today; (b) every active org's owners; (c) the lock org's owners
   and the person. Recommend (c). The lock is account-wide and disconnects
   Claude everywhere, so the person should hear it from us, especially if it
   wasn't them. Not (b): other orgs' logins were never at risk, they can do
   nothing about it, and the email would tell them the person is in another
   org.
5. **Fold "password reset revokes Claude tokens" into PR 5.** Recommend: yes.
   It is one call in the reset transaction plus one line of email copy. PR 6
   lets an existing account join by proving its email, which makes the reset
   path matter more, so it should land before PR 6 as planned.
6. **Leave an organisation.** Recommend: out of scope. Do it after PR 6 with
   ownership transfer, since an owner usually can't leave without one. This
   PR makes the core reusable.
7. **The removal notice and banner.** Recommend: name the org only when the
   person's membership there is `removed`, and offer no "Back". A forged
   `?from=` can then only name an org the person was removed from, which they
   already know.
8. **HTMX with no active org gets a 409 to Home** instead of a 403 it can't
   show. Recommend: yes. Otherwise a tab whose last org was removed just stops
   responding.
