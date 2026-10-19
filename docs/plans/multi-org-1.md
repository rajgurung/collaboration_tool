# Multi-org 1: stop assuming one membership per user

Status: Draft · PR 1 of 7 · Owner: Raj · 19 October 2026

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
  has been chosen. For now, the oldest. With one membership per user this is
  the same row as today.

`all_by_user` (platform admin page) becomes `default_by_user`: one row per
user, the oldest membership, so the admin page shows the same as today.

## Callers

| File | Today | After |
|---|---|---|
| `src/extractors/current_member.rs` | the user's one membership picks the org | `default_for_user`; same pages and statuses |
| `src/extractors/bearer.rs` | the user's one membership, then checks it is the token's org | `active_in(token org, user)`: membership in the token's org, by org id |
| `src/controllers/auth.rs` sign out everywhere | logs the event in the user's org if active | logs it in every org where the user is active |
| `src/controllers/auth.rs` password reset | tells the user's org owners if active and the user sees logins | the same check for every org where the user is active |
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

- Model: `for_user` returns all memberships oldest first; `default_for_user`
  returns the oldest, `None` without one.
- Request: a user with memberships in two orgs. The test drops the unique
  index first (each test boots a freshly migrated database, so this does not
  leak). Checks: pages use the oldest org; a Claude token for the second org
  works there and sees that org's data, and stops working when that
  membership is removed, while the first org is unaffected. Sign out
  everywhere logs the event in both orgs.
- The full suite stays green.

## Risks

- Hidden single-membership assumptions outside `find_for_user`. Grepped every
  `memberships::` use; the rest is already scoped by org id.
- Removing someone from one org bumps their session version and closes their
  live connections, which signs them out of every org. Fine today; a later PR
  must decide if that is wanted.
- `join` creates a new user, so an existing email cannot join another org yet.
  Out of scope.

## Checks

`cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
No visible change, so no screenshots.
