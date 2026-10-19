# Multi-org 4: create another organisation

Status: Draft · PR 4 of 7 · Owner: Raj · 19 October 2026

## Summary

PR 3 (#44) added the switcher and the stale-tab check. Nobody can have two
organisations yet, except by SQL. This PR lets a signed-in person create
another organisation from the switcher.

- `GET /orgs/new` shows the form. `POST /orgs` creates the organisation with
  the person as its active owner, under the username they choose for it.
- It uses signup's own code to set the organisation up: the slug, `#general`
  with the owner in it, and the "Company" and "Logins" folders. One code path.
- The form asks for the organisation name and your username there. Both are
  checked exactly as on `/signup`.
- After creating, the browser switches into the new organisation, the way
  `POST /orgs/{id}/switch` does: `current_org` is set, `acting_org` cleared,
  any logins unlock ended, and live connections closed. It lands on Home with
  a toast, and the "Getting started" checklist shows for the new org.
- The switcher shows for everyone now, even with one organisation, and ends
  with a "New organisation" row.
- Limits: at most 10 organisations created per person, and 3 per 24 hours.
  Both are counted from `organisations.created_by_id`.

No migration. This is the first PR that gives anyone a second membership in
production, so the rollback window for PR 2's index migration closes when it
ships (see "Gates").

The design is being drawn in parallel at `docs/designs/create-org.html`. The
plan covers what the server and `app.ts` must do. Layout, wording and the
choice of page, dialog or sheet are the mockup's. Its sections are named here
by topic (entry row, form, errors, after creating, acting). Swap in the
section numbers once it lands.

## Who may create

**Any active member, through `CurrentMember`.** That includes a super admin
while acting.

- `CurrentMember` on both routes, not `CurrentUser`. The form lives in the app
  layout, and a 422 has to redraw it there, so it needs the member. It also
  keeps `POST /orgs` inside the stale-tab check. A stale tab is refused and
  sent Home (or keeps its typed text behind the banner). That is safe, if
  slightly annoying, and it needs no new entry in the exempt list.
- Someone with no active membership (pending, declined, removed) sees their
  waiting or removed page, as today, so they can't create. Offering "create
  your own" on those pages is later.
- **Super admin while acting** (decision 1, recommended: allow). Creating is
  about their own account, not the org they are acting in. Like a switch, it
  clears `acting_org`, so acting ends and they land in their new org as its
  owner. The form says acting will end (mockup, acting section). The
  alternative, refusing while acting, needs its own message and buys nothing:
  they can leave and create a moment later.
- A super admin with no membership at all still goes to `/admin` from any
  tenant page. They can only create while acting.

## Fields and validation

The same rules as signup, so nobody meets two sets of messages.

| Field | Form name | Rule | Message |
|---|---|---|---|
| Organisation name | `organisation_name` | 2 to 80 characters, trimmed | "Use 2 to 80 characters." |
| Your username there | `name` | `users::USERNAME_RE` and `users::not_reserved` | "Start with a letter and use 3 to 30 letters or numbers." and the `everyone` message |

- `name` matches signup and join, so `field_errors` keys and the templates'
  error slots work the same way.
- The username is checked against `idx-memberships-org-username`
  (`organisation_id, lower(username)`) by nature: the new org has only its
  owner, so it is always free. No extra query.
- The form starts with the username you use in the current org
  (`member.username`, which is `user.name` while acting). The mockup decides
  whether to prefill.
- **Duplicate names** (decision 3, recommended: refuse). If one of your active
  organisations already has this name (case-insensitive, trimmed), the
  create is refused with a field error on `organisation_name`. The switcher
  shows only initial, name and role, so two "Acme" rows can't be told apart.
  This also stops a double-click from making two orgs. Other people's orgs
  may share the name; their slugs differ, as today.

The params get their own struct, `organisations::NewOrgParams
{ organisation_name, name }`, with the same `#[validate]` attributes as
`SignupParams`. The messages move into shared constants so they can't drift.
A test posts the same bad input to both forms and compares the errors.
Nesting `NewOrgParams` inside `SignupParams` was considered. Nested validator
errors come back under prefixed keys (`org.organisation_name`), which would
need changes in `field_errors`. Not worth it for two fields.

## Abuse limits

**The cap and the rate limit come from the `organisations` table, not
`auth_attempts`.**

- Every creation is already a row with `created_by_id` and `created_at`. So
  "how many has this person made" and "how many in the last day" are two
  counts on data that exists. No new table, no new row kind.
- `auth_attempts` is for signed-out flows (sign-in, forgot, app-password
  reauth). It is keyed by email hash and IP, and rows older than 24 hours are
  deleted on every write (`KEEP_HOURS`). It could not hold a lifetime cap.
- **Numbers** (decision 2, recommended): `MAX_ORGS_CREATED = 10` per person,
  counting the org they signed up with. `ORGS_PER_DAY = 3` in any 24 hours.
  Constants in `src/models/organisations.rs`. The super admin gets the same
  limits.
- **Races.** Inside the create transaction, lock the person's `users` row
  first (`SELECT ... FOR UPDATE`, Sea-ORM `lock_exclusive()`). Two creates at
  once from one person then run one after the other, and both counts and the
  duplicate-name check see the earlier one.
- **Refusal.** A 422 that redraws the form with the message on
  `organisation_name` (copy from the mockup's errors section). Not a 429:
  `base.html`'s `responseHandling` only swaps 422s, so a 429 would show
  nothing in an HTMX form.
- **No per-IP limit.** The person is signed in, and `/signup` itself has no
  rate limit today. Anyone can already make accounts, and with them orgs,
  without a limit. The cap stops one account piling up orgs. A limit on
  `/signup` is a separate change (see "Out of scope").
- `created_by_id` is set to `NULL` if the user is deleted, which frees their
  count. That is fine: the account is gone.

**Emails and notifications on creation: none.**

- Signup sends nothing today: no welcome email and no verification email
  (`signup::create` and `organisations::Model::sign_up` call no mailer).
  Creating an org sends nothing either.
- A new org has one member, so there is nobody to mention or notify.
  `SecurityMailer::to_owners` would only reach the creator.
- Join requests send no email to owners (`docs/flows/accounts.md`), so a new
  org's join link can't be used to email its owner.
- So there is no per-org email or notification spam path in this PR. The cap
  bounds how many join links and `#general` channels one account can make.

## What happens on create

`POST /orgs`, in `src/controllers/orgs.rs`:

1. `member: CurrentMember` (the claim check runs here), `Form<NewOrgParams>`.
2. `organisations::Model::create_for_user(&ctx.db, &member.user, &params)`:
   1. `ValidatorTrait::validate(params)`.
   2. Begin a transaction. Lock the user row.
   3. Count orgs with `created_by_id = user.id`, all time and in the last 24
      hours. Over either limit: a field error.
   4. Duplicate-name check over the person's active memberships' orgs
      (decision 3).
   5. `create_with_owner(&txn, &params.organisation_name, user,
      params.name.trim())`.
   6. `memberships::Model::mark_welcomed(&txn, org.id, user.id)` (decision 4).
   7. Commit.
3. On a field error: 422, the form redrawn with values and errors. Over HTMX,
   the form fragment; otherwise the full page. The same split as `GET /orgs`.
4. On success, the same steps as `switch`:
   - `users::Model::end_unlock(&ctx.db, user.id)`;
   - `redirect_response(&headers, "/dashboard?created=1")`, which gives
     `HX-Redirect` over HTMX and a 303 otherwise;
   - `current_org_cookie(&ctx, org.id)` and `cleared_acting_org_cookie`;
   - `users::Model::close_live_connections(&ctx, user.id)`;
   - an info log line: `user_id`, `org_id`, "created organisation". Numbers
     only.
5. The target is a constant. No `next`.

`create_with_owner` gains a `username: &str` and passes it to
`memberships::Model::create_owner`, which stops copying `user.name`. Signup and
the super admin task pass `&user.name`, so they behave exactly as today. This
is the one code path. Anything signup sets up later is set up here too.

The new org gets the column default time zone, `Europe/London`, as signup
does. The owner changes it in settings.

### Home after creating

- `dashboard::HomeQuery` gets `created: Option<String>`. The toast is built
  from `member.org.name`, the org this request resolved, never from the URL.
  The same pattern as `switched`. Copy from the mockup, for example
  "Northwind Bakery is ready".
- `Cache-Control: no-store` when `created` is set, as for `switched` and
  `from`.
- `app.ts` drops `?created=` with `history.replaceState`, next to `switched`
  (`frontend/app.ts` ~line 208).
- `/dashboard?created=1` typed by hand names your current org and changes
  nothing.
- **Checklist.** Shows by itself: the new membership has `guide_dismissed_at`
  null, and the owner gets the "sets up team" steps (invite, project, task,
  hello, tag). Nothing to add.
- **Welcome pop-up** (decision 4, recommended: skip it). It also keys off the
  new membership (`welcomed_at` null), so without a change it would show
  again. The person has already seen the tour in their first org, and it
  would sit on top of the toast. Step 2.6 marks it seen. Signup and join keep
  it.

## The switcher

- **Shows for everyone.** `PinnedMember` stops calling
  `memberships::Model::switcher_counts`, which saves one query per full page
  load. `CurrentMember.org_menu` is always true for a resolved member. The
  `org_menu` key and the template branches stay, because `/admin` renders
  `app.html` without it and keeps its plain "Platform admin" name.
- Remove `switcher_counts` and its model test. Nothing else uses it.
- **"New organisation" row** (mockup, entry row section): after the list in
  `orgs/_list.html`, under a separator. A link to `/orgs/new`, outside the
  switch `<form>`, with `role="menuitem"`. `orgItems` in `app.ts` already
  picks up every `[role^=menuitem]`, so arrow keys reach it. Check the letter
  jump: "N" now also lands on it.
- The same row appears in the phone sheet and on the no-JS `/orgs` page,
  since they share `_list.html`.
- More's "Organisation" row and Home's phone header button now show for
  everyone.
- While acting with no membership of your own, the list has "Acting in",
  an empty "Your organisations" and the new row. The mockup decides how the
  empty part looks.

### `GET /orgs/new`

- `CurrentMember`. Over HTMX, the form fragment `orgs/_new_form.html`, if the
  mockup opens it in a dialog or sheet. Otherwise the full page
  `orgs/new.html`, which also works without JavaScript.
- `Cache-Control: no-store` and `Vary: HX-Request`, like `GET /orgs`.
- The form posts with `hx-post="/orgs"` (header claim) and has
  `method="post" action="/orgs"` as the fallback (the submit listener adds
  `_org`). The submit button uses `hx-disabled-elt` so a double click sends
  once.

## What changes, file by file

| File | Change |
|---|---|
| `src/models/organisations.rs` | `NewOrgParams`, shared message constants, `MAX_ORGS_CREATED`, `ORGS_PER_DAY`, `create_for_user`. `create_with_owner` takes `username`. `sign_up` passes `&user.name`. |
| `src/models/memberships.rs` | `create_owner` takes `username`. Remove `switcher_counts`. |
| `src/tasks/super_admin.rs` | Pass `&user.name` to `create_with_owner`. No other change. |
| `src/controllers/orgs.rs` | `GET /orgs/new`, `POST /orgs`. Share the redirect-and-cookies steps with `switch` in one private function, so the two can't drift. |
| `src/controllers/dashboard.rs` | `created` in `HomeQuery`, the toast, `no-store`. |
| `src/extractors/current_member.rs` | `org_menu` always true. Drop the `switcher_counts` call and update the doc comments. |
| `src/models/users.rs`, `src/models/access_tokens.rs` | Only if decision 5 is yes: `lock_account` revokes the user's tokens in every org (`revoke_all_for_user`). |
| `assets/views/orgs/_list.html`, `orgs/index.html` | The "New organisation" row. |
| `assets/views/orgs/new.html`, `orgs/_new_form.html` (new) | The form, per the mockup. |
| `assets/views/layouts/app.html`, `dashboard/index.html`, `more/index.html` | No logic change. The `org_menu` branches now always take the switcher side. Check the mockup for copy changes. |
| `frontend/app.ts` | `?created=` in the `replaceState` list. The dialog or sheet for the form, if the mockup has one. |
| `frontend/styles/app.css` | Styles from the mockup. |
| `tests/...` | See "Tests". |
| `docs/flows/accounts.md` | New "Create another organisation" section with a sequence diagram. "Sign up" points to it for the shared steps. "The switcher" says it always shows. |
| `docs/flows/super-admin.md` | Creating while acting ends acting. |
| `docs/architecture/erd.md` | `organisations.created_by_id` is used for the cap. Remove "Nothing creates a second membership until ..." (around line 173). |
| `docs/plans/multi-org-1.md`, `-2.md`, `-3.md` | No change. Their "Later PRs" sections already describe PR 4. |

No new crate. No migration. `organisations.created_by_id` has no index. The
table is tiny and the counts scan it once per create. Add an index if it
grows.

## Gates

Each must hold before PR 4 merges.

**1. The stale-tab check is live (PR 3).** Not yet. #44 is approved and open.
It must be merged and deployed, and the deploy checked, before this PR
merges. Rebase this branch onto `main` after #44 merges (it is based on the
PR 3 branch today).

**2. The unlock is bound to an org (PR 2).** Met in code. #43 merged on 19
October (`main` at `5d6612f`, CI green). `users::Model::secrets_unlocked`
requires `secrets_unlocked_org_id == Some(org_id)`
(`src/models/users.rs:411`). The Railway deploy of `5d6612f` is unverified
here. Check the Railway dashboard before merging.

**3. Safari and Firefox store the cookie from the banner's `redirect:
"manual"` switch.** Not done. #44 lists it under "Not verified". Raj and the
reviewers require a hand check before merge.

How:

- Playwright 1.55.0 matches the browsers already installed in
  `~/Library/Caches/ms-playwright` (`webkit-2203`, `firefox-1490`). Run it
  with `npx -y playwright@1.55.0`. Do not add it to the repo. The script goes
  in the scratchpad.
- Local app, seeded: Alice in Acme Studio and Northwind Bakery. With this PR
  that can be done through the real form instead of SQL.
- For each of `webkit` and `firefox`, headed so it can be watched:
  1. Sign in. Open page A on Acme `/tasks` and open the new task dialog.
     Type a title.
  2. In page B (same context), switch to Northwind from the menu.
  3. In page A, trigger a request (`htmx.ajax("GET", "/orgs/current")` from
     `page.evaluate`, or wait for the 60-second poll). Expect one banner and
     the title still typed.
  4. Click "Back to Acme Studio". Expect the banner to go.
  5. Read `context.cookies()`: `current_org` must equal Acme's id.
  6. Save the task. Expect it in Acme and not in Northwind (open `/tasks` in
     each org).
- Then by hand in real Safari (installed): the same steps, and check step 5
  in Web Inspector's Storage tab.
- Real Firefox is not installed. Playwright's Firefox is patched but close.
  Decision 7: accept it, or install Firefox for a hand run.
- Record the results in the PR under "Checks", with a screenshot of each
  browser's banner and the cookie.

If a browser does not store the cookie, the banner's Back leaves the tab
stale. The next save is refused, not misfiled, so it fails safe. The fix
would be a full navigation for Back in that browser. That would block the
merge until decided.

**4. A 20 MB refusal on the deployed app.** Not done. It needs #44 deployed.
It can't use a real stale tab in production before this PR, because nobody
there has two orgs. Two checks instead:

- **The probe** (browser): on collab.rajgurung.me, in devtools, set
  `document.body.dataset.orgId` and the `hx-headers` attribute to a wrong
  number, then drop a ~20 MB file. Expect the probe to refuse and send no
  bytes (Network tab).
- **A mid-body refusal** (curl): with your session cookie,
  `curl -i -X POST -H 'HX-Request: true' -H 'X-Org-Id: 1' -F file=@20mb.bin
  https://collab.rajgurung.me/documents/folders/<id>/files`, where `1` is not
  your org. Expect a 409 with `HX-Redirect`. A connection reset instead means
  the Railway edge drops it. That is acceptable (the browser shows "Stopped
  part way" and nothing is stored), but record which one happens.
- Confirm no `documents` row and nothing in storage afterwards.

**5. The rollback window closes.** The PR must say so. Text for the PR body:

> No migration in this PR, and reverting its code is safe. But the first
> organisation created through it gives someone a second membership. From
> then on PR 2's index migration (`memberships_one_per_org`) can't be rolled
> back: its `down` fails with a unique violation and changes nothing. Undoing
> it would mean deleting memberships. PR 2 and PR 3 become forward-only.
> Reverting PR 3 alone would bring back the stale-tab cross-org write for
> anyone with two orgs. The logins unlock migration can still roll back.

Before merging, run the PR 2 duplicate check on production once more. It
should return no rows:

```sql
SELECT organisation_id, user_id, count(*) FROM memberships
GROUP BY 1, 2 HAVING count(*) > 1;
```

**6. Email links 404 between PR 4 and PR 7.** Accepted (PR 2, decision 2).
What people see, once they have two orgs:

- A mention email's item link (`/tasks/5`, `/chat/12`), opened while the
  browser is in the other org, gives a 404. They switch from the menu and
  open the link again. Which 404 page they get is unverified. Check it by
  hand and put it in the PR.
- The mention email's "all notifications" link (`/notifications`) opens the
  browser's current org, not the email's, without saying so.
- A security email's "Login activity" link (`/documents/activity`) also opens
  the browser's current org.
- Who it reaches in PR 4: only people who created a second org. Mentions
  reach them in that org only after someone joins it by link and tags them.
  Security emails reach them as its owner.

PR 7 fixes the links. Ship its email-link part soon after this one.

## One-org assumptions that become reachable

PR 1 removed the code assumption. These are the behaviours that real users can
now meet.

| Place | What happens with two orgs | PR 4 |
|---|---|---|
| `/admin` people table (`default_by_user`) | Shows one row per person with their oldest active org. A creator's second org is not shown there. | Accept. The org list above it shows every org, including created ones. Listing every membership is out of scope. |
| `/admin` org list (`admin_overview`) | Four count queries per org. More orgs, more queries. | Accept. The cap bounds growth. See "Risks". |
| `cargo loco task super_admin` | Uses `default_for_user`. With several orgs it prints the oldest active one. Creates an org only when there is no membership at all. | Signature change only. |
| Mention mailer (`mentions::send_due`) | Groups by `(organisation_id, user_id)`, so one email per org, each naming its org. | Correct. Links: gate 6. |
| Sign out everywhere | Logs `sign_out_everywhere` in every active org, so the created org gets a row too. | Accepted in PR 1. Test it. |
| Password reset owner notices (`tell_owners_about_reset`) | Checked per active org. `to_owners` skips the person themselves. A creator is the only owner of their new org, so it sends nothing extra. An owner of two orgs gets two emails only when someone in both orgs resets, which can't happen before PR 6. Then each email has its own org's details, as PR 1 accepted. | No change. Test the creator case. |
| Account lock (`logins::lock`, `users::Model::lock_account`) | Locks the whole account and signs it out everywhere. Only the org where it happened is told, and only that org's Claude tokens are revoked. The bearer extractor refuses a locked account, so tokens in the other org stop working while locked. But they work again after the reset. A token minted in org B by whoever caused the lock in org A survives. | Decision 5 (recommended: revoke in every org). Who is told stays a PR 5 question. |
| Removal from one org | Still bumps `session_version`, signing the person out of every org. On sign-in they land in their oldest remaining active org. | Accepted until PR 5. |
| Notification stream | Keyed by user. A notice in any org reloads the badge, which counts the current org only. | Accepted. Unread dots per org are deferred. |
| Signing in | `signed_in_cookies` clears `current_org`, so each sign-in starts in the oldest active org, not the newest. | Expected. Mention in `accounts.md`. |

**Data: `organisations.created_by_id`.** Nullable, `ON DELETE SET NULL`.
`create_with_owner` already sets it (`organisations.rs:166`), for signup and
the super admin task alike. The cap relies on it. Before merging, check
production for orgs without one, for information only:

```sql
SELECT count(*) FROM organisations WHERE created_by_id IS NULL;
```

Rows with a null creator just don't count towards anyone's cap.

## Edge cases

- **Double submit.** `hx-disabled-elt` stops most. The user-row lock plus the
  duplicate-name check refuse a second one that gets through. Without
  decision 3, a double submit makes two orgs (`acme`, `acme-2`).
- **Two people create the same name at the same moment.** `unique_slug`
  checks, then inserts. Both can pick the same slug, and one insert fails on
  `organisations.slug` with a 500. Signup has the same race today. Rare.
  Accept, and note it.
- **A name that is only symbols** ("***"): slug `org`, then `org-2`, as in
  signup.
- **A stale tab creates.** The tab was drawn for Acme, and the browser has
  since switched to Beta. `POST /orgs` is refused like any tenant POST. With
  text typed in an HTMX form, the banner keeps it. Nothing is created.
- **Other tabs after creating.** They are on the old org and go stale. Their
  next request moves them to Home in the new org with the PR 3 notice ("This
  tab was showing Acme Studio"). The notice says "You switched organisation
  in another tab". The mockup may want "created" wording here; it is the same
  card.
- **Acting.** A super admin acting in X creates Y. `acting_org` is cleared,
  they land in Y as owner. X's tabs go stale. "Back to X" is not offered in
  the notice unless they are a member of X (PR 3 rule).
- **At the cap, or over the daily limit.** 422 with the message. Nothing is
  saved. The lock means a burst can't slip past.
- **Pending-only user** posts by hand. `CurrentMember` returns the waiting
  page (403). Nothing is created.
- **Locked account.** Has no session (the lock bumps `session_version`), so it
  can't reach the form.
- **No way back.** There is no delete and no leave. The last owner can't be
  removed. A mistaken org stays in the switcher until removed with SQL.
  Decision 6.
- **Claude connector.** Tokens are tied to the org they were granted in. The
  new org has none. An existing connector keeps acting in its own org.
- **No JavaScript.** The page form posts without `_org` and fails closed to
  Home, like every plain form since PR 3. Accepted.
- **Tabs open across the deploy.** Their switcher has no "New organisation"
  row until they reload. Harmless.

## Tests

`tests/models/organisations.rs`:

- `create_for_user_makes_an_active_owner_with_the_chosen_username`: the
  membership's username is the chosen one, not `user.name`.
- `create_for_user_sets_up_general_and_the_starter_folders`: `#general` with
  the owner in it, "Company" and "Logins".
- `create_for_user_sets_created_by`.
- `create_for_user_checks_fields_like_signup`: a 1-character and an
  81-character name, a bad username, `everyone`. Nothing saved.
- `create_for_user_stops_at_the_cap`: with 10 orgs created (backdated past
  the daily window), the 11th is refused.
- `create_for_user_stops_at_the_daily_limit`: three in the last day refuse a
  fourth. Backdate one past 24 hours and the next goes through.
- `create_for_user_refuses_a_name_you_already_have`: "acme " against "Acme".
  Another person's "Acme" does not count (decision 3).
- `two_creates_at_once_do_not_pass_the_cap`: at 9, two creates run together
  with `tokio::join!`. Exactly one succeeds.
- `the_created_membership_is_already_welcomed` (decision 4).
- `sign_up_still_uses_the_account_name_as_the_username`.

`tests/requests/create_org.rs` (new, in `tests/requests/mod.rs`):

- `the_form_renders_with_your_username`.
- `the_form_over_htmx_is_a_fragment`: no `<html>`, `no-store`,
  `Vary: HX-Request`.
- `creating_lands_on_home_in_the_new_org_with_a_toast`: 303 to
  `/dashboard?created=1`. That page has `data-org-id` of the new org, the
  toast with its name, and `no-store`.
- `creating_over_htmx_sends_hx_redirect_to_home`.
- `creating_sets_current_org_and_clears_acting_org`.
- `creating_ends_an_unlock_of_logins`.
- `the_new_org_shows_the_owner_checklist`: "Invite your team" is listed, and
  nothing is done yet.
- `the_new_org_has_no_welcome_pop_up` (decision 4).
- `usernames_are_per_org`: Alice is `alice` in Acme and `ali` in the new org.
  Each org's Members page shows its own.
- `bad_input_is_422_and_saves_nothing`.
- `signup_and_create_give_the_same_errors`: the same bad input to `/signup`
  and `/orgs` gives the same messages.
- `the_cap_is_a_422_and_saves_nothing`.
- `the_daily_limit_is_a_422`.
- `creating_needs_a_signed_in_user`: redirects to `/login`.
- `a_pending_only_user_cannot_create`: the waiting page, no org.
- `an_acting_super_admin_can_create_and_acting_ends` (decision 1).
- `the_created_flag_alone_changes_nothing`: names the current org, sets no
  cookie.
- `creating_ignores_a_next_parameter`.
- `creating_sends_no_email`: the stub mailer has no deliveries.
- `the_new_org_is_separate_from_the_old_one`: an Acme task is a 404 from the
  new org. The new org's task list is empty. Switching back shows Acme's.

`tests/requests/orgs.rs`:

- `one_org_shows_a_plain_name` becomes `one_org_shows_the_switcher`.
- `an_acting_admin_with_no_membership_sees_a_plain_name` becomes
  `an_acting_admin_with_no_membership_sees_the_switcher` (decision 1).
- `htmx_requests_skip_the_switcher_count` is removed with the count.
- `the_org_list_ends_with_new_organisation`: the fragment and the `/orgs`
  page both have the row, linking to `/orgs/new`, outside the switch form.
- `the_admin_page_keeps_its_plain_name`.

`tests/models/memberships.rs`: remove
`switcher_counts_counts_active_and_pending_only`.

`tests/requests/stale_tab.rs`: no edits. The route walks pick up `POST /orgs`
and `GET /orgs/new` by themselves, and must pass with them refused like any
tenant route. `the_exempt_lists_match_real_routes` stays green, since nothing
is added to `EXEMPT`.

`tests/requests/live_sign_out.rs` (real server):

- `creating_an_org_closes_the_chat_socket_and_notification_stream`.

`tests/requests/auth.rs`, `tests/requests/logins.rs`:

- `sign_out_everywhere_logs_in_the_created_org_too`.
- `a_creator_resetting_their_password_gets_no_owner_notice_from_their_own_org`.
- `locking_revokes_claude_tokens_in_every_org` (decision 5).

`tests/requests/multi_org.rs`: `alice_in_two_orgs` keeps inserting by SQL. It
gives Alice a membership in Bob's org, which this PR can't make (that is
PR 6). Leave a comment saying so.

**Not covered by `cargo test`:** the form dialog or sheet in `app.ts`, the
menu keyboard reaching the new row, and the gate 3 and 4 browser checks.

## Out of scope (PRs 5-7)

- **PR 5, org-scoped removal.** Events keyed by org and user. Removal stops
  signing people out of every org. Who is told when a lock in one org locks
  the whole account.
- **PR 6, join with an existing account.** The first way into someone else's
  org with one email. Until then a created org's only member is its creator,
  plus anyone who joins by link with a new account.
- **PR 7, links and the Claude connector.** Org-aware links in emails and
  notifications (gate 6). Connector settings per org.
- **Later, not in a numbered PR:**
  - delete or leave an organisation (decision 6);
  - "Create your own organisation" on the waiting, declined and removed
    pages;
  - a rate limit on `/signup`;
  - the `/admin` people table listing every membership;
  - copying the time zone or other settings from the current org;
  - transferring ownership;
  - unread dots per org in the switcher.

## Risks

- **The rollback window closes** (gate 5). After the first create, PR 2 and
  PR 3 are forward-only. The PR says so.
- **This PR turns PR 1 to 3 on for real users.** Everything they built was
  tested with SQL-made second memberships. The first creators are its first
  real users. Watch the logs for "refused a request from a tab drawn for
  another organisation" in the first days. A burst would mean a client path
  that lost its claim.
- **Browser cookie behaviour** (gate 3). If Safari or Firefox ignores
  `Set-Cookie` on a manual redirect, the banner's Back fails safe but
  confuses people.
- **Email links** (gate 6) mislead people with two orgs until PR 7.
- **Shared code with signup.** `create_with_owner` and `create_owner` change
  signature. The signup tests and a new test pin that signup's behaviour is
  unchanged.
- **`admin_overview`** runs four queries per org. Fine at today's size and
  bounded by the cap. Rewrite it as one grouped query if the org list grows
  into the hundreds.
- **No delete.** Test orgs made by mistake pile up in a person's switcher,
  up to the cap.
- **The slug race** can give a 500 on a simultaneous same-name create. Rare,
  and the same as signup today.

## Checks

```sh
bin/build-assets
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

By hand, locally:

- create an org from the desktop menu, the phone sheet and the no-JS `/orgs`
  page;
- the toast, the checklist and no welcome pop-up on Home;
- the form's errors: short name, bad username, `everyone`, duplicate name,
  the cap (set `created_at` back by SQL to reach it quickly);
- a second tab on the old org moves to Home with the notice;
- the chat socket in another tab closes and reconnects in the new org;
- create while acting: acting ends, and the banner is gone;
- the menu keyboard reaches "New organisation";
- gate 3 (Playwright WebKit and Firefox, then real Safari);
- the 404 page an old-org task link gives in the new org (gate 6).

On production, before merging: the duplicate check (gate 5), the
`created_by_id` count, the Railway deploy of #43 and #44 (gates 1 and 2), and
gate 4 once #44 is live.

After merging: create one org on production as Raj, switch back and forth,
and confirm the logs show the create and the switches.

## Screenshots needed

From the local app with realistic data: the mockup's people and orgs (Acme
Studio, Northwind Bakery), with members, tasks and chat in Acme. Store them on
the `pr-screenshots` branch under `multi-org-4/`. Desktop 1440px and phone
390px, light and dark:

- the switcher with one org and "New organisation" (desktop menu, phone
  sheet, More's row);
- the switcher with two orgs and the row;
- the create form, empty and with the username filled in;
- the form with a field error, and with the cap message;
- Home in the new org: the toast and the checklist;
- another tab's notice after the create;
- the menu and form while acting, if decision 1 is yes;
- gate 3: the banner and the stored cookie in WebKit, Firefox and Safari.

## Decisions for Raj

Each with a recommendation.

1. **May a super admin create while acting?** Recommend: yes. Creating ends
   acting, as a switch does.
2. **Limits.** Recommend: 10 orgs created per person, ever, including the
   signup one, and 3 per 24 hours. Counted from `organisations.created_by_id`.
   The same for the super admin.
3. **Refuse a name you already have** (among your own active orgs,
   case-insensitive). Recommend: yes. Two identical rows in the switcher can't
   be told apart, and it stops double submits.
4. **Skip the welcome pop-up in a created org,** but keep the checklist.
   Recommend: skip. The person has seen the tour, and it would cover the
   toast.
5. **On an account lock, revoke Claude tokens in every org,** not only the
   one where the lock happened. Recommend: yes, in this PR. It is one query
   change, and this PR is what makes a second org's tokens possible.
6. **No delete or leave for a created org.** Recommend: accept for PR 4, with
   the cap as the bound. Plan "delete an empty org" as a small later PR.
7. **Firefox check.** Recommend: Playwright's Firefox 1.55.0 build counts as
   the Firefox check, with real Safari checked by hand. Or install Firefox
   for a hand run, if the reviewers insist on a release build.
