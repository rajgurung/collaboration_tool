# Multi-org 6: join with an existing account

Status: Draft · PR 6 of 7 · Owner: Raj · 20 October 2026

## Summary

Today `/join/{slug}` always makes a new account
(`memberships::Model::join`, `src/models/memberships.rs:198`). Someone who
already has an account gets "An account with this email already exists. Sign
in instead." and has no way in. A signed-in visitor to `/join/{slug}` is sent
to `/dashboard` (`src/controllers/join.rs:23`). So a person can be in two
organisations only by creating the second one (PR 4).

After this PR, a person with an account can ask to join another
organisation, and a removed or declined person can ask again.

**How it works.**

- **Signed in.** `/join/beta` shows a short form: "Join Beta as you", with
  one field, the username to use in Beta. Posting it makes a pending
  membership for the signed-in account. Nothing else changes for them: they
  stay in the org they were in, and Beta shows in the switcher as "Waiting
  for approval".
- **Signed out, with an account.** The join page's "Already have an account?
  Sign in to join" goes to `/login?next=/join/beta`. That is the normal
  sign-in form, with its rate limits. After signing in they land back on
  `/join/beta`, signed in, and see the form above.
- **Signed out, new to the app.** The new-account form, as today.
- **Approval.** An owner or admin approves as today. Approval is now one
  conditional update, and in the same transaction it revokes any Claude
  tokens and unused codes the person still has in that org, ends a logins
  unlock for that org, re-adds them to `#general` and to their old direct
  messages, and clears the logins that were shared with them by name. After
  the commit the person gets an email: they're in.
- **Open sessions** gain the org on their next request. Nobody signs in
  again.

**Proof of email.** Being signed in is the proof. Nothing attaches a
membership to an account except a request made from that account's own
session. The new-account form never attaches to an existing account.

**No migration.** No new crate. Twelve decisions wait on Raj (see the end).

### Earlier constraints, and where this plan meets them

From `docs/plans/multi-org-1.md` ("Join with an existing account") and the
PR 5 security review:

| Constraint | Where |
|---|---|
| The person proves the email is theirs before a membership attaches | Signed-in session only; sign-in reuses `/login` (see "Proof of email") |
| The response is the same whether the email has an account or not | The new paths ask for no email outside `/login`, which already answers the same. The old new-account form still says the email exists; see decision 2 |
| A removed or declined member can't rejoin as active | `request_to_join` only ever writes `pending`. Only `approve` writes `active`, and only from `pending` |
| The joiner sets their username for the new org | The signed-in form's one field, checked in that org |
| A rejoin updates the existing row, never inserts a second | `request_to_join` locks and updates the row; `(organisation_id, user_id)` is unique anyway |
| Approval revokes every token and unused code `(org, user)` still has, and clears that org's unlock, inside the approval transaction | `approve` calls `access_tokens::Model::revoke_all_for` and `users::Model::end_unlock_in` in its transaction |
| `approve` is a conditional update on `status = 'pending'` that changes exactly one row | `approve` (below). `reject` gets the same treatment |
| Open sessions regain the org once approved | No session change on approval; `CurrentMember`, the switcher and `switch` check `active_in` per request (see "Open sessions") |

**PR 5 rules, re-checked.**

- Removal no longer ends sessions, so an approved rejoiner gets the org back
  without signing in. Tested.
- Revoked tokens stay revoked. Nothing in this PR un-revokes a token.
  Approval revokes again, as a second net.
- The unlock for the org was cleared at removal. Approval clears it again.
- Org-scoped close events: a pending person has no Beta sockets or streams
  (`CurrentMember` refuses Beta), so approval has nothing to close.
- Minting under `FOR SHARE`: a pending person can't mint for Beta
  (`active_in_for_share` refuses). Approval's conditional update takes the
  row lock, so a mint that waits on it sees the committed status.
- Lock order stays membership row, then user row, then tokens, as in
  `remove` (`src/models/memberships.rs:417`).

**Done in PR 5, not here.** "A password reset revokes Claude tokens", deferred
in PR 4 as "follow-up before PR 6", shipped in PR 5
(`users::Model::reset_password_everywhere` calls `revoke_all_for_user`). PR 6
relies on it: a person who forgot their password resets it, which also cuts
off any Claude connection, before signing in to join.

**This PR makes "removed, then rejoin" real.** PR 5 wrote the rules; until
now there was no route from `removed` or `rejected` back to `pending`
(`docs/flows/accounts.md:199`).

## Proof of email

**Recommended: sign in with the password first** (decision 1).

- It reuses `POST /login` as it is: the attempt is recorded before the
  password check, 5 failures per email or 20 per IP in 15 minutes lock it
  (`auth_attempts::begin_login`), an unknown email runs the dummy hash, and
  a locked account is refused.
- The only change to sign-in is that `safe_next` (`src/controllers/auth.rs:57`)
  also accepts `/join/<slug>`, where the slug is 1 to 60 characters of
  `a-z`, `0-9` and `-`, and nothing follows it. No query, no second slash,
  no encoded characters.
- No new tokens, columns or emails, so no migration and no new way to make
  the app email a stranger.

**Not recommended: an emailed link.** The person types an email on the join
page, gets a one-time link, and the link attaches the membership. It needs
a token column or table (a migration), a new rate limit for sending, and a
new email that anyone can aim at any inbox. Its one gain is a person who
forgot their password; they can use "Forgot password" and then sign in.

## No enumeration

**Signed-out path.** The existing-account path never asks for an email
outside `/login`. `/login` already gives "Email or password is incorrect."
with a 422 for an unknown email and for a wrong password, takes the same
time (dummy hash), and rate limits both the same way. Adding `next` changes
none of that. A test compares the two responses.

**Signed-in path.** It has no email field. The account is the session's
own. What the page says (already a member, already waiting, or the form) is
about the signed-in person's own membership. Nothing on it depends on any
other account.

**The new-account form, still.** It answers an existing email with "An
account with this email already exists." `/signup` says the same today. So
the app as a whole still reveals whether an email has an account. Hiding it
on `/join` alone protects nothing while `/signup` reveals it. The only way
to give one identical answer on both is email verification ("Check your
email" whatever was typed), which changes how signup and join work for every
new person. Decision 2 recommends keeping the message, now pointing at "Sign
in to join", and deferring verification for both forms to its own PR.

## What changes, file by file

### Join

| File | Change |
|---|---|
| `src/models/memberships.rs` | New `JoinParams { name }`, validated like `RegisterParams.name` (`USERNAME_RE`, `not_reserved`). New `pub enum JoinOutcome { Requested(Model), AlreadyPending(Model), AlreadyMember(Model) }`. New `request_to_join(db, org, user, params) -> ModelResult<JoinOutcome>`, below. The old `join` is renamed `join_new_account`; its body is unchanged. |
| `src/models/memberships.rs` | New `MAX_PENDING_REQUESTS: u64 = 5`. |
| `src/models/memberships.rs` | `active_first()` becomes `status_rank()`: `CASE status WHEN 'active' THEN 0 WHEN 'pending' THEN 1 ELSE 2 END`, sorted ascending. Used by `default_for_user`, `default_by_user` and `for_user_with_orgs`. A person with no active org now sees the waiting page for a pending request before an older removed or declined page (decision 10). |
| `src/controllers/join.rs` | `new` takes `Option<CurrentUser>`. Signed out: the page as today, plus "Sign in to join" linking to `/login?next=/join/{slug}`. Signed in: looks up `in_org_for_user(org.id, user.id)` and renders `join/existing.html` in one of three states: the form (no row, `removed`, `rejected`), "already waiting" (`pending`), or "already a member" with a Switch button (`active`; the button posts to `/orgs/{id}/switch`, which already takes `CurrentUser`). The form's username starts as the person's username in their default org, which they can change. `Cache-Control: no-store`. |
| `src/controllers/join.rs` | New `POST /join/{slug}/request`, `request`: `CurrentUser` (a signed-out post gets the usual 303 to `/login`). Calls `request_to_join`. `Requested` and `AlreadyPending`: 303 to `/dashboard?requested={org.id}`. `AlreadyMember`: 303 to `/join/{slug}`, which shows "already a member". Field errors and the pending cap: 422, the form redrawn. Logs `user_id` and `org_id` only, "requested to join". |
| `src/controllers/join.rs` | `create` (the new-account form) is unchanged except the existing-email message, which becomes "An account with this email already exists. Sign in to join {org}." with the sign-in link (decision 2). It never touches an existing account. |
| `src/controllers/auth.rs` | `safe_next` also accepts `/join/<slug>` (above). Doc comment updated. |
| `src/controllers/dashboard.rs` | `requested_notice`: when `?requested={id}` is set and `in_org_for_user(id, me)` is `pending`, Home shows "Request sent to {org}". Any other status, a foreign or unknown id: no notice. Same pattern as `stale_notice`, so a forged `?requested=` can only name an org the person really asked to join. With no active org, `/dashboard` shows the waiting page and the parameter is ignored. |
| `assets/views/join/new.html` | The "Sign in to join" link (mockup "Signed-out join page"). |
| `assets/views/join/existing.html` (new) | The signed-in states (mockup "Signed-in join", "Already a member", "Already waiting"). Uses `layouts/auth.html` and passes its own `org`, never `current_org_id`, so the page carries no org claim and `app.ts` adds no `_org` field. Shows which account is joining, with "Not you? Sign out". |
| `assets/views/dashboard/index.html` | The "Request sent" notice (mockup "Request sent"). |
| `assets/views/auth/login.html` | When `next` is a join path, a line naming the org being joined (mockup "Sign in to join"). The org name comes from the slug looked up on the server, never from the query text. |
| `src/controllers/auth.rs` `login_page` | Looks up the org for a join `next` to show that line. An unknown slug shows the plain page. |

**`request_to_join`, in one transaction:**

1. `ValidatorTrait::validate(params)`.
2. Lock the existing `(org, user)` row, if any, `FOR UPDATE`. Membership row
   first, then the user row, the same order `remove` and `approve` use.
3. `active`: return `AlreadyMember`. `pending`: return `AlreadyPending`.
   Nothing is written in either case, the username included.
4. Lock the user row `FOR NO KEY UPDATE` (as `create_for_user` does), then
   count the user's `pending` rows. At `MAX_PENDING_REQUESTS`: a form error,
   "You have 5 requests waiting. Wait for one to be answered first." (copy
   from the mockup). Two requests at once run one after the other.
5. Username check: case-insensitive, in this org, excluding this person's
   own row. Their old username in Beta is theirs to keep. Taken: "That
   username is already used in this organisation." on `name`.
6. **Rejoin** (row is `removed` or `rejected`): an `UPDATE ... WHERE id = ?
   AND status IN ('removed', 'rejected')` that sets `status = 'pending'`,
   `role = 'member'`, the new `username`, `approved_by_id = NULL`,
   `approved_at = NULL`, `updated_at`. It keeps `removed_at` and
   `removed_by_id` so owners can see the history. Exactly one row must
   change.
   **New:** insert a pending `member` row, as `join_new_account` does.
7. Commit, return `Requested`.

A unique violation on `idx-memberships-org-user` (two first requests at
once) is read back as `AlreadyPending`. One on `idx-memberships-org-username`
(two people taking the same name at once) becomes the username field error.

**The org cap doesn't apply.** `MAX_ORGS_CREATED` counts organisations the
person created (`organisations.created_by_id`). Joining creates none.
`request_to_join` never reads it. Tested.

### Approve and decline

| File | Change |
|---|---|
| `src/models/memberships.rs` | `approve(db, org_id, approver_id, id) -> ModelResult<Self>`, replacing `approve(self, ...)`. Below. |
| `src/models/memberships.rs` | `reject(db, org_id, approver_id, id)`: the same conditional update on `status = 'pending'`, setting `status = 'rejected'` and `approved_by_id`. Today it checks a row loaded earlier and then writes unconditionally, so a stale page could turn a just-approved member into declined without revoking anything (decision 11). |
| `src/models/conversation_members.rs` | `add_if_missing(db, conversation, user_id)`: `INSERT ... ON CONFLICT DO NOTHING`. Approve uses it, so a row left over from bad data can't turn approval into a 500. |
| `src/models/conversations.rs` | `dms_of(db, org_id, user_id)`: the org's DMs whose `dm_key` is `dm:{id}:...` or `dm:...:{id}`. |
| `src/models/login_accesses.rs` | `remove_for_member(db, org_id, user_id)`: deletes the rows naming this user on that org's logins. |
| `src/controllers/members.rs` | `approve` and `reject` call the new functions with `member.org.id` and `id`. `approve` then sends the approval email (below). Error toasts as today. |
| `src/mailers/membership.rs` (new, in `src/mailers/mod.rs`) | `MembershipMailer::approved(ctx, user, org)`. Templates in `src/mailers/membership/approved/` (`subject.t`, `text.t`, `html.t`). Copy from the mockup ("Approval email"). It names only Beta. It doesn't name the approver, so an acting super admin isn't named either. Until PR 7, links can't pick an org, so it links to `{app_url}/dashboard` and says to choose Beta from the organisation menu. Every `html.t` variable uses `| escape`. |

**`approve`, in one transaction:**

1. `UPDATE memberships SET status = 'active', role = 'member',
   approved_by_id, approved_at = now(), removed_at = NULL, removed_by_id =
   NULL, updated_at WHERE id = ? AND organisation_id = ? AND status =
   'pending'`. Exactly one row must change. Zero: "Only pending requests can
   be approved." and nothing else runs. `role = 'member'` is a second net:
   rejoin already set it.
2. Refuse if the row's `user_id` is the approver: "You can't approve your own
   request." Only an acting super admin can reach this (decision 9).
3. Lock the user row `FOR NO KEY UPDATE`, as `remove` does.
4. `users::Model::end_unlock_in(&txn, user_id, org_id)`.
5. `access_tokens::Model::revoke_all_for(&txn, org_id, user_id)`.
6. `login_accesses::Model::remove_for_member(&txn, org_id, user_id)`
   (decision 7).
7. Add them to `#general` with `add_if_missing`. Today's `approve` already
   does this (`src/models/memberships.rs:341`); a rejoiner lost it at removal.
8. Add them back to each of their DMs in this org (`dms_of`) with
   `add_if_missing` (decision 6). Groups are not restored, as for a first
   join; members add them back.
9. Commit. Log `org_id`, `user_id`, `approver_id`, "approved member".

The controller then sends `MembershipMailer::approved`. A failure is logged,
not shown: the approval stands.

**Why steps 4 to 6 on every approval, not only rejoins.** For a first join
they find nothing and cost three indexed statements. One path means no
"is this a rejoin" branch to get wrong.

### Owners' waiting list

| File | Change |
|---|---|
| `src/controllers/members.rs` | `MemberRow` gains `history: Option<String>` for pending rows only: "Removed on 3 Oct 2026 by priya" when `removed_at` is set, "Asked before and was declined" when the row was declined and asked again (its `updated_at` is later than `created_at` and `removed_at` is unset). Built from this org's row only. |
| `assets/views/members/_pending_row.html` | Shows `history` under the email (mockup "Owner's waiting list"). |

**What owners see, and don't.** The pending row shows the username the
person chose for this org, their email (as today), and that history line.
It doesn't show `users.name`, whether the account is new or old, other
organisations, or other usernames. The email is the account's sign-in email,
the same in every org. The person chose to share it by asking.

### Open sessions, the switcher and `enter_org`

Nothing to change. Checked in the code:

- `CurrentMember` picks the org per request: `chosen_org` calls `active_in`
  for the `current_org` cookie, and `default_for_user` reads the rows fresh.
  Nothing caches a membership across requests.
- `switch` (`src/controllers/orgs.rs:70`) checks `active_in(id, user)` per
  request. So does `enter_org` after it. A just-approved org can be switched
  to straight away.
- The switcher list (`GET /orgs`) is `no-store` and read fresh. `app.ts`
  loads it once per page (`loadOrgs`, `frontend/app.ts:1843`), so an open
  menu shows the pending row until the next page load. Accepted.
- `org_menu` is true for every non-acting member, so the switcher is there.
- A person whose only membership is the new one is on the waiting page.
  "Check again" loads `/dashboard`, which now finds an active org.

### Docs

| File | Change |
|---|---|
| `docs/flows/accounts.md` | "Join by link": three paths (new account, sign in to join, signed in), a new sequence diagram for the signed-in path and for approval. "Membership states": `removed --> pending` and `rejected --> pending` by asking again; remove "There is no route that moves rejected or removed back". The default-org order (active, pending, other). "Sign in": `next` also takes `/join/<slug>`. |
| `docs/architecture/erd.md` | Statuses: rejoin goes back to `pending` on the same row; approval clears `removed_at` and `removed_by_id`. The multi-org paragraph names PR 5 and PR 6. |
| `docs/flows/notifications.md:79` | Approvals now send an email to the person. Join requests, removals and role changes still send none. |
| `docs/flows/super-admin.md:95` | An acting super admin can't approve their own request. |
| `docs/flows/documents.md` | Approval clears logins shared by name with that person in that org. |
| `docs/plans/multi-org-1.md` | Mark the "Join with an existing account" constraints as planned here, and the PR 5 ones as done. |

## Edge cases

- **Already a member.** The join page says so with a Switch button. A post
  is a no-op and goes back there.
- **Already waiting.** The page says so. A post changes nothing, the
  username included, and lands on Home with the request notice.
- **Removed, asks again.** Same row, now `pending`, `role = member`. They
  can't open Beta. Once approved, everything above applies. Their old Claude
  tokens stay revoked.
- **Declined, asks again.** Same. Their row was never active, so there is
  nothing to revoke, but approval runs the same steps.
- **Asked again by a former owner or admin.** Comes back as a member. An
  owner can change their role after approval as today.
- **"Change these at the source" after a rejoin.** That page needs
  `status = 'removed'` (`src/controllers/members.rs:308`), so it ends once the
  person asks again. The remover was sent there at removal time. Accepted
  (decision 8).
- **No active org, removed from Acme, asks to join Beta.** The waiting page
  for Beta, not "removed from Acme" (`status_rank`).
- **Active in Acme, asks to join Beta.** Stays in Acme. Home shows the
  request notice. The switcher shows Beta greyed, "Waiting for approval".
- **Approved while the person is in Acme.** No email link can switch them
  (PR 7). The email says to choose Beta from the menu. Their next page load
  shows Beta active in the switcher.
- **A stale Members page approves a removed member.** The conditional update
  changes nothing. Error toast "Only pending requests can be approved."
  Tested.
- **Two owners approve at once.** One update changes the row. The other
  sees zero rows and gets the toast. One email.
- **Approve and decline at once.** Whichever commits first wins; the other
  gets its toast.
- **A double submit of the join form.** The row lock, then `AlreadyPending`.
  For a first request, the unique index, read back the same way.
- **Two people take the same username in Beta at once.** The unique index on
  `(organisation_id, lower(username))`, shown as the field error.
- **A removed member's old username.** Their removed row holds it, so nobody
  else could take it. They may keep it or pick another.
- **Pending cap.** Five open requests. A decline frees a slot. An owner who
  ignores a request leaves it pending, which keeps a slot used.
- **Signed in as someone else on a shared browser.** The page names the
  joining account, with "Not you? Sign out".
- **A locked account.** Its sessions ended when it locked, and `/login`
  refuses it. It can't join until it resets its password.
- **The super admin.** Can join an org like anyone. While acting in Beta, can
  approve others there (allowed today) but not their own request. Joining
  doesn't end acting.
- **The super admin acting in Beta while pending in Beta.** They already see
  Beta through acting. Approving themselves is refused; another owner or
  admin must approve.
- **Old DM history after rejoin.** Approval puts the person back in their
  DMs, so they see the messages from before (decision 6).
- **Old notifications.** Their Beta notifications from before the removal
  are still in Beta's bell. Unsent mention emails were cancelled at removal.
  Accepted.
- **The default org after a rejoin.** The row keeps its old `created_at`.
  Once approved it may be their oldest active org, so a fresh sign-in starts
  there. Accepted; the switcher is one click.
- **The join link is guessable.** It is the slug, from the org name
  (`/join/acme-studio`). Knowing it lets someone file a request and learn the
  org's name. Every request needs an owner or admin to approve. Requests
  email nobody and notify nobody. With this PR an account can file at most
  5 at once. Unguessable join links stay deferred (PR 4's list).
- **A stranger attaching a request to someone else's account.** Only a
  session for that account can call `request_to_join`. The new-account form
  never attaches to an existing account; with an existing email it creates
  nothing. A test checks that account and its memberships are unchanged.
- **One person spamming many orgs.** Five pending requests per account,
  under the user-row lock. More needs more accounts. Accounts are free to
  make (no `/signup` limit, deferred in PR 4), so the real bound is that
  requests email and notify nobody.
- **CSRF on the signed-in post.** The origin check
  (`src/initializers/origin_check.rs`) and `SameSite=Lax` cookies, as for
  every other POST.
- **The stale-tab check.** `POST /join/{slug}/request` takes `CurrentUser`,
  so the claim check never runs, and the join page carries no claim. Added to
  the route walk's exempt list next to `POST /join/{slug}`.

## Tests

Unit (`src/controllers/auth.rs`): `next_only_returns_to_the_allow_access_page`
becomes `next_only_returns_to_allow_access_or_a_join_page`. Accepts
`/join/acme-studio`. Refuses `/join/`, `/join/Acme`, `/join/acme?x=1`,
`/join/acme/request`, `/join/../admin`, `/join/acme%2F..`, `//join/acme` and
a 61-character slug.

Model (`tests/models/memberships.rs`):

- `request_to_join_adds_a_pending_member_row`.
- `request_to_join_when_pending_changes_nothing`: the username stays.
- `request_to_join_when_active_says_already_a_member`.
- `rejoin_after_removal_updates_the_same_row`: one row before and after, now
  `pending`, `role = member`, new username, `removed_at` kept,
  `approved_by_id` cleared.
- `rejoin_after_decline_updates_the_same_row`.
- `a_former_admin_rejoins_as_a_member`.
- `request_to_join_never_writes_active`: for every starting status.
- `request_to_join_checks_the_username_in_that_org_only`: taken by someone
  else in Beta is refused; the same name in Acme is fine; your own old Beta
  name is fine.
- `request_to_join_refuses_a_bad_or_reserved_username`.
- `request_to_join_stops_at_five_pending_requests`.
- `two_requests_at_once_do_not_pass_the_pending_cap`: at 4, two run with
  `tokio::join!`, one on `ctx.db` and one on `second_connection(&ctx)`.
- `the_org_cap_does_not_apply_to_joining`: a person who created 10 orgs can
  still ask to join.
- `approve_changes_only_a_pending_row`: on `active`, `removed` and
  `rejected` it errors and the row is unchanged.
- `two_approves_at_once_change_one_row`.
- `approve_revokes_tokens_and_codes_in_that_org_only`: unrevoked tokens and
  an unused code put in Beta and Acme by SQL; after approval Beta's are
  revoked and used, Acme's still work.
- `approve_ends_an_unlock_for_that_org_only`.
- `approve_clears_named_login_access_in_that_org_only`.
- `approve_adds_general_once`: also when the row is already there.
- `approve_restores_direct_messages_in_that_org`.
- `approve_clears_removal_and_sets_member`.
- `nobody_approves_their_own_request`.
- `reject_changes_only_a_pending_row`.
- `default_for_user_prefers_pending_over_removed`.

Requests (`tests/requests/join_existing.rs`, new, in `tests/requests/mod.rs`).
Bob owns Beta. Alice owns Acme:

- `signed_in_join_page_asks_only_for_a_username`: no email or password
  field, names Alice's account, `no-store`.
- `signed_in_join_page_says_already_a_member`: with the Switch form.
- `signed_in_join_page_says_already_waiting`.
- `joining_while_signed_in_keeps_the_current_org`: after the post, Alice's
  Acme pages and an HTMX post with `X-Org-Id: Acme` still work.
- `joining_lands_on_home_with_the_request_notice`.
- `the_request_notice_names_only_an_org_you_asked_to_join`: `?requested=`
  for an org where she is active, removed, declined or absent, or that
  doesn't exist, shows no notice.
- `a_taken_username_redraws_the_form`: 422, nothing written.
- `joining_needs_a_session`: a post with no cookie is a 303 to `/login`, no
  row.
- `the_switcher_shows_the_request_as_waiting`.
- `after_approval_alice_switches_to_beta_without_signing_in`: the same
  cookie, `POST /orgs/{beta}/switch` is a 303, then Beta pages load.
- `after_approval_the_waiting_page_opens_the_org`: Carol's only membership
  is the request; after approval `/dashboard` is Beta's Home.
- `rejoin_after_removal_waits_for_approval`: Bob removes Alice, she asks
  again, Beta pages are still refused, the switcher shows Beta waiting.
- `rejoin_after_decline_waits_for_approval`.
- `a_rejoined_member_has_no_old_claude_tokens`: her Beta token from before
  the removal still gets a 401 after approval.
- `sign_in_to_join_returns_to_the_join_page`: `POST /login` with
  `next=/join/beta` is a 303 to `/join/beta`.
- `sign_in_to_join_says_the_same_for_unknown_email_and_wrong_password`: same
  status and body.
- `sign_in_to_join_keeps_the_sign_in_limits`: the 6th failure for an email
  is a 429 with `next` set too.
- `the_new_account_form_never_attaches_to_an_existing_account`: Alice's
  email on Beta's new-account form gives 422, no Beta row, Alice's password
  and memberships unchanged.
- `approval_emails_the_person`: one email to Alice naming Beta, not Acme,
  and not the approver.
- `a_join_request_emails_and_notifies_nobody`.
- `an_acting_super_admin_approves_others_but_not_themselves`.
- `a_stale_members_page_cannot_approve_a_removed_member`: the toast, the row
  stays `removed`, no email.
- `the_waiting_list_shows_only_this_orgs_details`: the pending row has the
  Beta username, email and history line; not `users.name`, not "Acme".

Existing files:

- `tests/requests/join.rs`: `signed_in_users_skip_the_join_and_signup_forms`
  becomes `signed_in_users_skip_signup_but_see_join`: `/join/acme` is 200
  with the signed-in form; `/signup` and `/` still redirect. The
  existing-email case checks the new message and the sign-in link.
- `tests/requests/members.rs`: approve and decline tests call the new
  signatures; toasts unchanged.
- `tests/requests/stale_tab.rs`: `POST /join/{slug}/request` joins the
  exempt list.
- `tests/requests/multi_org.rs`: `alice_in_two_orgs` keeps inserting by SQL
  (faster); its comment now points at `request_to_join`.

**Not covered by `cargo test`:** the email's look in a real client, the
switcher menu staying stale until reload, and the screens against the
mockup. Checked by hand.

## Out of scope

- **The organisation in the address** (`/o/acme-studio/tasks`), the project
  that replaced PR 7. It runs as its own project with a brief. Until then
  the approval email can't link straight into Beta.
- **Email verification** on signup and join, which is the only way to stop
  the new-account forms revealing that an email has an account (decision 2).
- **A notice to owners about new requests** (decision 5).
- **Unguessable join links**, a `/signup` rate limit, and leaving an
  organisation, all deferred in PR 4 and PR 5.
- **A "Ask to join again" button** on the removed and declined pages
  (decision 12).
- **Restoring group conversations** after a rejoin.
- **The super admin "sign out everywhere and lock"** follow-up from PR 5.

## Risks

- **A wider `next`.** Any loosening of `safe_next` is an open-redirect risk.
  The pattern is exact and the unit test lists the tricks. The target is
  always a same-site path.
- **Approval now writes more.** It locks the user row and touches tokens,
  unlock columns, login access and conversations. The lock order matches
  `remove`, so they wait rather than deadlock. All of it is one short
  transaction.
- **DM history comes back.** A person removed for cause sees their old DMs
  once approved. The owner chose to approve. Decision 6 names it.
- **Clearing named login access** surprises a teammate who expected their
  share to survive. It is only on approval after a removal or decline, and
  the share is one click to redo. Decision 7.
- **More pending rows per person.** The switcher shows up to 5 greyed rows.
  Bounded by the cap.
- **Email delivery.** If the approval email fails, the person finds out on
  their next visit, as today.
- **The default-org order change** also moves `default_by_user` on `/admin`.
  Same rows, possibly a different one picked for a person with no active org.
  Harmless.

## Rollback

No migration. Reverting is safe for data:

- Rejoined rows stay `pending`. The old `approve` can still approve them. It
  doesn't revoke tokens or clear the unlock, but removal already did both.
  Rejoin set `role = member`, so nobody comes back with their old role.
- Approved rejoiners keep `removed_at` cleared. Rows approved by the old code
  after a revert keep `removed_at`, which nothing reads for active rows.
- People with several pending requests keep them.
- The join page goes back to sending signed-in people to `/dashboard`.

Revert PR 6 before PR 5, never PR 5 alone: PR 6 relies on removal not ending
sessions.

## Checks

```sh
bin/build-assets
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

By hand, locally, with realistic data: Priya owns Acme Studio, Dev owns
Northwind Bakery. Sam is in Acme only.

- Signed out, Sam opens `/join/northwind-bakery`, picks "Sign in to join",
  signs in, lands on the join form, picks a username, and is back in Acme
  with the notice. The switcher shows Northwind waiting.
- Dev approves. Sam gets the email (Mailpit). Sam's open Acme tab: open the
  switcher after a reload, Northwind is active, switching works without
  signing in.
- Dev removes Sam. Sam opens the join link again: the form. He asks again.
  Dev's Members page shows "Removed on ... by dev". Approve. Sam's old DM
  with Dev is back with its history; `#general` is back; his old Claude
  token still fails.
- Decline, then ask again: the history line says so.
- A sixth request: the error.
- Wrong password on "Sign in to join", and an unknown email: the same
  message.
- Sam's email on the new-account form: the "Sign in to join" message, and
  nothing changes on his account.

## Screenshots needed

From the local app with the data above, on the `pr-screenshots` branch under
`multi-org-6/`. Desktop 1440px and phone 390px, light and dark for every new
screen:

- the signed-out join page with "Sign in to join";
- the sign-in page with the join line;
- the signed-in join form, and with a username error;
- "already a member" and "already waiting";
- Home with the "Request sent" notice;
- the switcher with a waiting row (menu and phone sheet);
- the owner's waiting list with a rejoin history line;
- the pending-cap error;
- the approval email (text and HTML, from Mailpit).

## Decisions for Raj

Mockup sections are named as asked of the designer for
`docs/designs/join-existing.html`. Match them when it lands. Visuals and copy
come from the mockup; quoted copy here is a placeholder.

1. **Proof of email.** Recommended: sign in with the password first, through
   `/login?next=/join/<slug>`. No migration, no new email anyone can trigger,
   and the sign-in limits apply as they are. Not an emailed link.
2. **The new-account form still says an email exists.** Options: (a) keep
   the message, now "Sign in to join {org}", as `/signup` does; (b) answer
   "Check your email" whatever was typed, and email either a "you already
   have an account, sign in to join" note or a welcome. Recommended: (a) now,
   and a later PR adds email verification to signup and join together. (b)
   on `/join` alone protects nothing while `/signup` still answers, delays
   every new joiner, and adds an email anyone can send to any inbox. The
   security reviewer should weigh in.
3. **A cap of 5 open requests per account.** Recommended: yes, under the
   user-row lock. No daily limit: it would need a request timestamp
   (a migration), and a declined slot only frees when an owner acts.
4. **An email to the person when approved**, for every approval, first-time
   joiners too. Recommended: yes, email only. An in-app notice would sit in
   Beta's bell, which they can't see until they switch.
5. **Telling owners about requests.** Recommended: nothing new. Requests
   still email and notify nobody; More shows the count. Anything else turns
   the guessable join link into a way to ping owners.
6. **Restore direct messages on approval**, with their history. Recommended:
   yes. Otherwise `start_dm` finds the old DM (`dm_key`) and the rejoiner
   isn't in it, so the other person writes into a DM the rejoiner can't see.
   The alternative is a new DM key scheme, which is more work for a rare case.
7. **Clear logins shared by name on approval.** Recommended: yes. Owners were
   told to change those passwords at removal. Silently giving the person the
   new ones back would undo that. Shares are one click to redo.
8. **Rejoin resets the role to member at once.** Recommended: yes, so a
   pending row never carries an old owner or admin role, and a revert stays
   safe. The cost: "Change these at the source" ends when the person asks
   again. The alternative keeps the old role until approval so that page
   survives, at the price of a pending row holding an elevated role.
9. **Nobody approves their own request.** Recommended: yes. Only an acting
   super admin could, and acting is for helping, not for joining.
10. **The default org order: active, then pending, then removed or
    declined.** Recommended: yes. Someone removed from their only org who
    asks to join another should see that request, not the old removal.
11. **Decline becomes a conditional update too.** Recommended: yes. Same
    race as approve, same fix, a few lines.
12. **No "Ask again" button on the removed or declined pages.** Recommended:
    none. A person who should come back gets the link from someone in the
    org. A button would invite repeat requests from people who were removed
    on purpose.
