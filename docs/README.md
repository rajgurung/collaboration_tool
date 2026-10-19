# Collaboration Tool docs

Collaboration Tool is a workspace for small teams: roadmap, tasks, meetings,
chat, documents and a Claude connector. It is a Rust app on Loco (a
Rails-like framework). Pages are rendered on the server with Tera templates,
and HTMX with a little TypeScript makes them feel quick. Chat runs over
WebSockets. Data is in Postgres, files are in Cloudflare R2, and it is
deployed on Railway. It is multi-tenant by organisation: every row belongs to
one organisation, and people only see their own organisation's work.

Start with `AGENTS.md` at the repo root for how the code is built and the
checks to run.

## Architecture

- [architecture/erd.md](architecture/erd.md): the data model. An overview
  diagram, one diagram per area, unique indexes, cascades, tenancy and what is
  encrypted.

## Business flows

Each page has a summary, a diagram, the rules and edge cases, and the files
that implement it.

- [flows/accounts.md](flows/accounts.md): sign up an organisation, join by
  link, owner approval, membership states, removing members, sign in,
  password reset, sign out everywhere.
- [flows/tasks.md](flows/tasks.md): projects, task statuses, assignees, notes,
  @mentions and @everyone, and the notifications they send.
- [flows/chat.md](flows/chat.md): channels, groups and DMs, sending over
  WebSockets, read receipts, reactions and threads.
- [flows/notifications.md](flows/notifications.md): the bell, the SSE stream,
  what triggers each kind, and mention emails.
- [flows/documents.md](flows/documents.md): folders, uploads to R2, encrypted
  logins, unlock and reveal, and the audit log.
- [flows/claude-connector.md](flows/claude-connector.md): OAuth consent,
  bearer tokens, what the MCP tools can do and their limits.
- [flows/super-admin.md](flows/super-admin.md): the platform super admin and
  acting inside another organisation.
- [flows/deploy.md](flows/deploy.md): from pull request to production, and
  migrations on start.

## How we work and deploy

- [how-we-work.md](how-we-work.md): how a feature goes from brief to PR
  through the agent team, and the hard rules.
- [deploy.md](deploy.md): Railway setup, environment variables, the custom
  domain, and running the image locally.

## Notes

How a part of the system works and why it was built that way.

- [notes/auth.md](notes/auth.md): sessions, CSRF, password and email rules.
- [notes/claude-connector.md](notes/claude-connector.md): the MCP server,
  token design, redirect allowlist and what is not done yet.
- [notes/websocket-spike.md](notes/websocket-spike.md): the spike that proved
  WebSockets work in Loco with the HTMX ws extension.

## Briefs

Product briefs from the product manager: the problem, who it is for and the
first-version scope. One file per feature in [briefs/](briefs/).

- [briefs/chat-emoji.md](briefs/chat-emoji.md): reactions attached to the
  message bubble, and reactions on task notes.
- [briefs/chat-threads.md](briefs/chat-threads.md): thread replies and chat
  reactions.
- [briefs/documents.md](briefs/documents.md): the Documents space for files
  and shared logins.
- [briefs/homepage-documents.md](briefs/homepage-documents.md): showing
  Documents on the public homepage.

## Plans

Build plans, reviewed before anything is built. One file per feature in
[plans/](plans/).

- [plans/chat-emoji.md](plans/chat-emoji.md): attached reactions and note
  reactions.
- [plans/chat-threads.md](plans/chat-threads.md): threads and reactions.
- [plans/claude-connector.md](plans/claude-connector.md): the MCP server with
  OAuth and personal tokens.
- [plans/documents.md](plans/documents.md): the Documents space.
- [plans/documents-drop.md](plans/documents-drop.md): drag and drop to upload.
- [plans/done-cap.md](plans/done-cap.md): keeping the Done column short.
- [plans/homepage.md](plans/homepage.md): the public homepage.
- [plans/homepage-documents.md](plans/homepage-documents.md): Documents on the
  homepage.

## Designs

HTML mockups from the designer, in [designs/](designs/).

- [designs/chat-emoji.html](designs/chat-emoji.html): the approved mockup for
  attached reactions. Open it in a browser.

Screenshots used by the repo README are in [images/](images/).

## Glossary

- Organisation: the tenant. A team's own space with its own projects, tasks,
  chat and documents. Table `organisations`; every other tenant table has
  `organisation_id`.
- Member: a person in an organisation, through a row in `memberships`. Only
  members with status `active` can use the app. The other statuses are
  `pending` (waiting for approval), `rejected` (shown as "declined") and
  `removed`.
- Roles: `owner`, `admin` and `member`. Owners and admins approve, decline
  and remove people, manage folders and logins, and set the time zone. Only
  owners change roles and read the login activity log. The person who signs
  up an organisation is its first owner.
- Super admin: a platform-wide user (`users.is_super_admin`) who can see every
  organisation and act inside any of them as an owner.
- Acting: a super admin working inside an organisation they do not belong to.
  Some actions are blocked while acting.
- Channel: a conversation for everyone in the organisation. Today only
  `#general` exists.
- Group: a private conversation with a name, for the people its creator
  picked.
- DM: a direct conversation between two people.
- Thread: replies hanging off one top-level message. One level deep.
- Run: consecutive messages from the same person in a chat feed. The avatar
  and reader faces show on the last message of a run.
- Receipt: who has read a message, worked out from each member's
  `last_read_at`. Groups and channels show reader faces to everyone; a DM
  shows "Read" to the author only.
- Chore: a task with no project.
- Login: a shared username and password in Documents. The password is
  encrypted; seeing it needs the app password again.
- Grant: one Claude connection. Every code and token from one consent shares
  a `grant_id`, and revoking ends them all.
- Loop file: `.claude/loops/<id>.json`, one per piece of work, which the
  agent-board plugin reads to show its stage and agents. Local only, not in
  git. See [how-we-work.md](how-we-work.md).
