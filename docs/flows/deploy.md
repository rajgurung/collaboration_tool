# Deploy

Merging to `main` ships to production at collab.zen-scale.io. Railway builds
the Docker image and starts it, and the app runs any new migrations as it
boots. The setup steps and variables are in [`docs/deploy.md`](../deploy.md).

## From pull request to production

```mermaid
flowchart TD
    PR[Pull request] --> CI[CI: cargo fmt, clippy -D warnings, cargo test]
    PR --> Bot[Review bot Michael Scott reviews]
    CI --> Merge{Merge to main, only when Raj says so}
    Bot --> Merge
    Merge --> CI2[CI runs again on main]
    Merge --> Build[Railway builds the Dockerfile]
    Build --> Start[collab-cli start, LOCO_ENV=production]
    Start --> Mig[auto_migrate: pending migrations run]
    Mig --> Serve[app serves on 0.0.0.0:PORT]
    Serve --> Health{GET /_health within 120 s}
    Health -->|ok| Live[new version live]
    Health -->|fails| Old[Old version behaviour: unverified]
```

## What the repo says

- CI (`.github/workflows/ci.yaml`) runs on every pull request and on every
  push to `main`: `cargo fmt --all -- --check`, `cargo clippy --all-targets
  -- -D warnings`, `cargo test --all`.
- The review bot runs on pull requests that are opened, updated, reopened or
  marked ready (`.github/workflows/claude-code-review.yml`).
- `railway.toml` tells Railway to build `Dockerfile`, check `/_health` with a
  120 second timeout, and restart on failure up to 5 times.
- The image runs `/usr/app/collab-cli start` with `LOCO_ENV=production`
  (`Dockerfile`).
- `config/production.yaml` sets `auto_migrate` from `DB_AUTO_MIGRATE`,
  default `true`. So migrations run when the app starts, before it serves.
- Production refuses to boot if a required variable is missing. That
  includes `DATABASE_URL`, `JWT_SECRET`, `APP_URL`, the mailer settings, the
  `R2_*` settings and `SECRETS_KEK`.

## What lives outside the repo

These are set in GitHub and Railway, not in any file here:

- Railway deploys automatically on every push to `main`. The trigger is set
  in Railway, not in `railway.toml`. Seen on every merge in October 2026.
- `main` is protected: one approving review, and a new push dismisses an
  earlier approval. Admins can bypass it. No status checks are required by
  the rule (checked with the GitHub API on 19 October 2026), so CI passing is
  a team rule, not one GitHub enforces.
- Railway does not wait for CI. The deploy starts as soon as `main` moves,
  while CI on `main` is still running (seen on PR #37's merge).
- What Railway does with the old container when the new one fails its health
  check: unverified.

## Migrations

- Each migration is a file in `migration/src/`, listed in
  `migration/src/lib.rs`. They run in order, once, at boot.
- Every migration has a `down`. Deploys never run `down`; rolling back is a
  manual step.
- Some `down` steps lose data or can fail. Examples:
  - `m20261008_165224_allow_tasks_without_project`: rolling back fails while
    any task has no project.
  - `m20261008_162805_remove_progress_from_projects`: the column comes back
    at 0.
  - `m20261012_181452_add_removed_to_memberships`: removed members become
    `rejected`.
- The project CLAUDE.md asks every PR with a migration to say what it does
  and whether it can be rolled back.

## Why one instance

Three things live in the app's memory, so the service must run as one
replica:

- `ChatHub`: the live chat broadcast (`src/data/chat_hub.rs`).
- `NotifyHub`: the bell's live updates (`src/data/notify_hub.rs`).
- The mention email loop (`src/initializers/mention_emails.rs`). It is safe
  across restarts, because it marks rows emailed before sending.

`auto_migrate` at boot also assumes one instance. The comment in
`config/production.yaml` says to turn it off if more than one instance runs,
so they do not race.
