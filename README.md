# Collaboration Tool

A polished, mobile-friendly project collaboration workspace with persistent tasks, roadmaps, progress tracking, meeting minutes, channels, group conversations and direct messages.

The repository contains anonymised demo data and fictional team members.

## Features

- One-tap demo identity selection
- Roadmap lanes and project progress
- Task board with owners, priorities, status changes and comments
- Founder/team progress dashboard
- Persistent meeting minutes and decisions
- Slack-style channels, custom groups and direct messages
- Responsive desktop and mobile navigation
- Durable SQLite storage through Cloudflare D1

## Stack

- Next.js-compatible Vinext runtime
- React 19 and TypeScript
- Tailwind CSS and Shadcn UI primitives
- Drizzle ORM
- Cloudflare Workers and D1

## Local development

```bash
pnpm install
pnpm run dev
```

Type-check, lint and build:

```bash
pnpm exec tsc --noEmit
pnpm run lint
pnpm run build
```

Schema definitions live in `db/schema.ts`, with generated SQL migrations under `drizzle/`.

## Important

The included one-tap sign-in is intentionally designed for a product demo. Replace it with production authentication and authorization before storing sensitive information or deploying for real users.
