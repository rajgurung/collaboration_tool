# Multi-org 6: join with an existing account

Status: Approved by Raj 20 Oct (option A); reviews folded in (staff: approve with changes; security: approve with changes) · PR 6 of 7 · Owner: Raj · 20 October 2026

## Summary

Today `/join/{slug}` always makes a new account
(`memberships::Model::join`, `src/models/memberships.rs:198`). Someone who
already has an account gets "An account with this email already exists. Sign
in instead." and has no way in. A signed-in visitor to `/join/{slug}` is sent
to `/dashboard` (`src/controllers/join.rs:23`). So a person can be in two
organisations only by creating the second one (PR 4).

After this PR, a person with an account can ask to join another
organisation, and a removed or declined person can ask again.

**How it works (Raj's option A).**

- **Signed in.** `/join/beta` shows "Ask to join Beta", with one field: the
  username to use in Beta. Posting it makes a pending membership for the
  signed-in account. They stay in the org this browser was in, and Beta
  shows in the switcher as "Waiting for approval".
- **Signed out, with an account.** "Already have an account? Sign in to join
  Beta" goes to `/login?next=/join/beta`, the normal sign-in form with its
  rate limits. After signing in they land back on `/join/beta` and see the
  form above.
- **Signed out, new to the app.** Today's new-account form, unchanged except
  that an existing email now gets "An account with this email already
  exists. Sign in to join Beta." with the sign-in link.
- **Approval.** An owner or admin approves as today. Approval is now one
  conditional update. In the same transaction it re-checks the approver,
  revokes any Claude tokens and unused codes the person still has in that
  org, ends a logins unlock for that org, re-adds them to `#general` and to
  their old direct messages, and clears the logins that were shared with them
  by name. After the commit the person gets an email.
- **Open sessions** gain the org on their next request. Nobody signs in
  again.

**Proof of email.** Being signed in is the proof. Nothing attaches a
membership to an account except a request made from that account's own
session. The new-account form never attaches to an existing account.

**Waived until a follow-up.** "The same response whether or not the email has
an account" is not met by this PR. The new-account form, like `/signup`
today, still says an email exists. Raj signed off on waiving it until the
follow-up "email verification on /signup and /join" (board loop
`email-verify`). The link's security requirements are copied into "Out of
scope" so they aren't lost. `docs/plans/multi-org-1.md` marks the guarantee
as deferred.

**The mockup** is `docs/designs/join-existing.html`, approved by Raj. It sets
visuals and copy for every screen this PR keeps: section 1 (signed out,
today's form plus "Sign in to join"), 3 (signed in), 4 (already there: four
states with their notes), 5 (after asking), 7 (the owner's side, including
the invite card copy) and 8 (edge cases). Section 2 (the email link), the
two-tab signed-out shape in section 1, and the Home "approved" and "declined"
notices in section 6 are deferred. A note at the top of the mockup says so.

**No migration.** No new crate.

### Earlier constraints, and where this plan meets them

From `docs/plans/multi-org-1.md` ("Join with an existing account") and the
PR 5 security review:

| Constraint | Where |
|---|---|
| The person proves the email is theirs before a membership attaches | Signed-in session only; sign-in reuses `/login` |
| The response is the same whether the email has an account or not | **Deferred, not met.** Waived by Raj until `email-verify` |
| A removed or declined member can't rejoin as active | `request_to_join` only ever writes `pending`. Only `approve` writes `active`, and only from `pending` |
| The joiner sets their username for the new org | The signed-in form's one field, checked in that org |
| A rejoin updates the existing row, never inserts a second | `request_to_join` locks and updates the row; `(organisation_id, user_id)` is unique anyway |
| Approval revokes every token and unused code `(org, user)` still has, and clears that org's unlock, inside the approval transaction | `approve` calls `access_tokens::Model::revoke_all_for` and `users::Model::end_unlock_in` in its transaction |
| `approve` is a conditional update on `status = 'pending'` that changes exactly one row | `approve` (below). `reject` gets the same treatment |
| Open sessions regain the org once approved | No session change on approval; `CurrentMember`, the switcher and `switch` check `active_in` per request |

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
(`users::Model::reset_password_everywhere` calls `revoke_all_for_user`).

**This PR makes "removed, then rejoin" real.** PR 5 wrote the rules; until
now there was no route from `removed` or `rejected` back to `pending`
(`docs/flows/accounts.md:199`).

## Proof of email

**Sign in with the password first.**

- It reuses `POST /login` as it is: the attempt is recorded before the
  password check, 5 failures per email or 20 per IP in 15 minutes lock it
  (`auth_attempts::begin_login`), an unknown email runs the dummy hash, and
  a locked account is refused.
- The only change to sign-in is that `safe_next` (`src/controllers/auth.rs:57`)
  also accepts a join page (below).
- No new tokens, columns or emails, so no migration and no new way to make
  the app email a stranger.

**`safe_next` for join pages.** The value must be exactly `/join/` followed by
1 to 60 characters, each one of `a`-`z`, `0`-`9` or `-`, and nothing else.
Checked byte by byte on the raw value: no regex, no lowercasing, no
decoding. So nothing can follow the slug (no `/`, `?`, `#`, space, `%`,
backslash or line break), and anything outside ASCII fails. The existing
`/oauth/authorize?` rule is unchanged.

**Through forgot-password.** The login page's "Forgot password" link carries
`next` to `/forgot`. The forgot form posts it, the reset email's link carries
it (`/reset/{token}?next=...`), and the reset page sends the person to
`/login?reset=1&next=...`. Every hop runs `safe_next` again, and a value that
fails is dropped. So a person who forgot their password still ends on the
join page.

## What changes, file by file

### Join

| File | Change |
|---|---|
| `src/models/memberships.rs` | New `JoinParams { name }`, validated like `RegisterParams.name` (`USERNAME_RE`, `not_reserved`). New `pub enum JoinOutcome { Requested(Model), AlreadyPending(Model), AlreadyMember(Model) }`. New `request_to_join(db, org, user, params) -> ModelResult<JoinOutcome>`, below. The old `join` is renamed `join_new_account`; its body is unchanged. |
| `src/models/memberships.rs` | New `MAX_PENDING_REQUESTS: u64 = 5`. |
| `src/models/memberships.rs` | `active_first()` becomes `status_rank()`: `CASE status WHEN 'active' THEN 0 WHEN 'pending' THEN 1 ELSE 2 END`, sorted ascending. Used by `default_for_user`, `default_by_user` and `for_user_with_orgs`. A person with no active org sees the waiting page for a request before an older removed or declined page. |
| `src/models/memberships.rs` | New `history(&self) -> Option<History>`: `Removed(date)`, `Declined(date)` or `DeclinedBefore`, from the row's own dates (see "History"). |
| `src/controllers/join.rs` | `new` takes `Option<CurrentUser>`. Signed out: today's page plus "Already have an account? Sign in to join {org}", linking to `/login?next=/join/{slug}`. Signed in: looks up `in_org_for_user(org.id, user.id)` and renders `join/existing.html` in one of the mockup's four states (section 4): **active** ("You're already in Beta" with "Open Beta", the switch post; straight to Home when Beta is already this browser's org), **waiting** (the date asked and the username, no form), **declined** (the form with "Your last request was declined on {date}. You can ask again."), **removed** (the form with "You were removed from Beta on {date}. You can ask to join again."). No row: the plain form. Side-effect free: it reads, never writes. |
| `src/controllers/join.rs` | The username starts as the person's username in the org this browser is in (the `current_org` cookie, checked with `active_in`), else their default membership's, else their old username in Beta for a rejoin. They can change it. |
| `src/controllers/join.rs` | Every join page response (`new`, `create`'s redraws, `request`'s redraws) goes through a `guarded` helper like `src/controllers/oauth.rs:249`: `Cache-Control: no-store`, `X-Frame-Options: DENY` and `Content-Security-Policy: frame-ancestors 'none'`. Move `guarded` to a shared place (`src/views/headers.rs`) so both controllers use one copy. |
| `src/controllers/join.rs` | New `POST /join/{slug}/request`, `request`: `CurrentUser` (a signed-out post gets the usual 303 to `/login`). Calls `request_to_join`. `Requested` and `AlreadyPending`: 303 to `/dashboard?requested={org.id}`. `AlreadyMember`: 303 to `/join/{slug}`. Field errors, the pending cap and a locked account: 422, the form redrawn. Logs `user_id` and `org_id` only, "requested to join". |
| `src/controllers/join.rs` | **Asking ends acting.** When the request carries an `acting_org` cookie, the 303 clears it (`cleared_acting_org_cookie`), as `enter_org` does for a create in PR 4. The signed-in page shows the super admin "This request comes from your own account" and "Asking ends acting" (copy from the mockup). |
| `src/controllers/join.rs` | `create` (the new-account form) is unchanged except the existing-email message: "An account with this email already exists. Sign in to join {org}." with the link. It never touches an existing account. |
| `src/controllers/auth.rs` | `safe_next` (above). `login_page`, `forgot_page`, `forgot`, `reset_page` and `reset` carry `next`. Doc comments updated. |
| `src/mailers/auth.rs`, `src/mailers/auth/forgot/` | The reset link gains `?next=` when a safe one was given. |
| `src/controllers/auth.rs` `login_page` | When `next` is a join page, a line naming the org being joined. The name comes from the slug looked up on the server, never from the query text. An unknown slug shows the plain page. |
| `src/controllers/dashboard.rs` | `requested_notice`: when `?requested={id}` is set and `in_org_for_user(id, me)` is `pending`, Home shows the mockup's section 5 notice. Any other status, a foreign or unknown id: no notice. Same pattern as `stale_notice`. With no active org, `/dashboard` shows the waiting page and the parameter is ignored. |
| `frontend/app.ts` (~221) | Add `"requested"` to the `flags` list, so Home drops it from the address and a reload doesn't repeat the notice. |
| `assets/views/join/new.html` | The "Sign in to join {org}" link (mockup section 1, today's form only). |
| `assets/views/join/existing.html` (new) | The signed-in states (mockup sections 3 and 4). Uses `layouts/auth.html` and passes its own `org`, never `current_org_id`, so the page carries no org claim and `app.ts` adds no `_org` field. Shows which account is joining, with "Not you? Sign out". |
| `assets/views/dashboard/index.html` | The after-asking notice (mockup section 5). |
| `assets/views/auth/login.html`, `auth/forgot.html`, `auth/reset.html` | Hidden `next` fields; the join line on the login page. |

**`request_to_join`, in one transaction:**

1. `ValidatorTrait::validate(params)`.
2. Read the existing `(org, user)` row, if any, `FOR UPDATE`. Membership row
   first, then the user row, the same order `remove` and `approve` use.
3. `active`: return `AlreadyMember`. `pending`: return `AlreadyPending`.
   Nothing is written in either case, the username included.
4. Lock the user row `FOR NO KEY UPDATE` (as `create_for_user` does).
5. Refuse a locked account: "Your account is locked. Reset your password to
   unlock it." (the same check and words as
   `organisations::create_for_user`, `src/models/organisations.rs:220`).
6. If step 2 found no row, read it again now. A double submit waited on the
   user-row lock, so it sees the first one's insert: `pending`, returns
   `AlreadyPending`.
7. Count the user's `pending` rows. At `MAX_PENDING_REQUESTS`: a form error,
   with the mockup's copy. Two requests at once run one after the other.
8. Username check: case-insensitive, in this org, excluding this person's
   own row. Their old username in Beta is theirs to keep. Taken: the
   mockup's "That username is already used in {org}." on `name`.
9. **Rejoin** (row is `removed` or `rejected`): an `UPDATE ... WHERE id = ?
   AND status IN ('removed', 'rejected')` that sets `status = 'pending'`, the
   new `username` and `updated_at`. It leaves `role`, `approved_by_id`,
   `approved_at`, `removed_at` and `removed_by_id` alone: they are the
   history, and `approve` overwrites them. Exactly one row must change.
   **New:** insert a pending `member` row, as `join_new_account` does, with
   `welcomed_at = now()`. They already know the app, so the welcome pop-up
   doesn't show. A rejoined row keeps its own `welcomed_at`.
   `join_new_account` doesn't set it (a new person still gets the pop-up).
10. Commit, return `Requested`.

**Backstop for the unique indexes.** A violation of `idx-memberships-org-user`
can still happen if the two requests interleave before either locks the user
row. The handler rolls the transaction back, reads the row on `ctx.db`, and
returns `AlreadyPending`. One on `idx-memberships-org-username` (two people
taking the same name at once) becomes the username field error.

**The org cap doesn't apply.** `MAX_ORGS_CREATED` counts organisations the
person created (`organisations.created_by_id`). Joining creates none.
`request_to_join` never reads it. Tested.

**Limits.** A cap of 5 pending requests per account, under the user-row lock.
No daily limit.

### Approve and decline

| File | Change |
|---|---|
| `src/models/memberships.rs` | `approve(db, org_id, approver: Approver, id) -> ModelResult<Self>`, replacing `approve(self, ...)`. `Approver { user_id, acting }` comes from `CurrentMember`. Below. |
| `src/models/memberships.rs` | `reject(db, org_id, approver: Approver, id)`: a conditional update on `status = 'pending'` that sets `status = 'rejected'`, `approved_by_id` and `approved_at = now()`. `approved_at` becomes the decision time for a decline too. Exactly one row must change. Today it checks a row loaded earlier and then writes unconditionally, so a stale page could turn a just-approved member into declined. It re-checks the approver the same way `approve` does. |
| `src/models/conversation_members.rs` | `add_if_missing(db, conversation, user_id)`: `INSERT ... ON CONFLICT DO NOTHING`. |
| `src/models/conversations.rs` | `dms_of(db, org_id, user_id)`: `in_tenant(org_id)`, `kind = 'dm'`, and `dm_key LIKE 'dm:{id}:%' OR dm_key LIKE 'dm:%:{id}'`. The id is an `i64` formatted by the code, and `dm_key` is always `dm:{low}:{high}`, so `dm:1:%` can't match user 11 or 21, and `dm:%:1` can't match `dm:5:11` or `dm:5:21`. |
| `src/models/login_accesses.rs` | `remove_for_member(db, org_id, user_id)`: `DELETE FROM login_accesses WHERE user_id = ? AND login_id IN (SELECT id FROM logins WHERE organisation_id = ?)`. Only this org's logins. The stale doc comment at `:66-71` ("a removed person cannot rejoin (one organisation per user)") is rewritten: the rows stay after removal for "Change these at the source", and approval clears them. |
| `src/controllers/members.rs` | `approve` and `reject` pass `member.org.id`, an `Approver` and `id`. `approve` then sends the approval email (below). Error toasts as today, plus the self-approval one. |
| `src/mailers/membership.rs` (new, in `src/mailers/mod.rs`) | `MembershipMailer::approved(ctx, user, org)`. Templates in `src/mailers/membership/approved/`. Below. |

**`approve`, in one transaction:**

1. If not `approver.acting`: `active_in_for_share(&txn, org_id,
   approver.user_id)` and require owner or admin. Otherwise "Only owners and
   admins can approve." A person demoted or removed in another tab can't
   approve from an open page. Skipped while acting: the acting super admin
   has no membership there.
2. `UPDATE memberships SET status = 'active', role = 'member',
   approved_by_id, approved_at = now(), removed_at = NULL, removed_by_id =
   NULL, updated_at WHERE id = ? AND organisation_id = ? AND status =
   'pending' AND user_id <> :approver`. Exactly one row must change.
3. Zero rows: read the row. If it is pending and its `user_id` is the
   approver: "You can't approve your own request." Otherwise "Only pending
   requests can be approved." Nothing else runs. Only an acting super admin
   can be both.
4. Lock the user row `FOR NO KEY UPDATE`, as `remove` does.
5. `users::Model::end_unlock_in(&txn, user_id, org_id)`.
6. `access_tokens::Model::revoke_all_for(&txn, org_id, user_id)`.
7. `login_accesses::Model::remove_for_member(&txn, org_id, user_id)`.
8. Add them to `#general` with `add_if_missing`. Today's `approve` already
   does this (`src/models/memberships.rs:341`); a rejoiner lost it at removal.
9. Add them back to each of their DMs in this org (`dms_of`) with
   `add_if_missing`. Groups are not restored, as for a first join.
10. Commit. Log `org_id`, `user_id`, `approver_id`, "approved member".

**Why `role = 'member'` here and not at rejoin.** A rejoin leaves the old role
on the pending row so "Change these at the source" can still work out what a
former admin could see. The pending row's role grants nothing: every check
needs `active`. Approval sets `member` whatever it was. The rollback section
has the one line that makes a revert safe.

**Why steps 5 to 7 on every approval, not only rejoins.** For a first join
they find nothing and cost three indexed statements. One path means no
"is this a rejoin" branch to get wrong.

**The approval email.**

- Sent for every approval, first-time joiners too.
- Subject without the org name, for example "Your request was approved"
  (the mockup sets the words). The org name is only in the body, escaped
  (`| escape` in `html.t`).
- Fire-and-forget: sent after the commit, and a failure is logged and never
  fails the request. The approval stands.
- At most 20 approval emails per org in any 24 hours, counted as that org's
  memberships with `approved_at` in the last 24 hours (this one included).
  Over that, no email and an info log with the org id. Constant
  `APPROVAL_EMAILS_PER_DAY` in `src/mailers/membership.rs`.
- It doesn't name the approver, so an acting super admin isn't named.
- Until links can name an org, it links to `{app_url}/dashboard` and says to
  choose the org from the organisation menu.

**Deferred: the Home "you were approved" and "declined" notices** (mockup
section 6). They need a "seen" flag, which is a migration. Today's switcher
row going from greyed to live is enough for now.

### History

The waiting list, and the join page's declined and removed notes, show one
line from the row's own dates. Never from `updated_at`.

- `reject` now sets `approved_at = now()` and keeps `approved_by_id`, so
  `approved_at` is "when it was last decided".
- A rejoin keeps `approved_by_id`, `approved_at`, `removed_at` and
  `removed_by_id`. `approve` overwrites them.

The line, for a row that isn't active:

| Row | Line |
|---|---|
| `removed_at` set and later than `approved_at` | "Removed on {date}" |
| otherwise `approved_at` set (the last decision was a decline) | "Declined on {date}" |
| `approved_by_id` set, `approved_at` unset (a decline from before this PR) | "Declined before" |
| none of these (a first request) | no line |

So "declined after a removal" reads "Declined on", the most recent decision.
Dates only, in the org's time zone (`member.tz()` on the Members page,
`org.timezone` on the join page), with no names.

### Owners' waiting list

| File | Change |
|---|---|
| `src/controllers/members.rs` | `MemberRow` gains `history: Option<String>` for pending rows only, from `history()` above. |
| `assets/views/members/_pending_row.html` | Shows `history` (mockup section 7). |
| `assets/views/members/index.html` | The invite card copy from mockup section 7: "People who use it join with a new or existing account, then wait for you or another admin to let them in." |

**What owners see, and don't.** The pending row shows the username the
person chose for this org, their email (as today), and the history line. It
doesn't show `users.name`, whether the account is new or old, other
organisations, or other usernames. Nothing on the invite card or the waiting
list says or suggests the email was checked; it wasn't.

### "Change these at the source"

`rotate` (`src/controllers/members.rs:308`) today needs `status = 'removed'`.
It becomes "not active and `removed_at` set", so it still works for a
removed person who asked again (pending) or was then declined (rejected).
Since the rejoin leaves the role alone, `ToChange::for_former_member` still
sees a former admin's role and lists every login. After approval the row is
active and `removed_at` is cleared, so the page ends. The owners' "Removed"
list still lists only `removed` rows; the pending row's "Removed on" line is
where an owner sees the history.

### Super admins

- **No active membership, even with a pending one, lands on `/admin`.** In
  `resolve_org` (`src/extractors/current_member.rs:283-286`), the super admin
  check moves to cover any non-active default: when `default_for_user` is
  `None` or not active and the user is a super admin, redirect to `/admin`.
  Today a super admin whose only membership is pending gets the waiting page
  and loses `/admin`.
- **Asking ends acting** (above).
- **Approving while acting** works as today, except their own request
  (refused in the `UPDATE`).

### Stale tabs

`removed_from` (`src/extractors/current_member.rs:198`) becomes "the claimed
org's membership is there and not active". A tab drawn for an org where the
person is now pending (removed, then asked again) or declined gets
`X-Org-Gone`, the "no longer have access" banner, instead of the switch one.
The user id still comes from the resolved member, never the request.

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
- A person whose only membership is the new one is on the waiting page.
  "Check again" loads `/dashboard`, which now finds an active org.

### Copy that said "no way back"

Removal no longer means for ever. Comments only, no visible copy:

- `src/controllers/dashboard.rs:33`: "then it says so, with no way back"
  becomes "then it says so, with no Back button".
- `assets/views/layouts/app.html:66`: "no way back" becomes "no Back button".

### Docs

| File | Change |
|---|---|
| `docs/flows/accounts.md` | "Join by link": three paths (new account, sign in to join, signed in), a sequence diagram for the signed-in path and for approval. "Membership states": `removed --> pending` and `rejected --> pending` by asking again; remove "There is no route that moves rejected or removed back". The default-org order. "Sign in": `next` also takes a join page, kept through forgot-password. Super admins with no active membership land on `/admin`. |
| `docs/architecture/erd.md` | Statuses: a rejoin goes back to `pending` on the same row. `approved_at` is the time of the last decision, approve or decline. Approval clears `removed_at` and `removed_by_id` and sets `role = member`. The multi-org paragraph names PR 5 and PR 6. |
| `docs/flows/notifications.md:79` | Approvals send an email to the person, at most 20 per org a day. Join requests, declines, removals and role changes still send none. |
| `docs/flows/super-admin.md:95` | An acting super admin can't approve their own request. Asking to join ends acting. With no active membership they land on `/admin`. |
| `docs/flows/documents.md` | Approval clears logins shared by name with that person in that org. Rights that come from authorship come back on approval: an uploader can again delete files they uploaded before the removal ("Delete a file: the uploader, owners and admins"). |
| `docs/plans/multi-org-1.md` | Done in this commit: the "same response" guarantee marked deferred (not met), linked to `email-verify`; the join constraints point here. |
| `docs/designs/join-existing.html` | Done in this commit: a note at the top says the email link (section 2), the two-tab signed-out shape (section 1), the email-link edge cases (section 8) and the "approved" and "declined" Home notices (section 6) are deferred, and that section 5 uses `?requested=`, not `?asked=`. |

## Edge cases

- **Already a member.** The join page says so with "Open {org}". A post is a
  no-op and goes back there.
- **Already waiting.** The page says so. A post changes nothing, the
  username included, and lands on Home with the notice.
- **Removed, asks again.** Same row, now `pending`, role untouched. They
  can't open Beta. Once approved they come back as a member. Their old
  Claude tokens stay revoked.
- **Declined, asks again.** Same. Their row was never active, so there is
  nothing to revoke, but approval runs the same steps.
- **A super admin with only a pending membership.** Lands on `/admin`, not
  the waiting page. Tested.
- **No active org, removed from Acme, asks to join Beta.** The waiting page
  for Beta, not "removed from Acme" (`status_rank`).
- **Active in Acme, asks to join Beta.** Stays in Acme. Home shows the
  notice. The switcher shows Beta greyed.
- **Approved while the person is in Acme.** The email says to choose Beta
  from the menu. Their next page load shows Beta live in the switcher.
- **A stale Members page approves a removed member.** The conditional update
  changes nothing. Toast "Only pending requests can be approved." Tested.
- **An approver demoted in another tab.** The in-transaction re-check
  refuses.
- **Two owners approve at once.** One update changes the row. The other gets
  the toast. One email.
- **Approve and decline at once.** Whichever commits first wins; the other
  gets its toast.
- **A double submit of the join form.** The second waits on the user-row
  lock, re-reads, and gets `AlreadyPending`. The unique index is the
  backstop.
- **A locked account.** Its sessions ended when it locked, so it can't
  normally reach the form. A session that does is refused after the
  user-row lock.
- **Two people take the same username in Beta at once.** The unique index on
  `(organisation_id, lower(username))`, shown as the field error.
- **A removed member's old username.** Their row holds it, so nobody else
  could take it. They may keep it or pick another.
- **Pending cap.** Five open requests. A decline frees a slot.
- **Signed in as someone else on a shared browser.** The page names the
  joining account, with "Not you? Sign out".
- **The super admin joining.** Allowed, from their own account. It ends
  acting. They can't approve themselves.
- **DM history after rejoin.** Approval puts the person back in their DMs.
  They see the messages from before the removal, and also any sent to that
  DM while they were away.
- **Authorship rights come back.** Things tied to "who made it", such as
  deleting a file they uploaded, work again after approval. Written in
  `docs/flows/documents.md`.
- **Old notifications.** Their Beta notifications from before the removal
  are still in Beta's bell. Unsent mention emails were cancelled at removal.
  Accepted.
- **The default org after a rejoin.** The row keeps its old `created_at`.
  Once approved it may be their oldest active org, so a fresh sign-in starts
  there. Accepted.
- **Framing.** Join pages send `X-Frame-Options: DENY` and `frame-ancestors
  'none'`, so the "Ask to join" button can't be clickjacked.
- **CSRF on the signed-in post.** The origin check
  (`src/initializers/origin_check.rs`) refuses a cross-origin POST with 403,
  and cookies are `SameSite=Lax`. Tested on the new route.
- **`GET /join/{slug}`** never writes, so a link or prefetch can't file a
  request.
- **The join link is guessable.** It is the slug. Knowing it lets someone
  file a request and learn the org's name. Every request needs an owner or
  admin. Requests email and notify nobody. An account can file at most 5 at
  once. Unguessable join links stay deferred (PR 4's list).
- **A stranger attaching a request to someone else's account.** Only a
  session for that account can call `request_to_join`. The new-account form
  never attaches to an existing account. Tested.
- **One person spamming many orgs.** Five pending requests per account. More
  needs more accounts, which are free to make until the `/signup` and
  `/join` IP limit (see "Out of scope").
- **The stale-tab check.** `POST /join/{slug}/request` takes `CurrentUser`,
  so the claim check never runs, and the join page carries no claim.

## Tests

Unit (`src/controllers/auth.rs`): `next_only_returns_to_the_allow_access_page`
becomes `next_only_returns_to_allow_access_or_a_join_page`. Accepts
`/join/acme-studio`, `/join/a` and a 60-character slug. Refuses: the empty
string, `/join/`, `/join/Acme`, `/join/acme/`, `/join/acme#x`, `/join/acme?`,
`/join/acme?x=1`, `"/join/ acme"`, `/join/acme%20`, `/join/%61cme`,
`/join/ａcme` (fullwidth), `/\join/acme`, `/join/acme\r\n`,
`/join/acme/request`, `/join/../admin`, `//join/acme` and a 61-character
slug.

Model (`tests/models/memberships.rs`):

- `request_to_join_adds_a_pending_member_row`.
- `request_to_join_marks_a_new_row_welcomed`: `welcomed_at` set on insert;
  a rejoined row keeps its own value; `join_new_account` leaves it unset.
- `request_to_join_when_pending_changes_nothing`: the username stays.
- `request_to_join_when_active_says_already_a_member`.
- `a_double_submit_returns_already_pending`: two calls with `tokio::join!`,
  one on `ctx.db` and one on `second_connection(&ctx)`; one `Requested`, one
  `AlreadyPending`, one row.
- `request_to_join_refuses_a_locked_account`.
- `rejoin_after_removal_updates_the_same_row`: one row before and after, now
  `pending`, new username; `role`, `removed_at`, `approved_by_id` and
  `approved_at` unchanged.
- `rejoin_after_decline_updates_the_same_row`.
- `request_to_join_never_writes_active`: for every starting status.
- `request_to_join_checks_the_username_in_that_org_only`.
- `request_to_join_refuses_a_bad_or_reserved_username`.
- `request_to_join_stops_at_five_pending_requests`.
- `two_requests_at_once_do_not_pass_the_pending_cap`.
- `the_org_cap_does_not_apply_to_joining`.
- `approve_changes_only_a_pending_row`: on `active`, `removed` and
  `rejected` it errors and the row is unchanged.
- `two_approves_at_once_change_one_row`.
- `approve_rechecks_the_approver`: an admin demoted to member before the
  approve is refused; an acting super admin is not checked.
- `nobody_approves_their_own_request`: the own message, the row stays
  pending.
- `approve_sets_member_and_clears_removal`: a former admin comes back as a
  member, `removed_at` and `removed_by_id` cleared.
- `approve_revokes_tokens_and_codes_in_that_org_only`.
- `approve_ends_an_unlock_for_that_org_only`.
- `approve_clears_named_login_access_in_that_org_only`: Alice named on a
  Beta login and an Acme login; approving in Beta removes only the Beta row.
- `approve_adds_general_once`.
- `approve_restores_direct_messages_in_that_org`.
- `dms_of_never_over_matches`: users 1, 11 and 21 with DMs `dm:1:11`,
  `dm:11:21`, `dm:5:21` and `dm:1:21`. `dms_of(1)` is `dm:1:11` and
  `dm:1:21`. `dms_of(11)` is `dm:1:11` and `dm:11:21`. `dms_of(21)` is
  `dm:11:21`, `dm:5:21` and `dm:1:21`. A DM in another org and a group never
  match.
- `reject_changes_only_a_pending_row`.
- `reject_sets_the_decision_date`: `approved_at` set, `approved_by_id` kept.
- `history_lines`: removed ("Removed on"), declined ("Declined on"), declined
  after a removal ("Declined on", the later date), a pre-PR decline with no
  date ("Declined before"), a first request (none). Dates in the org's time
  zone.
- `default_for_user_prefers_pending_over_removed`.

Requests (`tests/requests/join_existing.rs`, new, in `tests/requests/mod.rs`).
Bob owns Beta. Alice owns Acme:

- `signed_in_join_page_asks_only_for_a_username`: no email or password
  field, names Alice's account, username prefilled from this browser's org.
- `join_pages_cannot_be_framed`: `X-Frame-Options: DENY`,
  `frame-ancestors 'none'` and `no-store` on the signed-out page, the
  signed-in page and a 422 redraw.
- `the_join_page_writes_nothing`: GETs signed in and out leave no row.
- `signed_in_join_page_says_already_a_member`,
  `signed_in_join_page_says_already_waiting`.
- `the_join_page_shows_the_removed_and_declined_notes`.
- `a_cross_origin_join_request_is_refused`: `Origin: https://evil.example`,
  403, no row.
- `joining_while_signed_in_keeps_the_current_org`.
- `joining_lands_on_home_with_the_request_notice`.
- `the_request_notice_names_only_an_org_you_asked_to_join`.
- `a_taken_username_redraws_the_form`.
- `joining_needs_a_session`.
- `asking_while_acting_ends_acting`: the 303 clears `acting_org`.
- `a_super_admin_with_only_a_pending_row_lands_on_admin`.
- `the_switcher_shows_the_request_as_waiting`.
- `after_approval_alice_switches_to_beta_without_signing_in`.
- `after_approval_the_waiting_page_opens_the_org`.
- `rejoin_after_removal_waits_for_approval`.
- `rejoin_after_decline_waits_for_approval`.
- `a_rejoined_member_has_no_old_claude_tokens`.
- `the_rotate_page_survives_a_rejoin`: removed, asked again, still reachable
  by the owner; declined, still reachable; after approval, 404.
- `a_tab_for_an_org_you_are_pending_in_gets_the_gone_header`.
- `sign_in_to_join_returns_to_the_join_page`.
- `forgot_password_keeps_the_join_page`: `next` survives `/forgot`, the reset
  email's link and the reset, and a bad `next` is dropped at each step.
- `sign_in_to_join_keeps_the_sign_in_limits`.
- `the_new_account_form_never_attaches_to_an_existing_account`.
- `approval_emails_the_person`: one email to Alice; the subject has no org
  name; the body names Beta, escaped, not Acme, not the approver.
- `approval_emails_stop_at_twenty_a_day_per_org`: the 21st approval sends
  none and still approves.
- `a_join_request_emails_and_notifies_nobody`.
- `an_acting_super_admin_approves_others_but_not_themselves`.
- `a_stale_members_page_cannot_approve_a_removed_member`.
- `the_waiting_list_shows_only_this_orgs_details`: username, email, history
  line; not `users.name`, not "Acme", no names of who decided.

Existing files:

- `tests/requests/join.rs`: `signed_in_users_skip_the_join_and_signup_forms`
  becomes `signed_in_users_skip_signup_but_see_join`. The existing-email case
  checks the new message and link.
- `tests/requests/members.rs`: approve and decline use the new signatures;
  the invite card copy.
- `tests/requests/stale_tab.rs`: `("POST", "/join/{slug}/request")` is
  inserted before the `// PinnedMember` entries in `EXEMPT`, and its size goes
  from 15 to 16.
- `tests/requests/multi_org.rs`: `alice_in_two_orgs` keeps inserting by SQL;
  its comment points at `request_to_join`.
- `tests/requests/org_removal.rs`: the gone-header tests still pass with the
  wider `removed_from`.

**Not covered by `cargo test`:** the email in a real client, the switcher menu
staying stale until reload, and the screens against the mockup. Checked by
hand.

## Out of scope

- **The organisation in the address** (`/o/acme-studio/tasks`), the project
  that replaced PR 7. Until then the approval email can't link straight into
  the org.
- **Email verification on /signup and /join** (board loop `email-verify`).
  It brings back the mockup's email link and two-tab signed-out shape, and
  meets "the same response whether the email has an account". Requirements
  from the security review, kept here:
  - a 32-byte random token, stored only as its SHA-256 hash;
  - single use, consumed on POST, never on GET (link scanners open links);
  - expires after 60 minutes;
  - bound to `(email, org)`;
  - pages sent with `Referrer-Policy: no-referrer` and `Cache-Control:
    no-store`;
  - never signs anyone in; it only attaches a waiting membership;
  - its own `auth_attempts` kind for rate limits;
  - the same work for known and unknown emails (time and what is sent);
  - no free org text in the email subject;
  - plus an IP rate limit on `POST /signup` and `POST /join`.
- **The Home "approved" and "declined" notices** (mockup section 6). They
  need a migration.
- **A notice to owners about new requests.** Requests still email and
  notify nobody.
- **"Don't let them ask again"**: a switch for owners to block repeat
  requests from one person. Follow-up.
- **Unguessable join links** and **leaving an organisation**, deferred in
  PR 4 and PR 5.
- **An "Ask again" button** on the removed and declined pages.
- **Restoring group conversations** after a rejoin.
- **The super admin "sign out everywhere and lock"** follow-up from PR 5.

## Risks

- **A wider `next`.** Any loosening of `safe_next` is an open-redirect risk.
  The check is byte by byte with a fixed alphabet, and the unit test lists
  the tricks. It now runs at four more hops (forgot, the email link, reset).
- **Approval now writes more.** It locks the user row and touches tokens,
  unlock columns, login access and conversations. The lock order matches
  `remove`. One short transaction.
- **The new-account form still reveals emails**, as `/signup` does. Waived by
  Raj until `email-verify`.
- **DM history comes back,** including messages sent while they were away.
  The owner chose to approve.
- **Clearing named login access** surprises a teammate who expected their
  share to survive. Only on approval, and a share is one click to redo.
- **A pending row keeps an old owner or admin role** until approval. It
  grants nothing (every check needs `active`), but a revert must reset it
  (see "Rollback").
- **Approval emails capped per org.** The 21st approval in a day sends none.
  The person still sees the org in the switcher.
- **The default-org order change** also moves `default_by_user` on `/admin`.
  Harmless.

## Rollback

No migration. Before reverting, run:

```sql
UPDATE memberships SET role = 'member' WHERE status = 'pending';
```

The old `approve` doesn't reset the role, so without this a rejoined former
admin or owner would come back with their old role. Every other pending row
is already a member, so the line changes nothing else.

Then reverting is safe for data:

- Rejoined rows stay `pending`. The old `approve` can still approve them. It
  doesn't revoke tokens or clear the unlock, but removal already did both.
- `approved_at` on declined rows is a date the old code never reads. The
  history lines go with the revert; the old code had none.
- Rows approved by the old code keep `removed_at`. The old `rotate` only
  reads `removed` rows, so nothing shows it.
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
  signs in, lands on the join form, asks, and is back in Acme with the
  notice. Reload: the notice is gone and the address has no `?requested=`.
  The switcher shows Northwind waiting.
- The same, through "Forgot password": he ends on the join page.
- Dev approves. Sam gets the email (Mailpit): no org in the subject. After a
  reload Northwind is live in the switcher and switching works.
- Dev removes Sam. Sam opens the join link: the form with "You were removed
  on ...". He asks again. Dev's Members page shows "Removed on ...", and
  "Change these at the source" still opens. Approve. Sam's old DM with Dev
  is back; `#general` is back; his old Claude token still fails; a login
  shared with him by name is no longer visible.
- Decline, then ask again: "Declined on ...".
- A sixth request: the error.
- As the super admin acting in Northwind: the join page for another org says
  "Asking ends acting"; asking ends it.
- Sam's email on the new-account form: the "Sign in to join" message, and
  nothing changes on his account.

## Screenshots needed

From the local app with the data above, on the `pr-screenshots` branch under
`multi-org-6/`. Desktop 1440px and phone 390px, light and dark for every new
screen:

- the signed-out join page with "Sign in to join";
- the new-account form's "Sign in to join" error;
- the sign-in page with the join line;
- the signed-in join form, and with a username error;
- the join form in its removed and declined states, with the note;
- "already a member" and "already waiting";
- the acting super admin's join page;
- Home with the after-asking notice;
- the switcher with a waiting row (menu and phone sheet);
- the owner's waiting list with "Removed on" and "Declined on" lines, and
  the invite card;
- the pending-cap error;
- the approval email (text and HTML, from Mailpit).

No email-link screens.

## Decisions for Raj

All decided on 20 October 2026: option A, the plan's shape, with the staff
and security reviews folded in.

1. **Proof of email:** sign in with the password first. Decided.
2. **"Same response whether the email has an account":** waived until
   `email-verify`. Decided.
3. **A cap of 5 open requests per account, no daily limit.** Decided.
4. **An approval email for every approval**, no org in the subject, at most
   20 per org a day. Home notices deferred. Decided.
5. **Nothing new to owners about requests.** Decided.
6. **Restore DMs on approval**, with history including the gap. Decided.
7. **Clear logins shared by name on approval**, this org's logins only.
   Decided.
8. **The rejoin leaves the role alone; approval sets member**, so "Change
   these at the source" survives a rejoin. Decided.
9. **Nobody approves their own request**, in the `UPDATE` itself. Decided.
10. **The default org order: active, pending, then the rest**, and a super
    admin with no active membership lands on `/admin`. Decided.
11. **Decline is a conditional update too, and records its date.** Decided.
12. **No "Ask again" button** on the removed or declined pages; "don't let
    them ask again" is a follow-up. Decided.
13. **A super admin asking to join ends acting.** Decided.
