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

The `agent-board` Claude Code plugin shows every piece of work at once.

- **Band above the prompt:** one line per open loop (up to three, then "+N
  more"): its stage, a progress bar, and either the agents working on it or
  what it is waiting for. ◆ means waiting on Raj, ● running, ✓ done.
- **`/board`:** a pane with the loops on the left (press one to open it, done
  loops hidden behind "Show done") and the open loop on the right: stages,
  next step, its agents with status and time, notes and links.

The producer keeps one file per piece of work in `.claude/loops/<id>.json`
(local, ignored by git):

```json
{
  "decision": "drop",
  "title": "Documents: drag and drop to upload",
  "stage": 2,
  "stages": ["Plan", "Build", "Review", "PR"],
  "state": "waiting",
  "next": "Raj approves the PR",
  "notes": { "staff-reviewer": "Approve with changes" },
  "links": { "PR": "https://github.com/..." }
}
```

- `stage` counts from 1. `state` is optional: `waiting` (needs Raj) or
  `done`; otherwise the board works it out from the loop's agents.
- **Every agent's description starts with its loop id**, e.g. `drop: Build
  drag-and-drop`. That is how the board groups agents under their loop;
  anything without a prefix shows under "Other agents".
- The older single `.claude/loop.json` still works and shows as one loop.

The plugin is not in this repo. Install it with `/plugin install agent-board
--marketplace rajgurung/agent-board`; `.claude/settings.json` enables it here.

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
