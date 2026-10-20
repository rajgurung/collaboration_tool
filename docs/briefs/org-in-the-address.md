# Organisation in the address

Status: Draft · Owner: Raj · 20 October 2026

## Summary

Every app page carries its organisation in the address, for example
`/o/acme-studio/tasks`. Each tab then holds its own organisation, so two
companies can be open side by side. Email, notification and Claude links name
their organisation, which covers the old PR 7. The homepage, sign-in, sign-up
and join pages keep their addresses.

## Who and why

The decision, as recorded in `docs/plans/multi-org-1.md` ("PR 7 replaced",
19 October): "Each tab then holds its own organisation, so two companies can
be open side by side and a tab can no longer write into another company by
accident." This is the plan's wording of Raj's decision, not a direct quote.
Raj can add his own words if he wants them here.

Who has the problem: anyone in two or more organisations. Since PR 4 (create
another organisation) and PR 6 (join with an existing account) that is real,
starting with Raj.

What they live with today:

- One `current_org` cookie per browser picks the organisation for every tab.
  A switch in one tab makes every other tab stale. PR 3's check refuses its
  next request and sends it to Home with "You switched organisation in
  another tab". Nothing is misfiled, but two companies can't be worked on at
  once.
- A mention email's item link (`/tasks?open=58`, `/chat/12`) opened in the
  other organisation gives the "It may be in another of your organisations"
  404 page. The person switches and taps the link again.
- The mention email's "all notifications" link (`src/mailers/mentions.rs:56`)
  and the security email's "Login activity" link (`src/mailers/security.rs:151`)
  silently open the cookie's organisation. An owner reading a security alert
  can look at the wrong organisation's activity log and conclude nothing
  happened. That is the costliest failure here.
- The approval email links to `/dashboard` and asks the person to pick the
  organisation from the menu (`src/mailers/membership.rs:47`).
- The Claude Allow page binds the token to the cookie's organisation. To
  connect Claude to Beta you switch to Beta first.

Phones first. On a phone the address bar is mostly hidden, and side-by-side
tabs matter less. The phone gain is links that open the right organisation.
So every page must still name its organisation on screen, and slugs must stay
short (40 characters today).

## What people can do (first version)

1. Open any app page at `/o/{slug}/...`. The page works in that organisation,
   whatever other tabs are doing. Open Acme and Northwind in two tabs and work
   in both.
2. Copy an address and send it to a teammate in the same organisation. It
   opens the same page for them.
3. Use the switcher. Picking another organisation opens the same section
   there (Tasks stays Tasks). From an item page (`/o/acme/tasks/58`) it opens
   that section's list, since the item doesn't exist in the other
   organisation. The rows are real links, so a long-press or Cmd-click opens
   the other organisation in a new tab.
4. Tap a link in an email or a bell notice and land in the organisation it is
   about, signed in or not. A signed-out tap goes through sign-in and comes
   back to the page.
5. Keep using old links and bookmarks. A bare address (`/tasks`, `/dashboard`,
   an old email's `/tasks?open=58`) redirects to the right place (question 2).
6. Connect Claude to a chosen organisation. With two or more, the Allow page
   asks which one.

By role:

- Owner, admin and member: the same addresses. Role checks don't change.
- Waiting for approval in Beta, active in Acme: `/o/beta/...` shows Beta's
  waiting page with a link to Acme. Today it silently falls back to Acme.
- Removed or declined in Beta: Beta's removed or declined page.
- Not a member of Beta, or Beta doesn't exist: the same plain "not found"
  page either way, so the address can't be used to test which organisations
  exist.
- Super admin acting: see question 7.

## Not in this version

- Email verification on `/signup` and `/join`. Its own booked follow-up,
  board loop `email-verify`.
- Custom domains or subdomains per organisation (`acme.collab...`).
- Editable slugs, and a history of old slugs (question 1).
- Changing existing slugs. "lumen-caf" stays "lumen-caf".
- Unread dots for other organisations in the switcher.
- A combined bell across organisations.
- Links in Claude tool results. The MCP tools return no URLs today.
- Removing the stale-tab check (question 3).
- Unguessable join links, and leaving or deleting an organisation (deferred in
  PR 4 and PR 5).

## Done when

- Raj opens Acme Studio's Tasks in one tab and Northwind Bakery's Tasks in
  another. He adds a task in each. Each task is in its own organisation.
  Neither tab is sent to Home, and no notice appears.
- With both tabs open, a chat message in each organisation arrives live in its
  own tab. Each tab's bell counts its own organisation.
- Shusma's last organisation was Acme. She taps a mention email from Northwind
  on her phone. The task opens in Northwind, with Northwind named at the top.
- The same tap while signed out goes to sign-in, then straight to the task.
- An owner taps "Login activity" in a security email about Northwind. The
  page shows Northwind's log, whichever organisation they last used.
- A mention email sent before the change (`/tasks?open=58`) still opens task
  58 in its own organisation for someone who is a member there.
- A bookmark of `/tasks` opens Tasks in the last organisation used.
- Kreepa, who is not in Northwind, opens `/o/northwind-bakery/tasks`. She sees
  the same page as for `/o/made-up-name/tasks`.
- Someone waiting for approval in Beta opens a Beta link and sees Beta's
  waiting page, not Acme.
- A Claude connection made before the change keeps working without
  reconnecting.
- Raj connects Claude with two organisations. The Allow page asks which one,
  and the token works there only.
- The homepage, `/login`, `/signup` and `/join/{slug}` keep their addresses.
- A tab left open across the deploy saves nothing in the wrong place. Its next
  form lands on Home.

## Risks and open questions

Risks:

- Size. Every tenant route, link, form and redirect moves. A quick count:
  about 100 routes in 21 controllers, about 187 hard-coded paths in 64
  templates, about 77 server-built paths and redirects in 25 Rust files, and
  about a dozen in `frontend/app.ts`. PR 3 touched about 200 test call sites.
  A missed link is a visible bug (an extra redirect or a 404), not a leak, as
  long as bare POSTs never replay into the cookie's organisation.
- Bare POSTs. A redirect for a POST would repeat it in the last-used
  organisation, which is the cross-org write PR 3 closed. Only GETs may
  redirect. A bare POST is refused and sends the tab to Home.
- HTMX. Every `hx-get`, `hx-post` and `HX-Redirect` value carries the prefix.
  The `#id` targets don't change. The probes (`/orgs/current`) and `?from=`,
  `?switched=` and `?created=` on Home need a new home or retiring.
- Websockets and SSE. The chat socket (`/chat/{id}/ws`) and the notification
  stream (`/notifications/stream`) move under `/o/{slug}/`. Each tab gets its
  own stream for its own organisation. The switch's "close every live
  connection" step stops being needed.
- Old links in sent emails must keep working. They carry ids or nothing.
  Bell notices store a path without the organisation (`notifications.link`),
  but each row has its `organisation_id`, so the prefix can be added when
  shown. No data migration.
- Sign-in `next`. Today it only accepts the Claude Allow page and join pages
  (`safe_next`, `src/controllers/auth.rs`). Email links need it to accept
  `/o/{slug}/...` too. Any loosening is an open-redirect risk and needs the
  security reviewer.
- PR 2's rule "never add a GET that changes the org". With the organisation in
  the address, a page load chooses its organisation by design, and the cookie
  only remembers the last one. The rule needs restating, and the security
  reviewer should confirm it.
- The logins unlock is tied to one organisation at a time. Unlocking in the
  Northwind tab ends the unlock in the Acme tab. Accepted for the first
  version.
- Search engines and privacy. There is no `robots.txt` today. Slugs name
  companies, and join pages are public. Add a `robots.txt` and `noindex` on
  join and app pages. No `Referrer-Policy` header is set in `src/`; browsers
  default to sending only the origin cross-site. Unverified whether anything
  overrides that. Setting it explicitly is cheap.
- Claude tokens are bound to `{APP_URL}/mcp`. Moving `/mcp` would end every
  connection (`docs/flows/claude-connector.md`).
- Rollback. The plan should need no migration, so reverting is a code revert.
  But emails sent after the change carry `/o/` links, which a reverted app
  would 404. Keep a tiny `/o/{slug}/...` redirect in any revert.

Questions for Raj:

1. Slug or id in the address? Recommend the slug. It is already unique,
   already public in the join link, and readable. An id (`/o/12/tasks`) is
   stable but reveals how many organisations exist. Slugs not editable in
   this version: a rename would break every sent link and join link. Fix
   accents for new organisations only ("Lumen Café" becomes `lumen-cafe`),
   since existing join links are already shared. Editable slugs with
   old-slug redirects can come later if asked for.
2. What do bare addresses do? Recommend: redirect GETs to the last-used
   organisation, keeping the path and query. For an old item link
   (`/tasks?open=58`, `/chat/12`), first find the item's organisation among
   yours and go there. Ids are global, and an item in an organisation you're
   not in gets the plain not-found page. Never ask: a chooser adds a tap on
   every old link, on a phone.
3. The cookie and the stale-tab check? Recommend: the cookie only remembers
   the last organisation, for bare addresses and sign-in landing. Keep the
   check as a backstop, comparing the page's claim with the address's
   organisation. It is built and tested, and costs nothing. Rewrite the Home
   notice, since "you switched in another tab" can no longer happen. Revisit
   removing it after a quiet month of logs.
4. Should every email link carry the organisation? Recommend: yes for every
   link about one organisation (mentions, "all notifications", login activity,
   approval). No for account links (forgot, reset, password changed), which
   stay bare.
5. Does `/mcp` change? Recommend: no. Tokens already name their organisation,
   and moving it ends every connection. The Allow page gets an organisation
   picker when you have two or more, starting on the last used.
   `/settings/claude` moves under `/o/{slug}/` and shows that organisation's
   connections (the old PR 7 "connector settings per org").
6. Public pages stay outside `/o/`? Recommend: yes. The homepage, sign-in,
   sign-up, forgot and reset, `/join/{slug}`, OAuth, `/.well-known`, `/mcp`
   and `/admin`. Account pages (`/orgs/new`, sign out everywhere) stay
   outside too.
7. Super admin acting: does the address show the acting organisation?
   Recommend: yes, `/o/{acting slug}/...`. A page is open to you if you are an
   active member, or a super admin acting in that organisation. Your own
   organisation then works normally in another tab. The acting banner shows
   only on the acting organisation's pages.
8. The switcher: same page or Home? Recommend: the same section, as in "What
   people can do", item 3. Home only from pages with no section equivalent.
9. Rollout? Recommend three PRs, no feature flag. A flag would mean two link
   schemes in every template and test. The bare-address redirect keeps old
   links working, so each PR is safe on its own.
   1. Organisation in the address: routes, links, forms, redirects, HTMX,
      socket and stream, bare-GET redirects, the switcher, `robots.txt`. The
      big one, and the one that gives side-by-side tabs.
   2. Links: email and notification links carry the organisation, old item
      links find their organisation, sign-in `next` accepts `/o/` paths, and
      the approval email links straight in.
   3. Claude: the Allow page picker and per-organisation `/settings/claude`.

   A smaller first step exists: `/o/{slug}` only as a link that sets the
   cookie. It fixes email links but not side-by-side tabs, and it is the "GET
   that switches" PR 2 ruled out. Not recommended.
