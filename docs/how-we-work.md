# How We Work

Status: Active · Owner: Raj · Last updated: 12 October 2026

Features go through a small team of Claude Code agents before and after they
are built. Raj approves the brief, the mockup and the plan. Each PR is one
feature, with screenshots.

## Who decides

- **Raj** is the final approver. Nothing is built without his yes.
- **The main Claude Code session is the producer.** It hands work to agents,
  merges their feedback into one proposal, keeps the board up to date, pushes
  and opens the PR. Subagents cannot talk to each other, so the producer carries
  work between them.
- **Agents advise and build.** Any reviewer can block in its own area. Raj can
  overrule a block, and the PR says so.

## The team

| Agent | Where | Owns |
|---|---|---|
| Product manager | `.claude/agents/product-manager.md` | The problem, who it is for, first-version scope, "done when", risks. Writes `docs/briefs/`. |
| Designer | `.claude/agents/designer.md` | Screens and flows as an HTML mockup in the app's own look; reviews built screens. |
| Security reviewer | `.claude/agents/security-reviewer.md` | Threat-models anything touching secrets, files, sign-in, access rules, tenancy or third parties. Can block. |
| Staff reviewer | `~/.claude/agents/staff-reviewer.md` | Reviews the build plan and the finished diff: correctness, data, access rules, migrations. |
| Staff engineer | `~/.claude/agents/staff-engineer.md` | Builds the approved plan in a worktree and runs the checks. Never pushes. |

The review bot (Michael Scott) reviews every PR on GitHub.

## The loop

Stages, in order. The board shows them by these names.

1. **Brief.** The product manager writes `docs/briefs/<feature>.md`, with
   questions for Raj.
2. **Approve brief.** Raj answers the questions and approves.
3. **Design.** The designer makes the mockup. The producer publishes it as an
   artifact link.
4. **Approve design.** Raj approves or asks for changes.
5. **Plan.** The producer writes `docs/plans/<feature>.md`. The staff reviewer
   and product manager review it in parallel, plus the security reviewer when
   it touches secrets, files, sign-in, access or third parties. At most 2 rounds.
6. **Build.** The staff engineer builds it in a worktree.
7. **Review.** The staff reviewer checks the diff; the designer checks the
   screens against the mockup.
8. **PR.** The producer takes screenshots, pushes and opens the PR, and adds or
   updates the task on the board in the Collab app.

**Small changes** (bug fixes, copy, tuning) skip Brief and Design: plan, build,
review, PR.

## Review format

Every agent review returns the same four parts:

```
Verdict: Approve | Approve with changes | Block
Findings: numbered, concrete, each naming the file or section
Tradeoffs: what the plan gives up, from this agent's area
Unverified: claims no one has checked
```

A Block says exactly what would turn it into an Approve.

## Watching the loop

The `agent-board` Claude Code mod shows the loop live: a band above the prompt
with the feature, a stage bar, the agents running and how long each has taken,
and the next step. `/board` opens a pane with the full table.

The producer keeps `.claude/loop.json` up to date at each stage. It is local and
ignored by git:

```json
{
  "decision": "documents",
  "title": "Documents space",
  "stage": 1,
  "stages": ["Brief", "Approve brief", "Design", "Approve design", "Plan", "Build", "Review", "PR"],
  "next": "Raj answers the brief's questions",
  "notes": { "product-manager": "Brief drafted" }
}
```

`stage` counts from 1. The mod lives in the Claude Code mods folder, not in this
repo.

## Where things live

| What | Where |
|---|---|
| Product briefs | `docs/briefs/` |
| Build plans | `docs/plans/` |
| Notes on how something works | `docs/notes/` |
| Tasks and progress | The Collab app itself (project "Collab Tool Website") |
| This page | `docs/how-we-work.md` |

## Hard rules

- No secrets or passwords in git or in the app's database as plain text.
- Git commits have no Claude co-author line.
- Rebase PR branches onto `main`; never merge `main` into a PR branch.
- Merging to `main` deploys to production. Only merge when Raj says so.
