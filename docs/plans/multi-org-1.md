# Multi-org 1: stop assuming one membership per user

Status: Reviewed (staff: approve with changes, applied; security: approve) · PR 1 of 7 · Owner: Raj · 19 October 2026

## Goal

People will own several organisations and belong to others with one email.
The code still assumes one membership per user. This PR removes that
assumption from the code. Nothing changes for anyone: no new screens, no
cookie, no switcher, no migration.

The database still has a unique index on `memberships.user_id`, so every real
user has exactly one membership. A later PR drops it.

## What changes

`memberships::Model::find_for_user` (returns one row, picked by Postgres) is
replaced by two functions:

- `for_user(db, user_id)`: every membership of the user, oldest first
  (`created_at`, then `id`).
- `default_for_user(db, user_id)`: the membership to use when no organisation
  has been chosen. Active memberships come first, then the oldest
  (`status = 'active'` descending, `created_at`, `id`). A person removed from
  their oldest org but active in another lands in the active one. With one
  membership per user this is the same row as today.

`all_by_user` (platform admin page) becomes `default_by_user`: one row per
user, picked the same way as `default_for_user`, so the admin page shows the
same as today.

## Callers

| File | Today | After |
|---|---|---|
| `src/extractors/current_member.rs` | the user's one membership picks the org | `default_for_user`; same pages and statuses |
| `src/extractors/bearer.rs` | the user's one membership, then checks it is the token's org | `active_in(token org, user)`: membership in the token's org, by org id |
| `src/controllers/auth.rs` sign out everywhere | logs the event in the user's org if active | logs it in every org where the user is active |
| `src/controllers/auth.rs` password reset | tells the user's org owners if active and the user sees logins | the same check for every org where the user is active; a mail failure in one org is logged and the others are still told |
| `src/controllers/admin.rs` | `all_by_user` | `default_by_user` |
| `src/tasks/super_admin.rs` | creates an org if the admin has no membership | `default_for_user`; same |

Tests that read "the user's membership" switch to `default_for_user`.

## Access checks

The bearer extractor is the one place that checks access to a specific
organisation. Today it loads "the" membership and compares its org with the
token's. With two memberships that could pick the wrong row and reject a valid
token, or (if the comparison were dropped) accept the wrong org. After this PR
it looks up the membership in the token's org directly. Everything else
already scopes by `member.org.id`.

## Tests

- Model: `for_user` returns all memberships oldest first, by `created_at`
  (the second membership is given an earlier date to prove it);
  `default_for_user` returns the oldest active one, else the oldest, `None`
  without one.
- Request: a user with memberships in two orgs. The test drops the unique
  index first (each test boots a freshly migrated database, so this does not
  leak). Checks: pages use the oldest active org, and the other org when
  removed from the oldest; a Claude token for the second org works there and
  sees that org's data, and stops working when that membership is removed or
  pending, while the first org is unaffected. Sign out everywhere logs the
  event in both orgs. A password reset tells the owners of both orgs.
- The full suite stays green.

## Risks

- Hidden single-membership assumptions outside `find_for_user`. Grepped every
  `memberships::` use; the rest is already scoped by org id.
- Removing someone from one org bumps their session version and closes their
  live connections, which signs them out of every org. Fine today; a later PR
  must decide if that is wanted.
- `join` creates a new user, so an existing email cannot join another org yet.
  Out of scope.

## Later PRs

1. This PR: stop assuming one membership per user in code.
2. Migration and `current_org` cookie.
3. Switcher UI.
4. Create another organisation.
5. Org-scoped removal.
6. Join with an existing account.
7. Organisation in the address (replaces "Links and the Claude connector").

**PR 7 replaced (Raj decided on 19 October).** Instead of keeping the current
organisation in a per-browser cookie, every page will carry it in its address,
for example `/o/acme-studio/tasks`. Each tab then holds its own organisation,
so two companies can be open side by side and a tab can no longer write into
another company by accident. Email, notification and Claude connector links
name their organisation by construction, which covers the old PR 7 scope. It is
a larger change (every route, link, form and redirect), so it runs as its own
project, starting with a brief, after PRs 5 and 6. The `current_org` cookie
then only remembers the last organisation for bare URLs like `/`.

**Order (Raj decided on 19 October).** Org-scoped removal (5) comes before
join with an existing account (6), as the reviewers recommended. Today removal
bumps the user's session version and closes all their live connections. Once
someone can be in two orgs, removal from one would sign them out of every org.

**PR 2: dropping `idx-memberships-user_id`.**

- In the same migration, add `UNIQUE (organisation_id, user_id)` and a plain
  index on `user_id`.
- Rolling back is only safe until a user has two memberships, that is before
  PR 4 (create another organisation) ships. PR 4 makes the first second
  membership in production. The PR must say so.

**PR 2: the `current_org` cookie.**

- The cookie is only a hint. Every request re-checks
  `active_in(cookie_org, user)`. On failure it falls back to the default
  active membership and clears the cookie.
- The cookie is HttpOnly, Secure and SameSite=Lax.
- While a platform admin is acting, `acting_org` wins and `current_org` is
  ignored. Both are cleared on switch and on logout.
- The pending, removed or declined page shows only when the user has no
  active membership.
- Sockets and SSE streams for the old org close, or are re-checked, on
  switch.

**PR 2: pin the org on the OAuth consent form.** Moved from PR 3, because
PR 2 adds the switch endpoint. Put a hidden org id in the form and check it
on POST in `authorize` in `src/controllers/oauth.rs`, so a switch in another
tab can't bind a Claude token to the wrong org. The personal token form is
pinned the same way. See `docs/plans/multi-org-2.md`.

**PR 3: stale-tab org check (hard gate before PR 4 ships).** A page left open
on one org can post into another after a switch, and a login, document,
message or token typed for Acme would land in Beta. PR 3 must add:

- `hx-headers` on `<body>` with the page's org id, and a submit listener in
  `frontend/app.ts` that adds a hidden `_org` field to plain urlencoded
  forms. Multipart forms are HTMX and carry the header; a no-JS multipart
  form fails closed.
- When the value differs from the resolved org, `CurrentMember` sends the
  tab to `/dashboard?from=<id>`: 409 with `HX-Redirect` for HTMX and
  `fetch`, 303 for plain forms. Home says what happened. This applies while
  acting and to HTMX GETs too. See `docs/plans/multi-org-3.md`.
- Once every form carries it, a missing value fails closed for non-GET
  requests.

**Join with an existing account.**

- The person proves the email is theirs, by password or an email link,
  before a membership attaches.
- The response is the same whether the email has an account or not.
- A removed or declined member can't re-join as active.
- The joiner sets their username for the new org.
- A rejoin updates the existing removed or declined row. It never inserts a
  second one; `(organisation_id, user_id)` is unique.
- Approving a membership revokes every token and unused OAuth code that
  `(org, user)` still has, and clears an unlock for that org, inside the
  approval transaction (from the PR 5 security review).
- `memberships::approve` becomes a conditional update (`WHERE status =
  'pending'`, exactly one row changed), so a stale page can't turn a declined
  or removed membership into active (from the PR 5 security review).
- Once re-approved, open sessions regain the org without signing in again,
  since removal no longer ends them (PR 5).

**Org-scoped removal (done, PR 5).**

- Revoke that org's tokens, and close only that org's sockets and streams.
  That needs events keyed by org and user.
- Rely on the per-request `active_in` check instead of bumping the session
  version.
- Decide who is told when an account lock (account-wide) is caused by one
  org's app passwords.
- Done in PR 5 (#46, merged 20 October 2026): removal closes only that org's
  sockets and streams (`CloseScope::Org`), relies on per-request `active_in`,
  and an account lock emails the person plus the lock org's owners. Plan:
  `docs/plans/multi-org-5.md`.
- Follow-up: a super admin "sign out everywhere and lock" for a user, from
  `/admin`, for suspected takeovers. Remove itself never signs people out.

**Known account-wide effects (accepted).**

- Sign out everywhere logs the IP and device in every active org.
- A password reset notifies every active org's owners, each with only their
  own org's details.

## Checks

`cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
No visible change, so no screenshots.
