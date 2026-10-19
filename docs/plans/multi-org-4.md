# Multi-org 4: create another organisation

Status: Approved by Raj 19 Oct; reviews folded in (staff: approve with changes, applied; security: approve with changes, applied) · PR 4 of 7 · Owner: Raj · 19 October 2026

## Summary

PR 3 (#44, merged as `44db5ab`) added the switcher and the stale-tab check.
Nobody can have two organisations yet, except by SQL. This PR lets a signed-in
person create another organisation from the switcher.

- `GET /orgs/new` shows the form. `POST /orgs` creates the organisation with
  the person as its active owner, under the username they choose for it.
- It uses signup's code to set the organisation up: the slug, `#general` with
  the owner in it, and the "Company" and "Logins" folders. One code path.
- Two fields, the organisation name and your username there, checked as on
  `/signup`.
- After creating, the browser switches into the new organisation the way
  `POST /orgs/{id}/switch` does. It lands on Home with "Lumen Café is ready.
  You're the owner." The "Getting started" checklist shows. The welcome
  pop-up does not.
- The switcher shows for every member who is not acting, even with one
  organisation, and ends with a "New organisation" row.
- Limits: 10 organisations created per person, ever, and 3 per 24 hours.
- An account lock now revokes the person's Claude tokens in every org.

No migration. This is the first PR that gives anyone a second membership in
production, so PR 2 and PR 3 become forward-only once it ships (see gate 5).

The design is `docs/designs/create-org.html` (`d835640`), approved by Raj on
19 October with every recommendation. This plan covers what the server and
`app.ts` must do. Layout and copy are the mockup's. Section numbers below are
the mockup's.

**One deviation from the mockup.** Its question 10 recommends running the
create as `CurrentUser`, exempt from the stale-tab check. Both reviewers
recommend `CurrentMember` instead, and Raj agreed. See "Who may create".

## Who may create

**Any active member, through `CurrentMember` on both `GET /orgs/new` and `POST
/orgs`. Both stay inside the stale-tab check, and neither is added to
`EXEMPT`.**

Why `CurrentMember`, not the mockup's `CurrentUser`:

- It keeps creation to active members. Someone with no active membership
  (pending, declined, removed) sees their waiting or removed page, as today.
- It adds no new exemption to the stale-tab check. Every exemption is a route
  the walk test has to explain.
- The claim check is a third CSRF layer, after `SameSite=Lax` and
  `origin_check`.
- A 422 redraws the form inside the app layout, which needs the member.

The cost: a stale tab's create is refused and sent Home, or keeps its typed
text behind the banner. Nothing is lost and nothing is misfiled.

**The no-JS form carries its claim.** The form renders `<input type="hidden"
name="_org" value="{{ current_org_id }}">` on the server. `app.ts`'s submit
listener overwrites it, as it does for other forms. Without JavaScript the
page form at `/orgs/new` still passes the check, instead of failing closed
like other plain forms. A test pins it.

**Super admin while acting** (mockup section 4):

- No "New organisation" row in the menu, the sheet or More while acting.
  That menu is about where they are acting and the way back. A create row
  there could read as "create inside Acme".
- `org_menu` is true for every member who is not acting. While acting it
  stays as in PR 3: `switcher_counts` runs only then, and the switcher shows
  only with a membership of their own.
- `/orgs/new` typed by hand works. The form first shows the acting note:
  "You're acting in Acme Studio. Creating this ends acting. The new
  organisation is yours, with you as owner."
- Creating clears `acting_org`, as a switch does, and lands them in their own
  new org as owner.
- `create_for_user` takes only `member.user` and the params. It never reads
  `member.org`, `member.role` or `member.username`. The one thing taken from
  the current org is the time zone, and never while acting (see "What happens
  on create"). The username prefill on the GET is the only other use of
  `member`.
- A super admin with no membership at all still goes to `/admin` from any
  tenant page. They can only reach the form while acting.

## Fields and validation

The same rules and messages as signup (mockup section 2, "Validation").

| Field | Form name | Rule | Message |
|---|---|---|---|
| Organisation name | `organisation_name` | 2 to 80 characters after trimming | "Use 2 to 80 characters." |
| Your username there | `name` | `users::USERNAME_RE` and `users::not_reserved` | "Start with a letter and use 3 to 30 letters or numbers." and "That name is reserved." |

- `name` matches signup and join, so `field_errors` keys and the templates'
  error slots work the same way.
- The username is always free in the new org, since it has only its owner.
  `idx-memberships-org-username` still backs that. No extra query.
- The form starts with the username in the current org (`member.username`;
  `user.name` while acting), with the hint "How people in this organisation
  see you."

**The params.** A new `organisations::NewOrgParams { organisation_name, name
}`. Its `#[validate]` attributes copy `SignupParams`, with the messages
written out as literals in each struct, as `users.rs` does for
`RegisterParams`. No shared constants: the validator derive may need literals
in `message = ...`, and two short copies are easier to read.
`signup_and_create_give_the_same_errors` keeps the copies honest.

**A spaces-only name.** Today `length(min = 2, max = 80)` counts untrimmed
characters, so "   " passes validation and is saved as an empty name. Replace
it with a custom validator, `organisations::trimmed_name_length`, which checks
2 to 80 characters of `name.trim()` with the same message. Use it in both
`NewOrgParams` and `SignupParams`. This fixes signup too. Tests cover both.

**Duplicate names** (mockup section 2):

- Refused when the name matches one of your active organisations. Names stay
  free across the platform; a stranger's "Lumen Café" doesn't count.
- Matching: trim, collapse runs of whitespace to one space, then Unicode
  lowercase (`str::to_lowercase`). Done in Rust, after the user-row lock,
  over `memberships::Model::for_user_with_orgs`, keeping the active rows.
- The message uses the stored name: "You already have an organisation called
  Acme Studio. Pick a name you can tell apart."
- With the lock, this also stops a double submit that gets past the locked
  button from making a second org.

## Abuse limits

**The cap and the rate limit are counted from `organisations`, not
`auth_attempts`.**

- Every creation is already a row with `created_by_id` and `created_at`.
  Both limits are counts on rows that already exist.
- `auth_attempts` is for signed-out flows (sign-in, forgot, reauth). It is
  keyed by email hash and IP, and rows older than 24 hours are deleted on
  every write (`KEEP_HOURS`), so it could not hold a lifetime cap.
- `MAX_ORGS_CREATED = 10` per person, counting the signup org.
  `ORGS_PER_DAY = 3` in any 24 hours. Constants in
  `src/models/organisations.rs`. The super admin gets the same limits.
- Counting by `created_by_id` means the cap can't be dodged by leaving an
  org. It is set to `NULL` if the user is deleted, which frees the count. The
  account is gone by then.

**The lock.** The first statement in the create transaction locks the
person's `users` row: `users::Entity::find_by_id(id).lock(LockType::NoKeyUpdate)`
(`SELECT ... FOR NO KEY UPDATE`; `LockType::NoKeyUpdate` is in sea-query
1.0.2). `FOR NO KEY UPDATE`, not `FOR UPDATE`, because it doesn't conflict
with the `FOR KEY SHARE` locks other transactions take when they insert rows
pointing at this user (messages, notes and so on). Then, as separate
statements: the all-time count, the 24-hour count and the duplicate check.
Two creates at once from one person run one after the other, and the second
sees the first.

**At the cap** (mockup section 4, "A limit"):

- `GET /orgs/new` shows the at-cap state in place of the fields: "You've set
  up 10 organisations" and "That's the most one account can set up for now.
  You can keep using the ones you have." No contact is named. The row still
  shows in the switcher.
- A `POST` at the cap (a form opened earlier, or a typed request) is a 422
  with the same state.

**The daily limit** is only checked on `POST`: a 422 with the field error
"You've set up 3 organisations today. Try again tomorrow." on
`organisation_name`.

Both are 422s, not 429s. `base.html`'s `responseHandling` only swaps 422s,
so a 429 would show nothing in the dialog.

**No per-IP limit.** The person is signed in, and `/signup` itself has no
rate limit today, so anyone can already make accounts and orgs without one.
The cap stops one account piling up orgs. A `/signup` limit is deferred.

**Emails and notifications on creation: none.**

- Signup sends nothing today: no welcome and no verification email
  (`signup::create` and `organisations::Model::sign_up` call no mailer).
  Creating an org sends nothing either.
- A new org has one member, so nobody can be mentioned or notified.
  `SecurityMailer::to_owners` would only reach the creator.
- Join requests email nobody, so a new org's join link can't be used to
  email its owner.
- So there is no per-org email or notification spam path in this PR. The cap
  bounds how many join links and `#general` channels one account can make.

## What happens on create

`POST /orgs`, in `src/controllers/orgs.rs`:

1. `member: CurrentMember` (the claim check runs here), `Form<NewOrgParams>`.
2. `organisations::Model::create_for_user(&ctx.db, &member.user, &params,
   timezone)`. `timezone` is `Some(&member.org.timezone)` when not acting,
   and `None` while acting.
   1. `ValidatorTrait::validate(params)`.
   2. Begin a transaction. Lock the user row (above), as the first statement.
   3. The all-time count, then the 24-hour count. Over either: the at-cap
      state or the daily field error.
   4. The duplicate-name check.
   5. `create_with_owner(&txn, &params.organisation_name, user,
      params.name.trim())`.
   6. If `timezone` is set, update the new org's `timezone` to it. Otherwise
      it keeps the column default, `Europe/London`, as signup does.
   7. `memberships::Model::mark_welcomed(&txn, org.id, user.id)`.
   8. Commit.
3. On a field error or the cap: 422, the form redrawn with values and errors.
   Over HTMX, the fragment `orgs/_new_form.html`; otherwise the page
   `orgs/new.html`.
4. On success, the shared private function `enter_org(ctx, headers, user,
   org_id, target)`, which `switch` also calls:
   - `users::Model::end_unlock`;
   - `redirect_response(&headers, target)`: `HX-Redirect` over HTMX, a 303
     otherwise;
   - `current_org_cookie(&ctx, org_id)` and `cleared_acting_org_cookie`;
   - `users::Model::close_live_connections`.
   The target is the constant `/dashboard?created=1`. There is no `next`.
5. An info log line, numbers only: `user_id`, `org_id`, `from_org` (the
   request's `current_org` cookie, logged only if it parses as an `i64`, as
   in `switch`) and, when acting, `acting_org` (`member.org.id`).
   "created organisation".

`create_with_owner` gains a `username: &str` and passes it to
`memberships::Model::create_owner`, which stops copying `user.name`. Signup and
the super admin task pass `&user.name`, so they behave exactly as today.
Signup keeps the `Europe/London` default. This is the one code path:
anything signup sets up later is set up here too.

**Why copy the time zone** (mockup section 2): most people start a second org
in the same place. Owners can change it on Members. Not while acting, because
the acting org belongs to someone else and its settings shouldn't carry into
the admin's own org.

### Errors in the dialog (mockup section 4, "Failures")

- 422: the fragment swaps in, keeps what was typed, focuses the first field
  with a problem and marks it `aria-invalid`.
- 5xx: `app.ts`, on `htmx:responseError` from the form, shows the mockup's red
  alert at the top of the form: "Something went wrong, and nothing was
  created. Try again." The fields stay filled. The transaction means nothing
  was saved.
- No connection: on `htmx:sendError`, "Couldn't create the organisation.
  Check your connection and try again." The form stays open and filled.
  Built in the alert slot, not as a toast: a modal sheet sits in the top
  layer, above any toast (review fix).
- While the request runs, the button reads "Creating…" with a spinner and
  both fields lock (`hx-disabled-elt`). A double tap sends once.

### Home after creating (mockup section 3)

- `dashboard::HomeQuery` gets `created: Option<String>`. The toast "Lumen
  Café is ready. You're the owner." is built from `member.org.name`, the org
  this request resolved, never from the URL. Tera escapes it.
- `Cache-Control: no-store` when `created` is set, as for `switched` and
  `from`.
- `app.ts` adds `created` to the `replaceState` block next to `switched` and
  `from` (`frontend/app.ts` ~line 208).
- `/dashboard?created=1` typed by hand names your current org and changes
  nothing.
- **Checklist.** Shows by itself: the new membership has `guide_dismissed_at`
  null, and the owner gets the five team-setup steps.
- **Welcome pop-up.** Skipped, because step 2.7 sets `welcomed_at`. The
  person already uses the app, and the tour would sit on top of the toast.
  Signup and join keep it.

## The switcher (mockup section 1)

- **Shows for every member who is not acting.** In `PinnedMember`,
  `org_menu = true` when not acting, with no query. While acting it keeps
  PR 3's rule, and `switcher_counts` runs only then. That saves one query per
  full page load for everyone else.
- The `org_menu` key and the template branches stay. `/admin` renders
  `app.html` without it and keeps its plain "Platform admin" name. An acting
  admin with no membership keeps the plain name too.
- **"New organisation" row**: after the list and any pending rows, under a
  hairline, outside the scrolling part, so it stays in view with nine orgs. A
  dashed square with a plus. A link to `/orgs/new`, outside the switch
  `<form>`, with `role="menuitem"` (not `menuitemradio`). `orgItems` in
  `app.ts` already picks up `[role^=menuitem]`, so arrow keys and the letter
  "N" reach it.
- Desktop: the row opens the form in the existing `dialog.sheet`, centred at
  560px. Focus starts in "Organisation name". Esc and Cancel close it and put
  focus back on the switcher button.
- Phone: tapping the row swaps the sheet's content to the form, with a back
  arrow to the list. No second sheet. The button sits in
  `.sheet-sticky-foot` above the keyboard.
- More: the "Organisation" section shows for everyone. With one org, the
  first row names your org and role, with no "Switch". The second row is
  "New organisation". With two or more, "Switch" comes back.
- The row is left out while acting (`{% if not me.acting %}`), in
  `_list.html` and on More.

### `GET /orgs/new`

- `CurrentMember`. Over HTMX, the fragment `orgs/_new_form.html` for the
  dialog or sheet. Otherwise the full page `orgs/new.html`: the no-JS
  fallback and the target of a typed link.
- `Cache-Control: no-store` and `Vary: HX-Request`, like `GET /orgs`.
- Shows the at-cap state instead of the fields when the person is at 10.
  This is a plain count with no lock. The `POST` checks again.
- Shows the acting note while acting.
- The form has `hx-post="/orgs"` and `method="post" action="/orgs"`, with the
  server-rendered `_org`.

## What changes, file by file

| File | Change |
|---|---|
| `src/models/organisations.rs` | `NewOrgParams`, `trimmed_name_length`, `MAX_ORGS_CREATED`, `ORGS_PER_DAY`, `create_for_user`, a `created_count` helper for the GET. `SignupParams` uses `trimmed_name_length`. `create_with_owner` takes `username`; `sign_up` passes `&user.name`. |
| `src/models/memberships.rs` | `create_owner` takes `username`. `switcher_counts` stays (used while acting). |
| `src/models/access_tokens.rs` | `revoke_all_for_user(db, user_id)`: revokes every token and unused OAuth code the person has, in every org. |
| `src/models/users.rs` | `lock_account` calls `revoke_all_for_user` instead of `revoke_all_for`. Its doc comment says tokens in every org are revoked; who is told is unchanged. |
| `src/tasks/super_admin.rs` | Passes `&user.name` to `create_with_owner`. |
| `src/controllers/orgs.rs` | `GET /orgs/new`, `POST /orgs`, and `enter_org`, shared with `switch`. |
| `src/controllers/dashboard.rs` | `created` in `HomeQuery`, the toast, `no-store`. |
| `src/extractors/current_member.rs` | `org_menu` true when not acting; `switcher_counts` only while acting. Doc comments. |
| `assets/views/orgs/_list.html`, `orgs/index.html` | The "New organisation" row, not while acting. |
| `assets/views/orgs/new.html`, `orgs/_new_form.html` (new) | The form: fields, hints, the two plain facts, the acting note, the at-cap state, the hidden `_org`, the error alert slot. |
| `assets/views/more/index.html` | The Organisation section for everyone, the one-org row without "Switch", and "New organisation" (not while acting). |
| `assets/views/layouts/app.html`, `dashboard/index.html` | No logic change. The `org_menu` branches now take the switcher side for members who are not acting. |
| `frontend/app.ts` | Open the form in the dialog or sheet, the sheet's back arrow, focus handling, the 5xx alert, the offline toast, `?created=` in `replaceState`. Uploads: on a 502 or network error, probe `/orgs/current` and run `staleRefusal` on a 409 before showing the upload error (gate 4). |
| `frontend/styles/app.css` | Styles from the mockup: the dashed row, the at-cap box, the alert. |
| `tests/...` | See "Tests". |
| `docs/flows/accounts.md` | A new "Create another organisation" section with a sequence diagram. "Sign up" points to it for the shared steps and notes the trimmed-length fix. "The switcher" says it shows for everyone. Sign-in starts in the oldest org. |
| `docs/flows/super-admin.md` | No create row while acting; a typed link works and ends acting. |
| `docs/flows/documents.md` | An account lock revokes Claude tokens in every org. |
| `docs/architecture/erd.md` | `organisations.created_by_id` backs the cap. Remove "Nothing creates a second membership until ..." (around line 173). |

No new crate. No migration. `organisations.created_by_id` has no index. The
table is tiny and the counts scan it once per create. Add one if it grows.

## Gates

**Production checks need a signed-in session.** The production steps in
gates 1, 3 and 4 are run by Raj, or through Raj's browser. No agent holds
production session cookies. Never paste a session cookie into the PR, logs
or screenshots.

**1. The stale-tab check is live (PR 3).** Met. #44 merged as `44db5ab` and
deployed (Railway: SUCCESS). Production is served at collab.zen-scale.io;
collab.rajgurung.me 308-redirects there. This branch is rebased onto
`44db5ab`.

Checked on production on 19 October, in Raj's signed-in Chrome:

- A `POST /tasks` with `X-Org-Id: 999999` got a 409, `HX-Redirect:
  /dashboard?from=999999` and `Cache-Control: no-store`.
- The `GET /orgs/current` probe gives a 409 with a wrong id and a 204 with
  the right one, both `no-store`.

**Pass.**

**2. The unlock is bound to an org (PR 2).** Met. #43 merged on 19 October.
`users::Model::secrets_unlocked` requires `secrets_unlocked_org_id ==
Some(org_id)` (`src/models/users.rs:411`). The migration is applied:
production boots with `auto_migrate` on (`config/production.yaml:113`,
`DB_AUTO_MIGRATE` defaults to true), which runs pending migrations at boot
and stops the app starting if one fails. A running app on `44db5ab` proves it
ran.

**3. Safari and Firefox store the cookie from the banner's `redirect:
"manual"` switch.** Required before merge.

- Locally, Playwright 1.55.0, which matches the browsers already in
  `~/Library/Caches/ms-playwright` (`webkit-2203`, `firefox-1490`). Run with
  `npx -y playwright@1.55.0`. Not added to the repo; the script lives in the
  scratchpad.
- Local app, seeded: Alice in Acme Studio and Northwind Bakery, the second
  made through the new form.
- For each of `webkit` and `firefox`, headed:
  1. Sign in. Page A on Acme `/tasks`; open the new task dialog and type a
     title.
  2. Page B in the same context: switch to Northwind from the menu.
  3. Page A: `htmx.ajax("GET", "/orgs/current")` through `page.evaluate`, or
     wait for the 60-second poll. Expect one banner, with the title still
     typed.
  4. Click "Back to Acme Studio". Expect the banner to go.
  5. `context.cookies()`: `current_org` equals Acme's id.
  6. Save the task. Expect the save to be accepted (no refusal, no banner),
     and the task in Acme and not in Northwind (open `/tasks` in each org).
- Then by hand in real Safari (installed, required): the same six steps,
  reading step 5 in Web Inspector's Storage tab.
- Real Firefox is not installed. Playwright's Firefox build counts as the
  Firefox check.
- Right after merge, by Raj: one real-Safari pass of the same steps on
  production, with his own two orgs. Recorded in the PR.
- Record each result in the PR under "Checks", with a screenshot of the
  banner and the cookie panel (cookie values hidden).

If a browser doesn't store the cookie, the next save is refused, not
misfiled. It fails safe, but it blocks the merge until decided.

**4. A 20 MB refusal on the deployed app.** Safe, with one gap that this PR
closes. Checked on production on 19 October, in Raj's signed-in Chrome, with
wrong-org multipart uploads to `/documents/folders/1/files`:

- **Nothing stored.** The folder held 6 files before and after every
  attempt.
- **At 1 KB** the edge passes the app's 409 with `HX-Redirect` through.
- **At 256 KB, 2 MB, 8 MB and 20 MB**, Railway's edge (`server:
  railway-hikari`) returns a **502** within about 50 ms, not the 409. The
  app refuses before reading the body, and the edge reports the early close
  as a bad gateway.
- **The effect.** A stale upload that gets past the pre-upload probe shows
  the generic upload error ("Stopped part way") instead of the banner. That
  can only happen in the short window between the probe and the upload
  starting. Nothing leaks and nothing is stored.

**The fix, required in this PR (`frontend/app.ts`).** When an upload fails
with a 502 or a network error (status 0), run the `GET /orgs/current` probe
before showing the upload error. If the probe gets a 409 with
`HX-Redirect`, hand it to `staleRefusal`, which shows the banner (the queue
counts as unsaved work) or goes Home. Only if the probe passes does the
upload show "Stopped part way". The probe adds one small request, and only
after a failure.

**Rejected: draining the body on the server** so a real 409 gets through the
edge. The app would have to read and throw away the whole upload before
answering, up to 25 MB per refusal. That is the cost the early refusal
exists to avoid.

To re-check after the fix, by Raj on production: in devtools, make the
probe pass and the upload fail (block `/orgs/current` once with request
blocking, or edit the claim between the probe and the send), then drop a
2 MB file. Expect the banner, not "Stopped part way", and still nothing
stored.

**5. The rollback window closes.** Text for the PR body:

> No migration in this PR. Reverting its code is safe for data. But the first
> organisation created through it gives someone a second membership, and
> from then on:
>
> - PR 2's index migration (`memberships_one_per_org`) can't be rolled back.
>   Its `down` fails with a unique violation and changes nothing. Undoing it
>   would mean deleting memberships.
> - PR 2's unlock migration (`secrets_unlocked_org_id`) must not be rolled
>   back either. Without the column, an unlock in one org would count in
>   another, which brings back the cross-org reveal.
> - PR 3 must not be reverted. That would bring back the stale-tab cross-org
>   write for anyone with two orgs. Redeploying any Railway image older than
>   `44db5ab` is the same as reverting PR 3.
>
> Reverting PR 4 itself brings back the one-org-only token revoke on an
> account lock.
>
> PR 2 and PR 3 are forward-only from here.

The PR 2 duplicate-row query is not needed. The live unique index
`idx-memberships-org-user` on `(organisation_id, user_id)` already guarantees
there are no duplicates.

**6. Email links 404 between PR 4 and PR 7.** Accepted (PR 2, decision 2).
What people see once they have two orgs:

- A mention email's item link (`/tasks/5`, `/chat/12`), opened while the
  browser is in the other org, gives a 404. They switch from the menu and
  open the link again. Done in this PR, not deferred to PR 7: checked by
  hand, it was Loco's raw JSON body, so a page load's 404 is now an app
  page (`src/app.rs`, `not_found_page`): "We couldn't find that in
  {current org}. It may be in another of your organisations. Switch from
  the menu, then open the link again." with "Go to Home". It names only the
  current org and never looks up where the item is. HTMX and API 404s are
  unchanged.
- The mention email's "all notifications" link (`/notifications`) opens the
  browser's current org, not the email's, without saying so.
- A security email's "Login activity" link (`/documents/activity`) also opens
  the current org.
- Who it reaches in PR 4: only people who created a second org. Mentions
  reach them there only after someone joins it by link and tags them.
  Security emails reach them as its owner.

PR 7 fixes the links. Ship its email-link part soon after this one.

## One-org assumptions that become reachable

| Place | With two orgs | PR 4 |
|---|---|---|
| `/admin` people table (`default_by_user`) | One row per person, with their oldest active org. A creator's second org isn't shown there. | Accept. The org list above shows every org. |
| `/admin` org list (`admin_overview`) | Four count queries per org. | Accept; the cap bounds growth. A grouped query is deferred. |
| `cargo loco task super_admin` | `default_for_user`: prints the oldest active org. Creates one only when there is no membership. | Signature change only. |
| Mention mailer (`mentions::send_due`) | Groups by `(organisation_id, user_id)`: one email per org, each naming its org. | Correct. Links: gate 6. |
| Sign out everywhere | Logs `sign_out_everywhere` in every active org, the created one too. | Accepted in PR 1. Tested. |
| Password reset owner notices | Checked per active org. `to_owners` skips the person. A creator is the only owner of their new org, so nothing extra is sent. One owner getting two emails needs someone in both of their orgs, which waits for PR 6. | No change. Tested. |
| Account lock | Locks the whole account and signs it out everywhere. Before this PR only the lock's org had its Claude tokens revoked. Tokens in the other org were refused while locked, then worked again after the reset, including any minted by whoever caused the lock. | Fixed: revoked in every org. Who is told stays a PR 5 question. |
| Removal from one org | Still bumps `session_version`, signing the person out of every org. Next sign-in lands in their oldest remaining active org. | Accepted until PR 5. |
| Notification stream | Keyed by user. A notice in any org reloads the badge, which counts the current org only. | Accepted. Unread dots deferred. |
| Signing in | `signed_in_cookies` clears `current_org`, so each sign-in starts in the oldest active org, not the newest. | Expected. Written in `accounts.md`. |

**Data: `organisations.created_by_id`.** Nullable, `ON DELETE SET NULL`.
`create_with_owner` already sets it (`organisations.rs:166`), for signup and
the super admin task alike. The cap relies on it. Rows with a null creator
count towards nobody's cap. For information before merging (Raj):

```sql
SELECT count(*) FROM organisations WHERE created_by_id IS NULL;
```

## Edge cases

- **Double submit.** The button locks. One that gets through waits on the
  user-row lock, then meets the duplicate-name check.
- **Two tabs create different names at once.** Two orgs, two toasts. The
  lock makes both counts see each other.
- **Two people create the same name at the same moment.** `unique_slug`
  checks, then inserts. Both can pick the same slug, and one insert fails on
  `organisations.slug` with a 500. The dialog shows "Something went wrong,
  and nothing was created. Try again.", and a retry gets `-2`. Signup has the
  same race today. An advisory lock is deferred.
- **Accents in the name.** Kept in the name. The slug drops them today:
  "Lumen Café" becomes `/join/lumen-caf`. Transliteration is deferred.
- **A name that is only symbols** ("***"): slug `org`, then `org-2`.
- **A spaces-only name.** Refused with "Use 2 to 80 characters." on both
  signup and create.
- **Markup in a name** (`<script>`). Stored as typed. Tera autoescapes it in
  the toast, the switcher, More and `/admin`. Tested.
- **A stale tab creates.** Refused like any tenant POST. With text typed in
  the dialog, the banner keeps it. Nothing is created.
- **Other tabs after creating.** They are on the old org, go stale, and move
  to Home in the new org with PR 3's notice. Nothing new to build.
- **Acting.** No row. A typed `/orgs/new` shows the note. Creating ends
  acting and makes the org the admin's own, with the default time zone. The
  acting org's tabs go stale. "Back to X" is offered only if they are a
  member of X (PR 3 rule).
- **At the cap.** The GET shows the at-cap state. A POST gets the same, as a
  422. Nothing saved.
- **Over the daily limit.** A 422 with the daily message. Nothing saved.
- **Pending-only user** posting by hand: `CurrentMember` returns the waiting
  page (403). Nothing created.
- **Locked account.** No session (the lock bumps `session_version`), so no
  form.
- **No way back.** No delete and no leave, and the last owner can't be
  removed. A mistaken org stays in the switcher until removed with SQL. Raj
  accepted this for PR 4, with the cap as the bound.
- **Claude connector.** Tokens are tied to the org they were granted in. The
  new org has none. An existing connector keeps acting in its own org.
- **No JavaScript.** The `/orgs/new` page form carries its server-rendered
  `_org` and works.
- **Tabs open across the deploy.** No row until they reload. Harmless.

## Tests

`tests/models/organisations.rs`:

- `create_for_user_makes_an_active_owner_with_the_chosen_username`: the
  membership's username is the chosen one, not `user.name`.
- `create_for_user_sets_up_general_and_the_starter_folders`: `#general` with
  the owner in it, "Company" and "Logins".
- `create_for_user_sets_created_by`.
- `create_for_user_checks_fields_like_signup`: 1 and 81 characters, a bad
  username, `everyone`. Nothing saved.
- `a_spaces_only_name_is_refused`: "   " and " a ", for both `NewOrgParams`
  and `SignupParams`.
- `create_for_user_stops_at_the_cap`: with 10 created (backdated past the
  daily window), the 11th is refused.
- `create_for_user_stops_at_the_daily_limit`: three in the last day refuse a
  fourth with the daily message. Backdate one past 24 hours and the next goes
  through.
- `create_for_user_refuses_a_name_you_already_have`: "  acme   studio " and
  "ACME STUDIO" against "Acme Studio", and "ÉCOLE" against "école". The
  message names the stored "Acme Studio". Another person's org doesn't
  count, nor does one you are only pending in.
- `two_creates_at_once_do_not_pass_the_cap`: at 9, two creates run with
  `tokio::join!`, one on `ctx.db` and one on `second_connection(&ctx)`, as in
  `tests/requests/oauth.rs:511-523`. The test pool has one connection.
  Exactly one succeeds.
- `create_for_user_copies_the_given_time_zone`: `Some("Asia/Kathmandu")`
  gives that zone.
- `create_for_user_without_a_time_zone_keeps_the_default`: `None` gives
  `Europe/London`.
- `the_created_membership_is_already_welcomed`.
- `sign_up_still_uses_the_account_name_as_the_username_and_the_default_zone`.

`tests/requests/create_org.rs` (new, in `tests/requests/mod.rs`):

- `the_form_renders_with_your_username_and_the_org_field`: prefilled
  username, and a hidden `_org` with the current org id.
- `the_form_over_htmx_is_a_fragment`: no `<html>`, `no-store`,
  `Vary: HX-Request`.
- `the_form_at_the_cap_shows_the_limit_instead_of_fields`.
- `creating_lands_on_home_in_the_new_org_with_a_toast`: 303 to
  `/dashboard?created=1`. That page has the new org's `data-org-id`, "… is
  ready. You're the owner." and `no-store`.
- `creating_over_htmx_sends_hx_redirect_to_home`.
- `a_plain_form_post_with_org_creates`: a urlencoded POST with `_org` and no
  header creates the org (the no-JS path).
- `creating_sets_current_org_and_clears_acting_org`.
- `creating_ends_an_unlock_of_logins`.
- `creating_copies_the_current_org_time_zone`: Acme on `Asia/Kathmandu`
  gives a new org on `Asia/Kathmandu`.
- `the_new_org_shows_the_owner_checklist`: "Invite your team" listed,
  nothing done yet.
- `the_new_org_has_no_welcome_pop_up`.
- `usernames_are_per_org`: Alice is `alice` in Acme and `ali` in the new org.
  Each org's Members page shows its own.
- `bad_input_is_422_and_saves_nothing`.
- `signup_and_create_give_the_same_errors`: the same bad input, including a
  spaces-only name, to `/signup` and `/orgs` gives the same messages.
- `the_cap_is_a_422_and_saves_nothing`.
- `the_daily_limit_is_a_422_with_its_message`.
- `creating_needs_a_signed_in_user`: redirects to `/login`.
- `a_pending_only_user_cannot_create`: the waiting page, no org.
- `acting_shows_no_new_organisation_row`: the menu fragment and More, while
  acting.
- `acting_can_create_from_a_typed_link_and_acting_ends`: the form shows the
  acting note. The create clears `acting_org`. The new org has the admin as
  owner, `Europe/London`, and nothing from the acting org. The acting org's
  members are unchanged.
- `the_created_flag_alone_changes_nothing`: names the current org, sets no
  cookie.
- `creating_ignores_a_next_parameter`.
- `creating_sends_no_email`: the stub mailer has no deliveries.
- `the_new_org_is_separate_from_the_old_one`: from the new org, an Acme task
  is a 404 and the task list is empty; documents has only the two starter
  folders, each empty; logins has none; Members lists only the owner; login
  activity is empty; chat has only `#general`. Switching back shows Acme's.
- `org_names_are_escaped_everywhere`: a name with
  `<script>alert(1)</script>` appears escaped in the toast, the switcher
  fragment, More and `/admin`, and never raw.

`tests/requests/orgs.rs`:

- `one_org_shows_a_plain_name` becomes `one_org_shows_the_switcher`.
- `an_acting_admin_with_no_membership_sees_a_plain_name`: kept, unchanged.
- `htmx_requests_skip_the_switcher_count`: kept, since the count still runs
  while acting.
- `the_org_list_ends_with_new_organisation`: the fragment and the `/orgs`
  page both have the row, linking to `/orgs/new`, outside the switch form.
- `the_admin_page_keeps_its_plain_name`.

`tests/models/memberships.rs`: `switcher_counts_counts_active_and_pending_only`
stays.

`tests/requests/stale_tab.rs`: no edits. The walks pick up `POST /orgs` and
`GET /orgs/new` and must refuse them like any tenant route. Nothing is added
to `EXEMPT`.

`tests/requests/live_sign_out.rs` (real server):

- `creating_an_org_closes_the_chat_socket_and_notification_stream`.

`tests/requests/auth.rs`, `tests/requests/logins.rs`:

- `sign_out_everywhere_logs_in_the_created_org_too`.
- `a_creator_resetting_their_password_gets_no_owner_notice_from_their_own_org`.
- `locking_revokes_claude_tokens_in_every_org`: tokens and an unused OAuth
  code in Acme and in the created org. Five wrong app passwords in Acme. Both
  orgs' tokens are revoked and the code is used. After a reset, neither token
  works.

`tests/requests/multi_org.rs`: `alice_in_two_orgs` keeps inserting by SQL.
It puts Alice in Bob's org, which this PR can't do (that is PR 6). A comment
says so.

**Not covered by `cargo test`:** the dialog and sheet in `app.ts`, focus
handling, the 5xx alert and offline toast, the keyboard reaching the new row,
the upload probe after a 502, and the gate 3 and 4 checks.

## Out of scope (PRs 5-7)

- **PR 5, org-scoped removal.** Events keyed by org and user. Removal stops
  signing people out of every org. Who is told when a lock in one org locks
  the whole account.
- **PR 6, join with an existing account.** Until then a created org's members
  are its creator plus anyone who joins by link with a new account.
- **PR 7, links and the Claude connector.** Org-aware links in emails and
  notifications (gate 6). Connector settings per org.

**Deferred, written down so they aren't lost:**

- slug transliteration ("Lumen Café" becomes `lumen-caf` today), as a small
  later PR;
- a rate limit on `/signup`;
- an advisory lock for the cross-user slug race (its 500 is now shown as the
  dialog's error alert);
- security emails saying "Switch to {org} first" until PR 7;
- unguessable join tokens instead of the slug;
- stripping Unicode control and format characters (categories Cc and Cf)
  from org names and usernames;
- the creator's email on the `/admin` org list;
- one grouped query for `admin_overview`;
- delete or leave an organisation;
- "Create your own organisation" on the waiting, declined and removed pages;
- the `/admin` people table listing every membership;
- transferring ownership;
- unread dots per org in the switcher;
- password reset revokes Claude tokens (follow-up before PR 6).

## Risks

- **The rollback window closes** (gate 5). After the first create, PR 2 (both
  migrations) and PR 3 are forward-only. The PR says so.
- **This PR turns PR 1 to 3 on for real users.** Everything they built was
  tested with SQL-made second memberships. Watch the logs for "refused a
  request from a tab drawn for another organisation" in the first days. A
  burst would mean a client path that lost its claim.
- **Browser cookie behaviour** (gate 3). If Safari or Firefox ignores
  `Set-Cookie` on a manual redirect, the banner's Back fails safe but
  confuses people.
- **Email links** (gate 6) mislead people with two orgs until PR 7.
- **Railway's edge turns early refusals of large bodies into 502s** (gate 4).
  The upload path now probes after a failure. Any other large-body request
  refused early would show a generic error too. Today only uploads send
  bodies that big; urlencoded forms are capped at 2 MB and refused before
  reading only over that.
- **Shared code with signup.** `create_with_owner`, `create_owner` and the
  name validator change for signup too. The signup tests and the new ones pin
  that only the spaces-only case behaves differently.
- **The lock revocation is account-wide.** A lock now revokes Claude
  connections in orgs where nothing went wrong. The person reconnects after
  the reset. That is the point.
- **`admin_overview`** runs four queries per org, bounded by the cap.
- **No delete.** Mistaken orgs pile up in a person's switcher, up to 10.
- **The slug race** can still fail one of two simultaneous same-name creates.
  Rare, and now shown as a retryable error.

## Checks

```sh
bin/build-assets
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

By hand, locally:

- create an org from the desktop menu (dialog), the phone sheet, More, and
  with JavaScript off (`/orgs/new`);
- the toast, the checklist and no welcome pop-up on Home;
- the new org's time zone matches the old one's;
- the form's errors: short name, spaces-only, bad username, `everyone`,
  duplicate name, the daily limit and the at-cap state (set `created_at`
  back by SQL to reach them quickly);
- the 5xx alert (force an error locally, for example by stopping Postgres
  mid-request) and the offline toast (devtools offline);
- a second tab on the old org moves to Home with the notice;
- the chat socket in another tab closes and reconnects in the new org;
- acting: no row; a typed `/orgs/new` shows the note; creating ends acting;
- the keyboard reaches "New organisation", and "N" jumps to it;
- a stale upload that fails as a 502 or network error shows the banner, not
  "Stopped part way": with two tabs, let the probe pass, switch org in the
  other tab before the send, and make the upload fail (devtools request
  blocking on the upload URL gives a network error). Then the same with no
  switch: the plain "Stopped part way" still shows;
- gate 3 (Playwright WebKit and Firefox, then real Safari);
- the 404 page an old-org task link gives in the new org (gate 6).

On production, by Raj: gates 1 and 4 passed on 19 October. Still before
merging: the `created_by_id` count. After deploying: the gate 4 re-check of
the upload fix. After merging: the real-Safari pass (gate 3), then
create one org as Raj, switch back and forth, and confirm the logs show the
create and the switches.

## Screenshots needed

From the local app with realistic data: the mockup's people and orgs (Priya
in Acme Studio, then Lumen Café; Northwind Bakery), with members, tasks and
chat in Acme. Store them on the `pr-screenshots` branch under `multi-org-4/`.
Desktop 1440px and phone 390px, light and dark:

- the switcher with one org and "New organisation" (desktop menu, phone
  sheet, More);
- the switcher with two orgs and the row;
- the create form: the desktop dialog and the phone sheet, empty and filled;
- the form with a field error, and with the duplicate-name error;
- the at-cap form;
- the server-error alert;
- the acting form note, and the acting menu with no row;
- Home in the new org: the toast and the checklist;
- another tab's notice after the create;
- gate 3: the banner and the cookie panel in WebKit, Firefox and Safari,
  with cookie values hidden.

## Decisions (Raj, 19 October)

All approved as recommended in the mockup and the plan's first draft, with
the reviews folded in:

1. `CurrentMember` on both routes, inside the stale-tab check. This deviates
   from mockup question 10.
2. A server-rendered `_org` on the form, so the no-JS path works.
3. No create row while acting. A typed link works, shows the note and ends
   acting.
4. 10 ever and 3 per 24 hours, from `created_by_id` and `created_at`, under a
   `FOR NO KEY UPDATE` lock on the user row.
5. Refuse a name matching one of your active orgs, after trim, whitespace
   collapse and Unicode lowercase.
6. Trimmed-length validation for org names, in signup too.
7. The dialog shows the mockup's 5xx alert and offline toast.
8. Copy the current org's time zone, except while acting.
9. A shared `enter_org` with `switch`. Skip the welcome pop-up, keep the
   guide. Toast "X is ready. You're the owner."
10. An account lock revokes Claude tokens in every org.
11. Playwright WebKit and Firefox locally, real Safari by hand (required), and
    one real-Safari pass on production after merge.
12. No delete or leave in PR 4.
13. After the gate 4 finding: an upload that fails with a 502 or a network
    error probes `/orgs/current` before showing the error. Draining the body
    on the server was rejected.
