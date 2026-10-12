# Public homepage

## Goal

Replace the small card at `/` with a full marketing homepage for logged-out
visitors, like Shopify's. Signed-in users still go straight to `/dashboard`.

The approved mockup is
`/private/tmp/claude-501/-Users-rajg-Codes-collaboration-tool/83f0d096-c371-4303-b30b-b78218969c3d/scratchpad/collab-homepage.html`
(published at https://claude.ai/artifact/8LzWujJoMuFauJeBWBGbv8). Match its
layout and look. Where this plan gives different copy or markup, this plan
wins (see "Copy corrections"). Do not add other claims.

## Scope

No migrations, no new routes, no new crates, no new JavaScript.
`src/controllers/page.rs` stays as is.

### 1. Template

Rewrite `assets/views/home/index.html`. It extends `layouts/base.html`
directly, not `layouts/auth.html`. Inside `{% block body %}`, one wrapper
`<div class="landing">` holds everything. Do not put `.landing` on `<body>`:
`frontend/app.ts` appends real flash toasts to `document.body`.

Sections in order: sticky nav, hero with the product window, four-point strip,
four feature rows (Tasks, Chat, Roadmap, Meetings), dark Claude band, three
steps, amber closing CTA, footer.

- Brand mark: `<img src="/static/favicon.svg" alt="" width="30" height="30">`
  (the mountain from the mockup), plus the name from `t(key="app-name", lang="en-US")`
  in nav and footer.
- Links: "Get started" and "Create your organisation" go to `/signup`. "Sign in"
  goes to `/login`. Section links are in-page anchors (`#tasks`, `#chat`,
  `#roadmap`, `#meetings`, `#claude`, `#start`). Give the anchored sections
  `scroll-margin-top: 80px` so the sticky nav doesn't cover them.
- The closing CTA paragraph is `t(key="app-tagline", lang="en-US")`.
- `{% block title %}`: "Collaboration Tool: roadmaps, tasks, meetings and chat".
- Product previews are static HTML with example data (Himalayan Ritual; Priya,
  Sonam, Maya, Raj). Wrap each preview `role="img"` with an `aria-label`
  describing it, and `aria-hidden` on the inner fake content is fine. The Claude
  example is a `<figure>` with a visually hidden `<figcaption>`.
- Two navs: `aria-label="Sections"` in the header, `aria-label="Footer"` below.
- Footer text "© 2026".
- No indentation whitespace inside chat bubble text (keep each bubble's text
  on one line).

### 2. Styles

Add a `/* Landing page */` section inside `@layer components` in
`frontend/styles/app.css`.

- **Prefix every landing class with `lp-`** (`lp-nav`, `lp-hero`, `lp-window`,
  `lp-card`, `lp-chip`, `lp-lane`, `lp-toast`, `lp-bubble`, `lp-claude-grid`,
  and so on). The app already styles `.card`, `.chip`, `.meta`, `.bar`, `.lane`,
  `.toast`, `.decision`, `.msg`, `.bubble`, `.mention`, `.composer`,
  `.board-head`, and Tailwind has `.grid`. None of those names may appear in
  the landing markup unless the app's style is wanted.
- Root `.landing` sets `font-size: 16px; line-height: 1.6`.
- Colours come from the existing tokens. New tokens for the dark Claude band
  (`--ink`, `--ink-text`, `--ink-muted`, `--ink-line`) go into all three token
  blocks (light `:root`, dark by device, `[data-theme="dark"]`). The tick icon
  needs a colour that reads on `--active-bg` in dark mode too (for example a
  CSS mask filled with `var(--link)` instead of a hardcoded SVG colour).
- Font: Plus Jakarta Sans, already self-hosted. Mono labels use
  `ui-monospace, "SF Mono", Menlo, monospace`. No Google Fonts.
- Buttons reuse the app's `.btn`, `.btn-primary`, `.btn-secondary`, `.btn-lg`
  (pill shape). That's an accepted change from the mockup's 10px corners. The
  ghost button gets `background: var(--surface)` via an `lp-` class.
- Reduced motion: under `prefers-reduced-motion: reduce`, set `animation: none`
  on the lifting card and the headline underline `::after` (the global rule only
  shortens durations, which makes an infinite loop flicker). The underline then
  shows at full width.
- Responsive: no horizontal page scroll at 390px. The hero board scrolls inside
  its own container on narrow screens; sidebar hidden below 900px. At 390px the
  nav must fit on one line: below 440px hide the wordmark text and keep the
  mark, "Sign in" and "Get started".

### 3. Copy corrections (override the mockup)

- **Meetings section.**
  - Body: "Log each meeting with what was discussed, who was there and what you
    agreed. The decision stays where the whole team can find it."
  - Ticks: "Everyone who attended is listed" and "The latest decision sits on
    Home".
  - Preview: date block, title "Weekly launch check-in", sub-line "4 attendees"
    (no time of day, no time zone), the decision, then the four attendee rows.
- **Steps, step 2.** "Invite your team. Share the invite link, then approve
  people as they ask to join."
- **Chat body.** "Channels, groups and direct messages that update live, with
  no refresh. Mention someone and they get a notification. If they leave it
  unread, they get an email."
- **Claude example.** Match the real tool inputs in `src/mcp/mod.rs`, e.g.
  `list_tasks project_id=1 status="blocked"` and
  `create_task title="Call supplier about gift boxes" assignees=["me"] due_on="2026-10-16" priority="high"`.
  Check the exact field names there.

### 4. Tests

- `tests/requests/page.rs`: assert the logged-out page contains
  "Every promise has an", `href="/signup"`, `href="/login"`, and does not
  contain `auth-card`.
- The signed-in redirect is already covered by
  `signed_in_users_skip_the_join_and_signup_forms` in `tests/requests/join.rs`.
  Don't duplicate it.
- `tests/views/render.rs` must still pass.

## Checks

`bin/build-assets` (Tailwind compiles), `cargo fmt --all`,
`cargo clippy --all-targets -- -D warnings`, `cargo test`. Use a separate test
database for this worktree: create `collab_test_homepage` on the local Postgres
(port 5433) and run tests with
`DATABASE_URL=postgres://loco:loco@localhost:5433/collab_test_homepage`.

## Not in scope

Pricing, testimonials, a blog, sign-up on the homepage itself, real screenshots
inside the page.
