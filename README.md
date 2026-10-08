# Collaboration Tool

A multi-tenant team workspace: roadmap, task board with notes, meeting minutes, and live chat with channels, groups and direct messages.

Live at https://collab.rajgurung.me.

## How it works

- Anyone can sign up and create an organisation. They become its owner.
- Others join through the organisation's link (`/join/<slug>`) and wait until an owner or admin approves them.
- Roles are owner, admin and member. A platform super admin can see and enter every organisation from `/admin`.
- Every page and query is scoped to the signed-in member's organisation.

## Stack

- Rust with [Loco](https://loco.rs) 1.2 (SeaORM, Tera 2 templates)
- PostgreSQL
- HTMX for interactivity, a small amount of TypeScript built with esbuild
- Tailwind CSS (standalone CLI)
- Live chat over WebSockets
- Email through Resend's HTTPS API in production, SMTP locally
- Deployed on Railway from the `Dockerfile`

## Local development

Requirements: Rust (stable), Docker.

```sh
# Postgres 17 for development and tests (port 5433)
docker run -d --name collab-postgres -p 5433:5432 \
  -e POSTGRES_USER=loco -e POSTGRES_PASSWORD=loco -e POSTGRES_DB=collab_development postgres:17
docker exec collab-postgres psql -U loco -d collab_development -c "create database collab_test"

# Optional: catch outgoing email at http://localhost:8025
docker run -d --name collab-mailpit -p 1025:1025 -p 8025:8025 axllent/mailpit

# Run the app with Tailwind and esbuild watching (http://localhost:5150)
bin/dev
```

Create a platform super admin for local use:

```sh
SUPER_ADMIN_PASSWORD=choose-one cargo loco task super_admin
```

Use `http://localhost:5150`, not `127.0.0.1`: form posts from any other origin are rejected.

## Checks

```sh
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs the same three on every pull request.

## Layout

| Path | What is there |
|---|---|
| `src/controllers` | One file per area: auth, signup, join, dashboard, roadmap, tasks, meetings, chat, chat_ws, members, admin |
| `src/models` | Domain logic. Generated SeaORM entities are in `_entities` (do not edit) |
| `src/extractors` | `CurrentUser` and `CurrentMember`, the access checks every page goes through |
| `src/workers` | Background jobs (sending email through Resend) |
| `src/data` | Typed settings and the in-memory chat hub |
| `assets/views` | Tera templates and components |
| `frontend` | Tailwind input and TypeScript helpers |
| `migration` | Database migrations |
| `tests` | Request, model, task and view tests, including `tests/requests/tenancy.rs` |
| `docs` | Deployment guide and design notes |

`SPEC.md` and `TASKS.md` record the plan this was built from.

## Deploying

See `docs/deploy.md`. Merges to `main` deploy to Railway.
