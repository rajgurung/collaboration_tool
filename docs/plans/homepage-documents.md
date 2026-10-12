# Homepage: show off Documents

Status: Approved by Raj, 12 October 2026 · Brief: `docs/briefs/homepage-documents.md`

Port the approved mockup
(https://claude.ai/artifact/BHAfGMuo5V7DoV37CyvqVs, local copy
`/private/tmp/claude-501/-Users-rajg-Codes-collaboration-tool/83f0d096-c371-4303-b30b-b78218969c3d/scratchpad/collab-homepage-documents.html`)
into the live homepage. Ships after the drag-and-drop PR, because one tick
claims drag and drop.

## Changes

1. `assets/views/home/index.html`
   - Nav: a "Documents" link (`#documents`) between Meetings and Claude.
   - Hero eyebrow: "Roadmaps · Tasks · Meetings · Chat · Documents".
   - Hero product window sidebar: "Documents" after Meetings.
   - New feature row `id="documents"` after Meetings, not flipped (copy
     left, preview right). Copy exactly as the mockup: eyebrow "Documents",
     heading "Company papers, one tap away.", body, and four ticks.
   - Preview: "Company" heading, amber Add, one PDF mid-upload with progress
     bar and "3.1 MB of 5.3 MB", "4 files", four rows (type tile, name,
     avatar "who · when · size", "…"). `role="img"` with an aria-label on
     the panel, inner content `aria-hidden="true"`. Long names truncate.
   - Nothing about logins, passwords, encryption or "attach to tasks".
2. `frontend/styles/app.css`, landing section only: new `lp-`-prefixed
   classes for the documents preview (no unprefixed names; app classes such
   as `.tile`, `.bar`, `.list-row` must not be reused or restyled). Move the
   nav-links breakpoint from 820px to 1000px and reduce the link gap to 18px
   up to 1100px, so six links fit.
3. `tests/requests/page.rs`: the homepage contains `id="documents"` and
   `href="#documents"`, and does not contain "password" or "Logins".

## Checks

`bin/build-assets`, `cargo fmt`, `clippy -D warnings`, `cargo test`.
Screenshots of the homepage section at 1440px and 390px, light and dark, plus
the nav at about 1050px. No sideways scroll at 390px.
