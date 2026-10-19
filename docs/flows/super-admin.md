# Super admin and acting in another organisation

The platform super admin is a user with `users.is_super_admin = true`. They
see every organisation at `/admin` and can work inside any of them as an
owner. Working inside an organisation they do not belong to is called
"acting".

## Making the super admin

There is no sign-up path. Run the task:

```sh
SUPER_ADMIN_PASSWORD=choose-one cargo loco task super_admin
```

- It reads `settings.super_admin` (email, username, organisation name; set by
  `SUPER_ADMIN_*` variables in production, see `docs/deploy.md`).
- If the user does not exist, it needs `SUPER_ADMIN_PASSWORD` and creates
  them. If they exist, it never changes their password.
- It sets `is_super_admin` and, if they have no membership, creates their own
  organisation with them as owner.
- Safe to run again.

File: `src/tasks/super_admin.rs`.

## Entering and leaving an organisation

```mermaid
sequenceDiagram
    actor S as Super admin
    participant Ad as admin controller
    participant CM as CurrentMember
    S->>Ad: GET /admin
    Ad-->>S: every organisation with counts, every user
    S->>Ad: POST /admin/orgs/ID/enter
    Ad-->>S: acting_org cookie = ID, redirect /dashboard
    S->>CM: any app page
    CM->>CM: is_super_admin and acting_org points to a real org
    CM-->>S: member of ID, role owner, acting = true
    S->>Ad: POST /admin/leave
    Ad-->>S: cookie cleared, redirect /admin
```

Rules:

- `/admin` and its routes answer 404 to anyone else, so the area is not
  advertised (`src/controllers/admin.rs:14`).
- Entering writes an info log line, "super admin entered organisation"
  (`src/controllers/admin.rs:73`). Nothing is written to the database.
- The `acting_org` cookie is `HttpOnly`, `SameSite=Lax`, and `Secure` in
  production. `CurrentMember` reads it only for super admins, so the cookie
  does nothing for anyone else (`src/extractors/session.rs:54`).
- While acting, the admin's own membership is ignored. Their username in that
  organisation is their `users.name`.
- Signing out (`POST /logout`) clears the cookie too.
- Signing in also clears it, so a new sign-in ends acting. `acting_org` has
  no `Max-Age`, so it already ended when the browser closed.
- `acting_org` wins over `current_org`. While acting, `current_org` is
  ignored and left alone, so leaving returns to it if it is still valid.
- Switching organisation (`POST /orgs/{id}/switch`) clears `acting_org`. A
  super admin can only switch to orgs they belong to. Entering any other org
  stays on `/admin/orgs/{id}/enter`.
- A super admin with no membership and no acting cookie is sent to `/admin`.
  A stray `current_org` is cleared on the way.
- The stale-tab check compares against the acting org (see
  [accounts.md](accounts.md), "Stale tabs"). A tab drawn for the admin's own
  org is stale while they are inside another, and goes to Home. Home offers
  "Back" only to orgs they are an active member of, so a tab drawn while
  acting gets no Back; they enter again from `/admin`.
- The switcher shows while acting only if they have a membership of their
  own. It reads "Acting in" with the acting org, ticked, as "Platform admin",
  then "Your organisations", then "Switching ends acting in Acme Studio."
  Leave stays in the banner. With no membership of their own the name is
  plain text.
- There is no "New organisation" row while acting, in the menu, the sheet or
  More: that menu is about where they are acting and the way back. A typed
  `/orgs/new` still works. The form first says "You're acting in Acme
  Studio. Creating this ends acting. The new organisation is yours, with you
  as owner." Creating clears `acting_org`, makes the admin the owner of the
  new org, and copies nothing from the acting org: it keeps `Europe/London`.
  The log line names the acting org as `acting_org`.

The decision happens in `resolve_org`
(`src/extractors/current_member.rs:219`), which both `CurrentMember` and
`PinnedMember` run.

## What acting allows and blocks

Acting gives the role `owner`, so owner and admin checks pass. Some actions
check `acting` and refuse.

| Area | While acting |
|---|---|
| Tasks, projects, roadmap, meetings, notes | Allowed |
| Approve or decline join requests, change roles, set the time zone | Allowed |
| Remove a member, see "Change these at the source" | Blocked (403) |
| Documents and logins, all routes | Blocked (403) |
| See a login password | Blocked (`can_reveal` is false) |
| React to a task note | Blocked (403) |
| `@everyone` in a task note | Saved, but notifies nobody; not offered in the @ menu |
| Connect Claude, make a personal token | Blocked: a page asks them to leave first |
| Chat | Only conversations they are a member of. They are not added to the organisation's `#general`. |

The blocks come from `CurrentMember::require_own_membership`
(`src/extractors/current_member.rs:133`) and direct `member.acting` checks in
`src/controllers/members.rs:48`, `src/controllers/tasks.rs:910`,
`src/controllers/notifications.rs:192`, `src/controllers/claude.rs:105`,
`src/controllers/oauth.rs:324` and `:363`, and `src/models/logins.rs:451`.

Unverified: chat routes have no `acting` check. Creating a group or starting
a DM while acting is not blocked in the code. Whether the UI offers it was not
checked.
