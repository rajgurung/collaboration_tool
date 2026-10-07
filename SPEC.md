# Project: Collaboration Tool on Loco

## Goal

Rebuild the collaboration tool as a multi-tenant Rust web app using the Loco framework, hosted on Railway with Postgres. Anyone can sign up and create an organisation, like Slack workspaces. Others join an organisation through its join link and wait for an owner or admin to approve them. Inside an organisation, members share a roadmap, task board with notes, meeting minutes, and live chat with channels, groups and DMs. The current Next.js/React/Cloudflare D1 code is replaced entirely.

## Scope

### In Scope

- Full Loco MVC app: SeaORM models, controllers, Tera server-rendered views.
- Email + password auth with a required username. Password reset by email.
- Multi-tenancy: organisations, memberships, roles, approval flow.
- Platform super admin (gurungraj26@gmail.com), created by a seed task.
- Feature parity with the current app, scoped per organisation:
  - Overview dashboard (completion %, per-member progress, recent tasks).
  - Roadmap (projects in now / next / later lanes).
  - Task board (create, change status, search, notes per task).
  - Meetings (log minutes, decisions, attendees).
  - Chat (channels, private groups, DMs) with live updates over WebSockets.
- Project create and edit. The current app only has seeded projects, so new organisations need a way to add them.
- Same dark visual design, rebuilt with Tailwind.
- Dockerfile and Railway deploy config.
- Remove the Next.js app and Cloudflare/Vite tooling once the Loco app reaches parity.

### Out of Scope (for now)

- Google / OAuth sign-in.
- Email invites. Joining is by join link plus approval.
- A user belonging to more than one organisation (except the super admin).
- Migrating data from D1. Fresh start.
- Running more than one app instance (chat broadcast is in-memory).
- Billing, plans, usage limits.
- File uploads or attachments.

## Technical Approach

### Stack

- Rust, Loco (latest stable), SeaORM, PostgreSQL.
- Views: Tera templates in `assets/views/`, HTMX for partial updates, HTMX WebSocket extension for chat.
- Client scripts: TypeScript in `frontend/`, compiled by esbuild (standalone binary) to `assets/static/js/`. Kept minimal: dialogs, chat autoscroll.
- CSS: Tailwind standalone CLI, scanning `assets/views/**/*.html` into `assets/static/css/app.css`.
- Email: Loco mailer over SMTP. Provider set by env vars.
- Deploy: multi-stage Dockerfile (Tailwind + esbuild + `cargo build --release`), Railway Postgres via `DATABASE_URL`, app binds `0.0.0.0:$PORT`.

### Data model

All tenant data carries `organisation_id` and every query filters by it.

- `users`: id, email (unique, lowercase), password_hash, username, is_super_admin, reset_token, reset_sent_at, created_at.
- `organisations`: id, name, slug (unique), created_by, created_at.
- `memberships`: id, organisation_id, user_id (unique: one organisation per user), role (owner | admin | member), status (pending | active | rejected), approved_by, approved_at, created_at. Username unique per organisation.
- `projects`: organisation_id, name, lane (now | next | later), status, progress 0-100, accent, owner_id -> users, summary, sort_order.
- `tasks`: organisation_id, project_id, title, status (todo | progress | blocked | done), owner_id -> users, due_date (date, nullable), priority (high | medium | low), sort_order.
- `task_notes`: organisation_id, task_id, author_id, body, created_at.
- `meetings`: organisation_id, title, held_on (date), summary, decisions, created_by, created_at.
- `meeting_attendees`: meeting_id, user_id.
- `conversations`: organisation_id, kind (channel | group | dm), name, created_by, created_at. Each new organisation gets a `general` channel.
- `conversation_members`: conversation_id, user_id (unique pair).
- `messages`: organisation_id, conversation_id, user_id, body (1-1000 chars), created_at.

Dropped from the old schema: `chat_messages` (unused), `sessions` (replaced by Loco auth), owner names stored as text.

### Usernames

Start with a letter, then letters or numbers (`alex`, `alex2`). 3 to 30 characters. Unique per organisation, case-insensitive. Login uses email, not username.

### Auth and tenancy

- Loco's built-in auth (password hashing, JWT). The JWT is stored in an HttpOnly, Secure, SameSite=Lax cookie so server-rendered pages work. To verify: Loco's support for reading the JWT from a cookie.
- One extractor, `CurrentMember`, loads the user, their membership and organisation. Controllers for tenant pages take this extractor, so a missing or pending membership can never reach tenant data.
- Pending members see only a "Waiting for approval" page. Rejected members see a short "Request declined" page.
- Super admin: an admin area listing all organisations and users, and the ability to enter any organisation as if they were its owner.

### Live chat

- WebSocket route per organisation. On send, the server saves the message, renders the message HTML, and broadcasts it through an in-memory `tokio::sync::broadcast` channel to connected members of that conversation.
- HTMX's WebSocket extension appends the HTML. No custom chat protocol on the client.

## Key Flows

1. Create organisation: visitor opens `/signup`, enters email, password, username, organisation name. They become the owner, active immediately, and land on the dashboard.
2. Join organisation: visitor opens `/join/<slug>`, enters email, password, username. Membership is created as pending. They see "Waiting for approval".
3. Approve: owner or admin opens Members, sees pending requests, approves or rejects. The approved member gets full access on next page load.
4. Roles: owner can promote members to admin or demote them. Admins approve members but cannot change roles.
5. Log in / log out: email + password at `/login`.
6. Reset password: `/forgot` sends an email with a one-time link (expires in 1 hour). The link opens a form to set a new password.
7. Work: create projects, add tasks with an owner, change status, add notes, log meetings.
8. Chat: post in `general`, create a private group with chosen members, start a DM. Messages appear live for everyone in that conversation.
9. Super admin: log in, open `/admin`, view all organisations and users, enter any organisation.

## Success Criteria

- [ ] All key flows above work in a browser locally and on Railway.
- [ ] A user in organisation A cannot read or change anything in organisation B. Covered by request tests on every tenant route.
- [ ] A pending member cannot reach any tenant page or WebSocket.
- [ ] Chat messages appear in another open browser within a second, without a reload.
- [ ] Password reset email is delivered and the link works once.
- [ ] `cargo test` and `cargo clippy` pass.
- [ ] Deployed on Railway with Postgres, from the Dockerfile.
- [ ] Layout works on mobile width.
- [ ] Next.js, React, Drizzle and Cloudflare files removed from the repo.

## Risks and Unknowns

- Loco API drift. Loco changes between versions. Mitigation: pin the version and check generated code and docs, not memory.
- Tenant data leaks. One missed filter exposes another company's data. Mitigation: the `CurrentMember` extractor plus scoped query helpers, and cross-tenant tests for every route.
- JWT in a cookie with HTML forms opens the door to CSRF. Mitigation: SameSite=Lax cookie, plus a CSRF token on forms. To decide during build.
- HTMX WebSocket extension and Loco WebSocket routing working together. Mitigation: build a small chat spike first.
- Email delivery on Railway. Unverified: some Railway plans may block outbound SMTP. Mitigation: use a provider with an HTTP API (for example Resend) if SMTP is blocked.
- Rebuilding the shadcn UI (dialogs, sheets, selects) in plain HTML and Tailwind takes time. Mitigation: use native `<dialog>` and `<select>`, a few TS helpers.
- Rust toolchain is not installed on the dev machine yet.

## Open Questions

- Email verification at sign-up: required, or skipped for now?
- Email provider choice (Resend, Postmark, other).
- Can an owner leave or delete their organisation? Can members be removed after approval?
- Should approved members get an email when approved?
- Custom domain for the Railway app.
