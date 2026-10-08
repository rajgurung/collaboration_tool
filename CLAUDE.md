# Collaboration Tool

Read `AGENTS.md` first. It covers how this Loco app is built and the checks to
run before calling anything done.

## Pull requests

- **Screenshots are a must.** Every PR that changes anything people can see
  includes screenshots in its description. Show desktop (1440px) and phone
  (390px) at least, and dark mode when colours or layout change. A PR with no
  visible change says "No visible change" instead.
- Take screenshots from the local app with realistic data, not test leftovers.
- Store them on the `pr-screenshots` branch in a folder named after the PR
  (e.g. `getting-started/`). Link them in the PR body with
  `https://raw.githubusercontent.com/rajgurung/collaboration_tool/pr-screenshots/<folder>/<file>.png`.
- Merging to `main` deploys to production and runs migrations. Say what a
  migration does in the PR, and whether it can be rolled back.
- The review bot (Michael Scott) reviews every PR. Reply to or fix its
  comments before asking for a merge.
