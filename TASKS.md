# Tasks

Generated from SPEC.md on 2026-10-07.

Work happens on a `loco` branch. The Loco app lives at the repo root next to the Next.js code (no path clashes) until Task 17 removes the old code.

Parallel tasks share a few files: `src/app.rs` (route registration), `src/controllers/mod.rs`, and the nav in the base layout. Task 5 registers every controller and nav item up front as stubs, so Group 3 tasks only fill in their own files.

## Prerequisite (you, on your machine)

- [x] **Task 0**: Install the toolchain.
  - Steps: install Rust via rustup, `cargo install loco sea-orm-cli`, start Postgres locally (`docker run -p 5432:5432 -e POSTGRES_PASSWORD=postgres postgres:17`).
  - Verify: `cargo --version`, `loco --version`, `psql` connects.

## Sequential: foundation

- [x] **Task 1**: Scaffold the Loco app.
  - What: `loco new` with the SaaS starter, Postgres, server-side rendered assets. Pin the Loco version. Merge `.gitignore`. Point `config/development.yaml` and `config/test.yaml` at local Postgres. Make `config/production.yaml` read `DATABASE_URL` and bind `0.0.0.0:$PORT`.
  - Depends on: Task 0.
  - Files: `Cargo.toml`, `src/`, `config/`, `migration/`, `tests/`, `assets/`, `.gitignore`.
  - Verify: `cargo loco start` serves a page. `cargo test` passes.

## Parallel Group 1 (after Task 1)

- [x] **Task 2**: Frontend build pipeline and base layout.
  - What: Tailwind standalone CLI and esbuild (pinned binaries fetched by a script). `frontend/app.ts` entry. A `bin/dev` script that runs Tailwind watch, esbuild watch and `cargo loco start`. Port the colour tokens, fonts and shared classes (`metric-card`, `task-card`, `chat-bubble`, ambient background) from `app/globals.css`. Base layout with header, desktop tabs and mobile bottom nav, HTMX and the HTMX ws extension loaded from `assets/static/vendor/`.
  - Files: `frontend/`, `tailwind.css`, `bin/`, `assets/views/base.html`, `assets/static/`.
  - Verify: `bin/dev` runs. A test page renders with the dark theme. Editing a `.ts` file rebuilds the JS.

- [x] **Task 3**: Database schema.
  - What: SeaORM migrations for every table in SPEC.md (extend the starter `users` table with `username`, `is_super_admin`, reset fields). Foreign keys, unique indexes (email, org slug, one membership per user, username per org case-insensitive, conversation member pair), and `organisation_id` indexes. Generate entities.
  - Files: `migration/src/`, `src/models/_entities/`.
  - Verify: `cargo loco db migrate` and `cargo loco db reset` succeed. `cargo loco db entities` produces no diff.

- [x] **Task 4a**: WebSocket spike.
  - What: throwaway route proving a Loco/axum WebSocket handler plus HTMX ws extension can broadcast server-rendered HTML to two browsers. Record findings in `docs/notes/websocket-spike.md`, then delete the route.
  - Files: `src/controllers/spike.rs` (temporary), `docs/notes/`.
  - Verify: a message sent in one tab appears in another within a second.

## Sequential: auth and tenancy (after Group 1)

- [x] **Task 4**: Auth.
  - What: adapt the starter auth to server-rendered pages. JWT in an HttpOnly, Secure, SameSite=Lax cookie (confirm Loco's cookie support, or write a small extractor). Login, logout, forgot password, reset password pages. Reset link emailed via the Loco mailer, valid once for 1 hour. SMTP settings from env vars. Decide and implement CSRF protection for forms. Username validation (letter first, letters and digits, 3 to 30).
  - Depends on: Tasks 2, 3.
  - Files: `src/controllers/auth.rs`, `src/models/users.rs`, `src/mailers/`, `assets/views/auth/`, `config/`.
  - Verify: request tests for login, wrong password, logout, reset (token used twice fails, expired token fails). Dev mailer shows the reset email.

- [x] **Task 5**: Tenancy core.
  - What: `CurrentMember` extractor (user + active membership + organisation, super admin may enter any org). Sign-up creates organisation, owner membership and `general` channel in one transaction. `/join/<slug>` creates a pending membership. Waiting and declined pages. Seed task that creates the super admin (gurungraj26@gmail.com, password from env var). Register stub controllers and nav items for dashboard, roadmap, tasks, meetings, chat, members, admin.
  - Depends on: Task 4.
  - Files: `src/extractors/`, `src/controllers/{signup,join,stubs}.rs`, `src/models/{organisations,memberships}.rs`, `src/tasks/`, `src/app.rs`, `assets/views/{signup,join,pending}/`.
  - Verify: request tests: owner sign-up lands on dashboard, join leaves user pending, pending user gets the waiting page on every tenant route, unknown slug is 404, seed task is idempotent.

## Parallel Group 2 (after Task 5)

- [x] **Task 6**: Members management.
  - What: members page listing active and pending. Owner/admin approve or reject. Owner promotes or demotes admins. Admins cannot change roles.
  - Files: `src/controllers/members.rs`, `assets/views/members/`.
  - Verify: request tests for each role's permissions, including a member trying to approve (403).

- [x] **Task 7**: Projects and roadmap.
  - What: roadmap page with now / next / later lanes. Create and edit project (name, lane, status, progress, accent, owner from org members, summary) in a `<dialog>` form via HTMX.
  - Files: `src/controllers/projects.rs`, `src/models/projects.rs`, `assets/views/projects/`.
  - Verify: request tests for create, edit, validation errors, owner must be an org member.

- [x] **Task 8**: Task board and notes.
  - What: tasks page with search, create task (project, owner, priority, due date), inline status change via HTMX, side panel with notes and add-note form.
  - Files: `src/controllers/tasks.rs`, `src/models/{tasks,task_notes}.rs`, `assets/views/tasks/`.
  - Verify: request tests for create, status change (invalid status rejected), notes, search.

- [x] **Task 9**: Meetings.
  - What: meetings list and log-meeting form (title, date, summary, decisions, attendees picked from members).
  - Files: `src/controllers/meetings.rs`, `src/models/meetings.rs`, `assets/views/meetings/`.
  - Verify: request tests for create and validation. Attendees must be org members.

- [x] **Task 10**: Chat (HTTP part).
  - What: conversation sidebar (channels, groups, DMs with last message), message history (latest 200), create group with chosen members, start DM (deduplicated per pair), send message via a normal POST as fallback. Body 1 to 1000 chars.
  - Files: `src/controllers/chat.rs`, `src/models/{conversations,messages}.rs`, `assets/views/chat/`.
  - Verify: request tests: non-members get 403 on a conversation, DM to same person twice reuses it, group members limited to the org.

- [x] **Task 11**: Super admin area.
  - What: `/admin` listing organisations and users with counts. "Enter organisation" sets the acting org for the super admin. Non super admins get 404.
  - Files: `src/controllers/admin.rs`, `assets/views/admin/`.
  - Verify: request tests for access control and entering an org.

- [x] **Task 12**: Dockerfile and Railway config.
  - What: multi-stage Dockerfile (Tailwind + esbuild, `cargo build --release`, slim runtime image with `assets/` and `config/`). Run migrations on start. `railway.json` or `railway.toml` with health check. Document required env vars.
  - Files: `Dockerfile`, `.dockerignore`, `railway.toml`, `docs/deploy.md`.
  - Verify: `docker build` succeeds. The container starts against local Postgres with `PORT=8080` and serves the login page.

## Sequential (after Group 2)

- [x] **Task 13**: Live chat over WebSockets.
  - What: per-organisation WebSocket route using the pattern from the spike. In-memory broadcast keyed by conversation. Sending through the socket saves the message and pushes rendered HTML to members. TS helper for autoscroll and reconnect.
  - Depends on: Tasks 4a, 10.
  - Files: `src/controllers/chat_ws.rs`, `src/chat_hub.rs`, `frontend/chat.ts`, `assets/views/chat/`.
  - Verify: two browsers see each other's messages within a second. Pending users and non-members cannot open the socket (test).

- [ ] **Task 14**: Overview dashboard.
  - What: completion percentage, per-member progress (same scoring as `app/page.tsx`: done 100, progress 55, blocked 10, todo 15), active projects, recent tasks.
  - Depends on: Tasks 7, 8.
  - Files: `src/controllers/dashboard.rs`, `assets/views/dashboard/`.
  - Verify: request test with known data checks the numbers.

## Final

- [ ] **Task 15**: Cross-tenant security tests.
  - What: one test file that creates two organisations and tries every tenant route and the WebSocket from org A against org B's IDs.
  - Depends on: Tasks 6 to 14.
  - Files: `tests/requests/tenancy.rs`.
  - Verify: every attempt returns 404 or 403. `cargo test` and `cargo clippy -- -D warnings` pass.

- [ ] **Task 16**: Deploy to Railway.
  - What: you create the Railway project, add Postgres, set SMTP and super admin env vars. Deploy, run the seed task, smoke test every flow in SPEC.md on desktop and mobile.
  - Depends on: Tasks 12, 15.
  - Verify: SPEC.md success criteria checked off on the live URL.

- [ ] **Task 17**: Remove the Next.js app.
  - What: delete `app/`, `components/`, `db/`, `drizzle/`, `lib/`, `hooks/`, `examples/`, `build/`, `scripts/`, `vendor/`, `public/`, `.openai/`, Node and Cloudflare config files. Rewrite README for the Loco app.
  - Depends on: Task 16.
  - Verify: repo contains only the Loco app. `cargo test` and `docker build` still pass.
