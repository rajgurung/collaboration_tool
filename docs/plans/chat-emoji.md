# Chat emoji, attached reactions and task notes: build plan

Status: Revised after review round 1 · Owner: Raj · 15 October 2026

Builds the approved brief `docs/briefs/chat-emoji.md` (with its "Decisions
(15 October 2026)", "Chip taps" and "One PR" sections) and the approved
mockup `docs/designs/chat-emoji.html`. "Mockup 6.3" means the mockup's
section 6, item 3 in its annotation list. Where Raj's later decisions differ
from the mockup, his decisions win.

## Summary

One PR, branch `chat-emoji` (cut from `main`). Raj chose one PR. Four parts,
built in this order:

- **A. Chat message restyle.** Time and ticks move inside the bubble. Group
  reader faces leave the message; "Read by" moves into the action sheet.
  The thread line becomes a footer inside the bubble. Reactions become one
  chip per emoji, overlapping the bubble's bottom edge. Tapping a chip adds
  or removes your reaction, as today.
- **B. Reactions on task notes.** A `note_reactions` table, the same six
  emoji, the same chips, a smiley-plus quick bar per note.
- **C. @everyone on task notes.** Notifies every approved member except the
  writer. Never from the Claude connector or an acting super admin.
- **D. Emoji picker.** Built in, about 200 emoji with names, search, a
  Recent row. Docks under the field on phones, a popover on desktop. On the
  chat composer, the thread composer and the note box.

Only part B has a migration.

## Approved decisions this plan follows

From the brief (Raj, 15 October 2026):

1. Notes get the same six reactions as chat, with the same look.
2. @everyone on a note reaches every approved member of the organisation
   except the writer.
3. Reactions stay at the six. No "react with any emoji".
4. The Claude connector cannot use @everyone in notes.
5. Group reader faces leave the message. Ticks show the state. Who read it
   moves to the message sheet.
6. Every message shows its time, inside the bubble.
7. The picker has search by emoji name.
8. One chip per emoji with its own count (the mockup's `pill()` function).
9. **Chip taps toggle**, like Slack and like today: one tap adds your
   reaction with that emoji, or removes it if it is yours. Chat and notes.
   Who reacted: a hover or focus popover on the chip on desktop; on phones,
   tap the bubble and the sheet lists reactions, as today.
10. **One PR.**

Also from the brief: note reactions send no notification and have no live
update. Emoji draw with the device's font. No third-party script, font or
image.

From the mockup annotations:

- Time and ticks sit inside the bubble, bottom right, with a pad on the last
  line so text never runs under them (mockup 1.1).
- Ticks: grey when sent, `--link` when everyone has read it (mockup 1.3).
- Others' chips start from the left, yours from the right. The bubble gets
  5px more bottom padding when it has chips. Your chip is amber. 26px to
  look at, 44px to tap. The avatar lifts to stay level with the bubble
  (mockup 1.5 to 1.7).
- Thread footer inside the bubble under a hairline, 44px, a real link
  (mockup 1.8).
- In a run, joined corners tighten to 6px (mockup 1.9).
- The smiley sits inside the field on the left (mockup 1.10).
- The sheet gains "Read by" for your own messages (mockup 3.1).
- The desktop hover toolbar stays as it is (mockup 3.4).
- Picker: phone docks where the keyboard was and the smiley becomes a
  keyboard button; desktop is a 376px popover that stays open; search first;
  Recent shows the last 16 and is hidden until used; eight 44px category
  tabs; desktop footer shows the focused emoji's name; no loading state
  (mockup 5.1 to 5.7).
- Notes stay bubble-less. Chips sit under the note text. A smiley-plus button
  at the end of each note's name row opens the six picks in a small bar.
  The @ menu shows "everyone" first with "Notify the whole team (N people)".
  A failed post keeps the text (mockup 6.1, 6.3 to 6.6).

## Choices this plan makes

1. **Chips keep the fixed `EMOJI` order**, as today. The mockup says "most
   used first" (mockup 1.4). Now that a tap toggles, a chip that moves when
   someone else's reaction lands could put a different emoji under your
   finger, and you would add or remove the wrong one. A fixed order never
   moves.
2. **Phone "who reacted" on notes is a small "Who reacted" text button**
   after the chips. It toggles an inline list with the existing
   `data-toggle` helper (`frontend/app.ts` line 302). It shows only when
   the note has reactions and only on devices that do not hover (`@media
   (hover: none)`). Desktop uses the hover popover on each chip, like chat.
   The chips themselves toggle, so they cannot open the list.
3. **The connector and acting super admins cannot use @everyone.**
   `note_added` decides this itself from the member: `member.via_session &&
   !member.acting`. Bearer-token members (Claude) have `via_session: false`
   (`src/extractors/current_member.rs` line 54). The caller cannot turn it
   on. The note text is saved as written; it is not stripped.
4. **"Read by everyone" counts only members who were in the conversation
   when the message was sent, and are still approved.** Today `receipt()`
   refuses an "everyone" state because "people who join later would make it
   misleading" (`src/controllers/chat.rs` line 383). The tick needs it, so
   `ReadMark` gains the member's join time (`conversation_members.created_at`).
5. **No "sending" clock and no "Not sent" state.** The mockup shows both
   (mockup 2). Today a message appears only once the server has saved it, so
   neither state exists. Adding them needs optimistic sending.
6. **No avatars in the @ menu.** Mockup 6.4 shows them. `data-mentions`
   carries names only. This plan adds the "everyone" row with icon and hint.
7. **No hover list of readers on desktop.** The reader list lives only in
   the sheet.

---

# Part A: chat message restyle

## 1. Shared reaction view types: `src/views/reactions.rs`

Move these from `src/controllers/chat.rs` (lines 101 to 198) into a new
`src/views/reactions.rs`, and add `pub mod reactions;` to `src/views/mod.rs`:

- `ReactionRow`, `Reactor`, `ReactionChip`, `Pick`, `chips()`, `picks()`.
- New `ReactionRow::named(emoji: String, user_id: i64, names: &HashMap<i64,
  String>) -> Self`, which uses "Former member" when the person has left.
  Chat's `reaction_rows()` and the notes code both call it.

In `src/controllers/chat.rs`, add `pub use crate::views::reactions::{chips,
picks, Pick, ReactionChip, ReactionRow, Reactor};` so `src/data/chat_hub.rs`,
`src/controllers/chat_ws.rs` and the tests keep compiling unchanged.

`chips()` and `picks()` do not change: fixed `EMOJI` order, "You" first,
labels "👍 3, you reacted" and "👍 3".

## 2. Receipts become ticks

`src/models/conversation_members.rs`:

- `pub type ReadMark = (i64, Option<DateTimeWithTimeZone>,
  DateTimeWithTimeZone);` The third value is when the person joined the
  conversation (the row's `created_at`). Doc comment says so.
- `read_marks()` maps `(m.user_id, m.last_read_at, m.created_at)`.

`src/controllers/chat.rs`, `Receipt` gains `pub all: bool`.

`receipt()` changes:

- Readers: as today (`id != author`, `last_read_at >= created_at`, in
  `names`). `names` is the approved team, so pending people never count.
- `all`: take the marks with `id != author`, `joined <= created_at` and
  `names.contains_key(id)`. `all` is true when that set is not empty and
  every one of them has read it.
- `text`: DM "Read" as today. Group or channel: "Read by everyone" when
  `all`, else today's "Read by bob, carol" or "Read by u2, u3 and 4 others".
- Still `None` when nobody has read it. Update the doc comment: drop "There
  is no everyone" and say why joiners are left out.

`src/controllers/chat_ws.rs` line 384: `marks.iter().map(|(id, _, _)| *id)`.

The live path keeps its shape. `mark_read` still publishes `ChatEvent::Read`,
and each socket swaps `#receipt-{id}`. Only the template changes.
`not_older()` still compares reader counts, which only grow.

## 3. Templates

### `assets/views/chat/_message.html`

New shape (the mockup's `msg()` function):

```html
<div class="msg [msg-own] [msg-start]" id="message-{id}">
  [<span class="msg-av visible|invisible">avatar</span>]   (others only, as today)
  <div class="msg-body">
    [<span class="msg-name">author</span>]                  (as today)
    <div class="bubble" tabindex="0" data-actions-url="...">
      <div class="bubble-main"><span class="bubble-text">{parts}</span><span class="stamp-pad[ stamp-pad-tick]" aria-hidden="true"></span><span class="stamp">{at}[{% include "chat/_receipt.html" %}]</span></div>
      {% include "chat/_thread_link.html" %}
    </div>
    {% include "chat/_reactions.html" %}
    {% include "chat/_tools.html" %}
  </div>
</div>
```

- The tick is included when `message.read_receipts` is true (your own
  top-level messages, as today). Replies keep no tick, as today.
- `stamp-pad-tick` is added with the tick, so the pad fits the time and the
  tick (60px), and is narrower without it (42px).
- `.msg-meta`, `.msg-time` and `.msg-extra` go. `show_time` stays on
  `MessageView`; it now only decides whether the avatar shows (last in a
  run). Its doc comment says so.
- `.bubble-main` has no whitespace between its spans. `.bubble` today has
  `white-space: pre-wrap`; move that to `.bubble-text` (section 4).
- Your own messages get `<span class="sr-only">Sent </span>` before the
  time, so the stamp reads "Sent 09:31, read by everyone" (mockup 2.3).

### `assets/views/chat/_receipt.html` (the tick)

Same id and the same `oob` switch, so `render_receipts()` in `chat_ws.rs`
does not change:

```html
<span class="tick{% if message.receipt and message.receipt.all %} tick-read{% endif %}" id="receipt-{id}" [hx-swap-oob="true"]>{{<icon name="check-check" class="h-[15px] w-[15px]"/>}}<span class="sr-only">, {receipt.text or "not read yet"}</span></span>
```

No avatars, no button, no reader popover. A DM's `all` is true once the
other person has read it.

### `assets/views/chat/_thread_link.html` (the footer)

Same slot, same id `thread-{id}`, same `oob` switch, so `render_reply()`
does not change. The slot now sits inside the bubble. The link class changes
from `thread-link` to `thread-foot`. Contents as today: face stack
(`avatar-2xs` instead of `xs`), `<b>3 replies</b>`, the time, a chevron.
Same `aria-label`.

### `assets/views/chat/_reactions.html` (the chips)

Today's markup, almost unchanged: same id `reacts-{id}`, same `oob` switch,
always rendered, no `div` inside (the request tests cut the slot at the
first `</div>`). Each chip stays `<span class="react-wrap"><button
type="button" class="react[ mine]" data-react data-emoji data-message
data-url aria-pressed aria-label>…</button><span class="react-pop"
role="tooltip">…</span></span>`. So a tap toggles, as today, and the hover
or focus popover still lists who reacted.

Only change: add `role="group" aria-label="Reactions"` to the `.reacts`
div. The look changes through CSS (section 4).

`_reactions_update.html`, `_tools.html` and `_tool_picks.html` do not
change.

### `assets/views/chat/_actions.html` (the sheet)

- The "Reactions" label shows the total: "Reactions · 8". The handler
  passes `reaction_total` (sum of counts).
- New "Read by" block (mockup 3.1), only when the handler passes `read_by`:
  - no receipt: one row, grey tick, "Not read yet";
  - `all`: `--link` tick, "Everyone" (DM: "Read"), "· N people" in a group
    or channel, and the `receipt-stack` of `shown` plus "+more";
  - otherwise: grey tick, reader names joined with ", ", and the stack.

`actions()` in `src/controllers/chat.rs` builds `read_by` only when
`message.user_id == member.user.id`, `message.parent_id` is `None`, and
`query.thread` is `None`. It calls `conversation_members::Model::read_marks`
(already `in_tenant(member.org.id)`) and `receipt()`, then counts the people
the tick waits for (the same set `all` uses) for "N people". One extra
query, only for your own messages.

### Live update targets

Every element a websocket out-of-band swap targets today keeps a target:

| Target | Rendered by | Where it lives now | Swapped by |
|---|---|---|---|
| `#reacts-{id}` | `_reactions.html` | `.msg-body`, after the bubble | `render_reactions` (`ChatEvent::Reactions`) and the `react` POST |
| `#tools-{id}` | `_tool_picks.html` | `.msg-tools` in `.msg-body` | same |
| `#thread-{id}` | `_thread_link.html` | inside `.bubble`, after `.bubble-main` | `render_reply` (main-feed sockets) |
| `#receipt-{id}` | `_receipt.html` | inside `.stamp`, own top-level messages | `render_receipts` (`ChatEvent::Read`) |
| `#thread-count` | `_thread_count.html` | thread page, unchanged | `render_reply` (thread sockets) |
| `#chat-feed` append | `_message_oob.html` | unchanged | `render` and `render_reply` |

The thread page (`thread.html`, `_thread_feed.html`) uses `_message.html`,
so it gets the new look with no template change. Its parent has `thread:
None`, so it shows chips and no footer (mockup 4).

## 4. CSS: `frontend/styles/app.css` chat block (lines 409 to 530)

Copy from the mockup's "NEW: the message" block, using the app's tokens.

Remove:

- `.msg > .msg-extra`, `.msg-own > .msg-extra` and the grid's second row.
- `.thread-link` rules (renamed `.thread-foot`).
- `.msg-meta`, `.msg-time`, `.msg-receipt`, `.msg-receipt-dm`,
  `.msg-receipt-btn`, `.receipt-pop`. Keep `.receipt-stack`, `.receipt-more`
  and `.receipt-pop-row` (the sheet and the chip popover use them).
- The "only the last message in a run shows its receipt" rules (lines 515 to
  516).
- In the `@layer utilities` `(hover: hover)` block (lines 911 to 925): the
  `.msg-receipt` rules. Keep the `.react-wrap` popover rules.

Add or change (values from the mockup):

- `.msg` keeps one row: `grid-template-columns: 28px minmax(0, 1fr)`,
  `align-items: end`. `.msg > .msg-body { grid-column: 2; justify-self:
  start; display: flex; flex-direction: column; align-items: flex-start; }`.
  `.msg-own .msg-body` sits in column 1 at the end, `align-items: flex-end`.
- Avatar lift: `.msg:has(> .msg-body > .reacts:not(:empty)) > .msg-av {
  margin-bottom: 18px; }`. `:not(:empty)` matters because the slot is always
  rendered.
- `.bubble`: `padding: 6px 10px 7px 12px; overflow: hidden; white-space:
  normal`. `.bubble-text { white-space: pre-wrap; }`.
- Run corners: `.msg:not(.msg-start):not(.msg-own) .bubble {
  border-top-left-radius: 6px; }`, `.msg-own:not(.msg-start) .bubble {
  border-top-right-radius: 6px; }`.
- `.bubble-main { position: relative; }`, `.stamp-pad` (inline-block, 42px),
  `.stamp-pad-tick` (60px), `.stamp` (absolute, right -2px, bottom -2px,
  11px, `--muted`; own: `color-mix(in srgb, var(--own-text) 62%,
  transparent)`), `.tick { display: inline-flex; }`, `.tick-read { color:
  var(--link); }`.
- `.thread-slot:empty { display: none; }`, `.thread-foot` (44px, margin
  `7px -10px -7px -12px`, hairline top border, hover tint, own-bubble border
  and time colours), `.thread-foot b { color: var(--link); }`,
  `.thread-stack` rings in `--surface` or `--own-bubble`.
- `.reacts { position: relative; z-index: 1; display: flex; flex-wrap: wrap;
  gap: 4px; max-width: 100%; margin: -8px 10px 0; padding: 0; }`,
  `.reacts:empty { display: none; }`, `.msg-own .reacts { justify-content:
  flex-end; }`.
- `.msg-body:has(> .reacts:not(:empty)) > .bubble:not(:has(.thread-foot)) {
  padding-bottom: 12px; }` so chips never cover the time.
- `.react` restyled as the mockup's `.react-chip`: 26px tall, 13px radius,
  `padding: 0 8px 0 6px`, `--surface`, 1px `--line` border, 12px bold
  tabular numbers, `box-shadow: 0 0 0 2px var(--bg)`. `.react .emoji {
  font-size: 14px; }`. `.react::after { inset: -9px -2px; }` for a 44px tap
  area. `.react.mine` unchanged (`--active-bg`, `--accent` border).
- `.react-pop` stays as today (opens upward, `left: 0`, `right: 0` on your
  own messages). Give it `z-index: 25` so it sits over the next bubble.

Check after the change: `.chat-feed > .msg:first-child .msg-tools`, the
desktop hover tint, and that the chip popover is not cut off by the feed's
scroll box on the first message.

## 5. TypeScript: `frontend/app.ts` chat section (lines 97 to 262)

- **Bubble click** (line 127): skip clicks inside `a, button, .react,
  .thread-foot`. Remove `.thread-link` and `.msg-receipt`.
- **Chips toggle** through the existing `[data-react]` handler (line 235).
  No change.
- **`showMine()`**: no change in logic. It still inserts a new chip in
  `REACTIONS` order, which matches the server's fixed order.
- The receipt toggle (`data-toggle` on `.msg-receipt-btn`) simply has no
  element left to act on. Nothing to remove in TS.

---

# Part B: reactions on task notes

## 6. Migration: `note_reactions`

Generate, then edit:

```sh
cargo loco generate model note_reactions emoji:string organisation:references task_note:references user:references
```

The generator writes `migration/src/m20261015_HHMMSS_note_reactions.rs`,
runs it and regenerates the entities. Then `cargo loco db down`, edit the
migration to match `m20261013_140537_reactions.rs`, then `cargo loco db
migrate` and `cargo loco db entities` again:

```rust
create_table(
    m,
    "note_reactions",
    &[("id", ColType::PkAuto), ("emoji", ColType::String)],
    &[("organisation", ""), ("task_note", ""), ("user", "")],
)
.await?;
// then, with execute_unprepared:
CREATE UNIQUE INDEX "idx-note-reactions-note-user-emoji"
  ON note_reactions (task_note_id, user_id, emoji);
CREATE INDEX "idx-note-reactions-org" ON note_reactions (organisation_id);
```

`("task_note", "")` gives a `task_note_id` column with a foreign key to
`task_notes` (`loco-rs-1.2.0/src/schema.rs` lines 708 to 714). All three
keys are `ON DELETE CASCADE`.

- **What it does:** one row per person, note and emoji. The unique index
  stops a duplicate and serves "reactions for these notes".
- **Cascades:** deleting a task deletes its notes and their reactions.
  Deleting a user deletes their reactions. Deleting an organisation deletes
  everything.
- **No CHECK on emoji:** the model holds the whitelist, as for chat.
- **Rollback:** `down` drops the table. Rolling back after use loses every
  note reaction and nothing else. A code-only rollback is also safe: older
  code never reads the table.
- Check the `src/models/_entities/` diff adds only `note_reactions.rs` and
  the new relations on `task_notes`, `users` and `organisations`.
- `src/app.rs` `truncate`: add `truncate_table(&ctx.db,
  note_reactions::Entity)` before `task_notes` (line 182).

## 7. Model: `src/models/note_reactions.rs`

Mirrors `src/models/reactions.rs`:

- Reuse `reactions::EMOJI`. Do not copy the list.
- `impl TenantEntity for Entity` on `OrganisationId`.
- `Model::set(db, note: &task_notes::Model, user_id, emoji, on) ->
  ModelResult<()>`. Refuses an emoji outside `EMOJI` with
  `field_error("emoji", "Pick one of the six reactions.")`. On: insert with
  `.on_conflict_do_nothing_on([TaskNoteId, UserId, Emoji])`. Off:
  `delete_many` filtered on organisation, note, user and emoji. The
  organisation always comes from `note.organisation_id`, never from the
  request.
- `Model::for_notes(db, org_id, note_ids: &[i64]) -> ModelResult<HashMap<i64,
  Vec<Self>>>`. One query, `in_tenant(org_id)`, `task_note_id IN (...)`,
  ordered by `created_at, id`. Callers pass `member.org.id`. Empty input
  returns an empty map with no query.

`src/models/task_notes.rs`, new finder:

- `Model::find_on_task(db, task: &tasks::Model, id) -> ModelResult<Self>`:
  `in_tenant(task.organisation_id)`, `task_id = task.id`, `id = ?`. Anything
  else is `EntityNotFound`.

## 8. Note views in `src/controllers/tasks.rs`

Replace the `serde_json::json!` note objects in `sheet_data()` (lines 869 to
883) with:

```rust
#[derive(Debug, Serialize)]
struct NoteView {
    id: i64,
    task_id: i64,
    author: String,
    color: &'static str,
    parts: Vec<Part>,
    at: String,
    reactions: Vec<ReactionChip>,
    picks: Vec<Pick>,
}
```

- `fn note_view(note, team, team_list, rows: &[ReactionRow], viewer_id, tz)
  -> NoteView`. `parts` uses `mention_parts` with `EVERYONE` added to the
  names (as `MessageView::new` does), so "@everyone" is highlighted.
- `sheet_data()` loads `note_reactions::Model::for_notes(&ctx.db,
  member.org.id, &ids)` once for all the task's notes and builds rows with
  `ReactionRow::named`. Two queries for notes, whatever their number.

## 9. Route

| Route | Does |
|---|---|
| `POST /tasks/{id}/notes/{note_id}/reactions` (form `emoji`, `on`) | Sets the viewer's reaction. Returns `tasks/_note.html` for that note. |

Handler `react_note`, checks in this order:

1. `CurrentMember`: signed in, approved, not removed. Pending and removed
   people get 403.
2. `member.require_own_membership()?`: a super admin acting inside another
   organisation gets 403. A reaction carries a person's name, so it needs a
   real membership, like documents.
3. `tasks::Model::find_in_org(db, member.org.id, id)`: else 404.
4. `task_notes::Model::find_on_task(db, &task, note_id)`: the note is on
   that task in that organisation. Else 404. A note id from another task, in
   this or any organisation, is 404.
5. `on` is "1" or "0" and the emoji is one of the six. Else 422 with
   `HX-Reswap: none` and an error toast, like chat's `react` (lines 794 to
   808).

Then `note_reactions::Model::set` with the note loaded from the database,
load its rows with `for_notes(member.org.id, &[note.id])`, and render
`tasks/_note.html` from that note. No notification, no websocket, no
`tasks-changed` trigger.

## 10. Templates

- New `assets/views/tasks/_note.html`, one note (moved out of the loop in
  `_sheet.html` lines 36 to 42):

  ```html
  <article class="note" id="note-{id}">
    avatar (md)
    <div class="note-main">
      <div class="note-head"><strong>{author}</strong><span class="meta text-xs">{at}</span>
        <button type="button" class="note-add" data-toggle="#note-picks-{id}" aria-expanded="false" aria-label="React to {author}'s note">{{<icon name="smile-plus" class="h-[18px] w-[18px]"/>}}</button>
        <div class="quick-react hidden" id="note-picks-{id}" role="group" aria-label="React to {author}'s note">
          six <button type="button"> from note.picks
        </div>
      </div>
      <p class="note-text">{parts}</p>
      <div class="reacts" id="note-reacts-{id}" role="group" aria-label="Reactions">
        per chip: <span class="react-wrap"><button type="button" class="react[ mine]" ...reaction attributes...>emoji count</button><span class="react-pop" role="tooltip">names</span></span>
      </div>
      [<button type="button" class="note-who-btn" data-toggle="#note-who-{id}" aria-expanded="false" aria-controls="note-who-{id}">Who reacted</button>]   (only when there are reactions)
      [<div class="note-who list hidden" id="note-who-{id}">one .who-row per chip: emoji, names, avatar stack</div>]
    </div>
  </article>
  ```

- **Reaction attributes**, the same on chips and on the quick bar's buttons:
  `hx-post="/tasks/{task_id}/notes/{id}/reactions"`,
  `hx-vals='{"emoji": "{emoji}", "on": "{0 if mine else 1}"}'`,
  `hx-target="#note-{id}"`, `hx-swap="outerHTML"`,
  `hx-sync="closest .note:replace"`, `data-note-react`, `aria-pressed`, and
  `aria-label` "React {emoji}" or "Remove your {emoji}" (chips use the chip
  label, as chat). The emoji comes from the server's `EMOJI` constant, so it
  is safe in the JSON. `hx-sync` makes a second quick tap replace the first
  request instead of racing it.
- A tap on a chip toggles your reaction, like chat (decision 9). The
  response swaps the whole article, so the quick bar and the who list come
  back closed.
- `_sheet.html`: the Notes loop becomes `<div class="notes">{% for note in
  notes %}{% include "tasks/_note.html" %}{% else %}<p class="meta
  m-0">No notes yet. Add context, an update or a decision.</p>{% endfor
  %}</div>`. The `{% else %}` empty state stays.

## 11. CSS and TypeScript for notes

CSS (mockup "NEW: notes with reactions"): `.notes`, `.note`, `.note-main`,
`.note-head` (position relative), `.note-add` (44px, `margin: -11px -12px
-11px auto`), `.note .reacts { margin: 6px 0 0; }` and `.note .react {
box-shadow: none; }` (no overlap, no ring), `.quick-react` (absolute above
the button, right-aligned, six 44px buttons, `.mine` outlined),
`.note-who-btn` (muted 13px text, `min-height: 44px`, no border; hidden in
`@media (hover: hover)`), `.note-who` (margin-top 8px).

TypeScript (small):

- Close an open `.quick-react` on Escape and on a click outside it and its
  button; set the button's `aria-expanded` back to false. Escape here must
  not close the task sheet (see the Escape note in section 17).
- Error toast: on `htmx:responseError` or `htmx:sendError` from an element
  with `data-note-react`, show `showToast({ kind: "error", message:
  "Couldn't save your reaction. Try again." })` (mockup 7). Skip it when
  `detail.xhr.status === 422`: `base.html`'s `responseHandling` marks 422
  as an error too, and a 422 already brings its own toast through
  `HX-Trigger`.
- No optimistic update. The note re-renders from the server's answer.

---

# Part C: @everyone on task notes

## 12. Notifications: `src/controllers/notifications.rs`

`note_added(ctx, member, task, note)` keeps its signature. Inside it:

```rust
// Only a person in their own organisation, in the browser, can tell everyone.
// Claude's bearer tokens and a super admin acting elsewhere cannot.
let everyone_allowed = member.via_session && !member.acting;
```

Order:

1. **Mentions** as today: `mentioned_ids(note.body, team)`, kind
   `MENTION`, "mentioned you on “{title}”". `told` = who got it. Tagging
   yourself is still a reminder.
2. **@everyone**, only when `everyone_allowed` and
   `mentions_everyone(&note.body)`: every id in `team`
   (`memberships::Model::team`, approved members only) except the writer and
   `told`. Kind `MENTION`, body `"mentioned everyone on “{title}”\n{quote}"`,
   link `/tasks?open={id}`. Add them to `told`.
3. **Assignees** as today, minus `told`: "commented on “{title}”".

Each person gets one notice. A name wins over @everyone. The writer gets
nothing from @everyone. Pending and removed people are not in `team`.

Callers do not change. `src/mcp/mod.rs` (line 575) passes its bearer member,
so @everyone is off there. Update the tool descriptions (lines 144 and 370)
to say "@username mentions notify people; @everyone does not notify anyone
from here."

The new notices are kind `MENTION`, so the unseen-mention email
(`due_mention_emails`) sends them, as it already does for chat @everyone.

## 13. The @ menu offers "everyone" first

`src/controllers/tasks.rs` `sheet_data()`:

- `mention_names`: `format!("{EVERYONE},{}", mention_names(&team_list,
  me))` when the team has at least one other person, else today's list.
- `everyone_hint`: "Notify the whole team (N people)", N = approved members
  minus the viewer; "1 person" when N is 1.
- `_note_form.html`: `data-everyone-hint="{{ everyone_hint }}"`.

Chat already puts `everyone` first outside DMs (`chat::mention_names`). Add
`data-everyone-hint` to `show.html` and `thread.html`: "Notify everyone in
#general (N people)" or "in {group}", N = the conversation's members on the
team minus the viewer. Not in DMs.

`frontend/app.ts` `showMentions()` (line 438):

- The `everyone` option is built with `createElement` and `textContent`:
  a `.mm-icon` span (its `innerHTML` is only the constant Lucide `users` SVG
  string), and `.mm-text` holding a `<b>` with "@everyone" and a `.mm-hint`
  span whose `textContent` is `field.dataset.everyoneHint`. The hint never
  goes through `innerHTML`.
- **No separator element.** Every `<li>` stays a real option, so
  `mentionActive`, the arrow keys and `aria-activedescendant` still line up.
  The line under "everyone" is CSS: `.mention-menu li[data-name="everyone"]
  { border-bottom: 1px solid var(--line-soft); margin-bottom: 4px; }`.
- Other rows stay `@name`.

CSS (mockup "NEW: mention menu"): `.mention-menu` width 300px, `li`
min-height 44px, `.mm-icon`, `.mm-text`, `.mm-hint`.

---

# Part D: emoji picker

## 14. Emoji data: `frontend/emoji.ts`

A TypeScript module, bundled into `app.js` by esbuild (`bin/build-assets`
runs `esbuild frontend/app.ts --bundle`). `app.ts` starts with `import {
CATEGORIES } from "./emoji";`. No third-party code, font or image.

```ts
export type Emoji = readonly [char: string, name: string];
export const CATEGORIES: readonly { id: string; label: string; icon: string; list: readonly Emoji[] }[] = [
  { id: "smileys", label: "Smileys and people", icon: "😀", list: [["😀", "grinning face smile happy"], ...] },
  ...
];
```

Seven categories, about 200 emoji:

| id | Label | Tab icon | About |
|---|---|---|---|
| smileys | Smileys and people | 😀 | 64 (faces, hands like 👍 👎 👏 🙌 🙏 👌 💪 👋 🤝) |
| animals | Animals and nature | 🐻 | 22 |
| food | Food and drink | 🍔 | 24 |
| activity | Activities | ⚽ | 22 (🎉 🎊 🎈 🎁 🏆 …) |
| travel | Travel and places | 🚗 | 20 (🚀 ✈️ 🏠 …) |
| objects | Objects | 💡 | 24 |
| symbols | Symbols | ❤️ | 24 (❤️ 💯 ✅ ❌ ⚠️ ✨ 🔥 …) |

Rules for the list:

- Unicode Emoji 12.0 (2019) or older, so it draws on any recent phone.
  Unverified per device. The mockup's 🪩 (Emoji 14) is left out.
- No skin-tone modifiers (U+1F3FB to U+1F3FF), no ZWJ (U+200D), no flags.
- Characters that show as text by default (❤ ✈ ⚠ ☺ ✌ ☀ ☁ ❄ ✏ ✂ ☎ and the
  like) are followed by U+FE0F, so they draw as emoji.
- Includes the six reactions and the brief's examples (✅ 🎉 🙏 🚀 👌 👀).
- `name`: the CLDR short name in lower case, plus up to three keywords, so
  "party" finds 🎉 🥳 🎊 🎈 🍾 and "thumbs" finds 👍 👎 (mockup 5.3).

Size: about 200 entries of 20 to 35 bytes, so roughly 6 KB of source and 5
KB minified (estimate, not measured). It ships with `app.js`, so there is no
loading state (mockup 5.7).

## 15. Markup: the smiley in the field

`assets/views/components/icons.html`: add `smile`, `smile-plus` and
`keyboard`. Paths are in the mockup's sprite (lines 295 to 297) and match
Lucide. Same `{% elif name == "..." %}` pattern.

Chat composer (`chat/show.html` line 24) and thread composer
(`chat/thread.html` line 23) become:

```html
<div class="field">
  <textarea id="composer-body" ...as today...></textarea>
  <button type="button" class="field-emoji" data-emoji-for="composer-body" aria-label="Emoji" aria-expanded="false" aria-controls="emoji-picker">{{<icon name="smile" class="ic-smile h-[22px] w-[22px]"/>}}{{<icon name="keyboard" class="ic-keyboard h-[22px] w-[22px]"/>}}</button>
</div>
```

The note box (`tasks/_note_form.html`) gets the same wrapper with
`data-emoji-for="note-body"`.

- Text must start right of the smiley. Chat: `.composer textarea {
  padding-left: 46px; }` in the stylesheet. Note box: the textarea's
  Tailwind `px-4` would beat a `@layer components` rule, so change its
  classes in the template from `px-4` to `pr-4 pl-[46px]`.
- `.field-emoji` is absolute, bottom-left, 44px.
- Both icons render; CSS shows the keyboard only on phones while the picker
  is open (mockup 5.1): `.field-emoji .ic-keyboard { display: none; }` and,
  in `@media (pointer: coarse)`, `[aria-expanded="true"] .ic-smile { display:
  none; }` and `[aria-expanded="true"] .ic-keyboard { display: block; }`.
  The script switches `aria-label` between "Emoji" and "Show keyboard".
- The mention menu is appended to `field.parentElement`, now `.field`. The
  rule `:has(> textarea[data-mentions]) { position: relative; }` still
  covers it.

## 16. The picker element

`app.ts` builds one picker on first use and moves it where it is needed,
like `mentionMenu` (line 413). Built with `createElement` and
`textContent`, never `innerHTML` with data or with the search text. The only
`innerHTML` is the constant SVG strings for the search, clear and clock
icons.

**Every button is `type="button"`.** The popover sits inside the chat form
and `#note-form`, and a default `submit` button would send the message or
post the note. One helper, `pickerButton()`, creates every picker button
(grid, tabs, clear) and sets `type = "button"`. The smiley toggle in the
templates is `type="button"` too. Check before the PR: `grep -n
'createElement("button")' frontend/app.ts` shows only the helper and
existing code that already sets a type.

```
<div id="emoji-picker" class="ep" role="dialog" aria-label="Emoji">
  .ep-search: search icon, <input type="search" placeholder="Search emoji" aria-label="Search emoji" autocomplete="off"> (no name attribute), clear button "Clear search" (44px)
  .ep-tabs: 8 buttons (Recent with a clock icon, then the 7 category icons), aria-label per category, aria-pressed on the current one
  .ep-body (scrolls): per section a .ep-label and a .ep-grid of 8 columns of 44px buttons
  .ep-empty when search finds nothing
  .ep-foot (desktop only): focused emoji, its name, "Enter to add · Esc to close"
</div>
```

- The search input has no `name`, so neither `ws-send` nor the note form
  sends it.
- Category tabs are plain buttons with `aria-pressed`, not ARIA tabs. They
  scroll `.ep-body` to their section. A scroll listener updates which tab is
  pressed.

## 17. Behaviour

**Opening.** A click on `[data-emoji-for]` toggles the picker for that
field. `matchMedia("(pointer: coarse)")` decides the mode.

- **Phone, docked** (mockup 5.1, 6.5): the picker goes in after the field's
  form (`form.after(picker)`), class `ep-dock` (336px tall, bottom padding
  for the safe area). Then call `field.blur()`, so iOS drops the keyboard.
  In chat it sits in `.chat-screen`'s flex column, so the feed shrinks and
  stays visible. In the task sheet it sits under the note form inside
  `.sheet-body`; then `form.scrollIntoView({ block: "end" })`. The composer
  drops its safe-area padding while docked (`.composer.docked`). The search
  box is not focused, since that would raise the keyboard.
- **Desktop, popover** (mockup 5.2): appended to the form row that holds
  `.field` (the `.composer`, or the note form's row), class `ep-pop`:
  absolute, `left: 0`, `bottom: calc(100% + 8px)`, 376px wide (at most
  `100vw - 32px`), 430px tall. If there is not 430px above the field inside
  its scroll box (the task side sheet), add `ep-below` to open downward. The
  search box gets focus.
- **Feed stays at the bottom.** Before docking or undocking in chat, note
  whether `#chat-feed` is scrolled to the end. If it was, call
  `scrollToEnd(feed)` afterwards, since the feed's height changes.

**Inserting** (mockup 5): a pick calls `field.setRangeText(emoji,
field.selectionStart, field.selectionEnd, "end")` and dispatches an `input`
event, so autogrow and the mention menu update. A blurred textarea keeps its
selection, so on a phone this inserts at the cursor without bringing the
keyboard back. If `field.maxLength > 0` and the result would pass it, do
nothing. The picker stays open.

**Back to typing (phone).** A `pointerdown` or `focus` on the field while
the picker is docked closes it and lets the keyboard come up. The keyboard
button does the same and calls `field.focus()` inside the tap. Tapping the
feed leaves the picker open.

**Closing on desktop:** Escape, the smiley again, or a `pointerdown` outside
the picker, the field and the smiley. Focus goes back to the field.

**Always:** when the field leaves the page (the task sheet re-renders after a
note is posted; `invalid_form` swaps `#note-form`), the picker goes with it.
On the next open, check `picker.isConnected` and reset every
`[data-emoji-for]` to `aria-expanded="false"`. Sending a chat message leaves
the picker open.

**Escape inside the task sheet.** The task sheet is a modal `<dialog>`, and
Escape closes it by default. The picker and the quick bar handle Escape in a
capture-phase `keydown` listener and call `preventDefault()`. Unverified
that this stops the dialog's `cancel` in every browser. Fallback: a
capture-phase `cancel` listener on `document` that calls
`preventDefault()` when a picker or quick bar was open at that keydown. The
thread page's "Escape goes back" handler already skips `defaultPrevented`
events (line 268).

**Enter in the search box** inserts the first result and calls
`preventDefault()`, so it never submits the chat form or the note form.

**Keyboard** (mockup 5.6): the grid uses a roving `tabindex` (one button at
0, the rest -1). Arrow Left/Right move by one, Up/Down by eight, Home/End to
the ends of the section. Enter or Space inserts. ArrowDown from the search
box moves into the grid. Tab moves search, then tabs, then grid. The footer
shows the focused or hovered emoji and its name.

**Search** (mockup 5.3): lower-case the query, split on spaces. An emoji
matches when every query word starts some word in its name. Results show
under one "Results" label, tabs hidden. No match shows `No emoji match
"brb"` and "Try a word like smile, heart or rocket." (mockup frame
`p-pick-none`), built with `textContent` so the search text is never parsed
as HTML. The clear button empties the search.

**Recent** (mockup 5.4): `localStorage` key `collab:emoji-recent`, a JSON
array of up to 16, newest first, no repeats. Every read and write is in
`try/catch`, like `collapsedLanes()` (line 369); bad JSON or no storage means
an empty list. Entries not in `CATEGORIES` are dropped. Recent and its tab
are hidden while the list is empty (mockup frame `p-pick-first`).

## 18. Picker CSS

Copy the mockup's "NEW: emoji picker" and "NEW: composer" blocks: `.field`,
`.field-emoji` (`[aria-expanded="true"] { color: var(--link); }`), `.ep`,
`.ep-dock`, `.ep-pop`, `.ep-below` (`top: calc(100% + 8px); bottom: auto`),
`.ep-search`, `.ep-tabs`, `.ep-tab` (inactive greyed, pressed one in colour
with the amber underline), `.ep-body` (`overflow-y: auto`), `.ep-label`,
`.ep-grid`, `.ep-grid button:focus-visible` (chip background plus accent
ring), `.ep-foot` (hidden in `pointer: coarse`), `.ep-empty`. Everything
uses the existing tokens, so dark mode needs no extra rules.

---

# Tests

## Rust unit tests

`src/views/reactions.rs` (moved from `chat.rs`): the existing
`chips_keep_the_fixed_order_and_put_you_first` and
`picks_mark_only_your_own_reactions` move with the code. New:
`ReactionRow::named` uses "Former member" for someone off the team.

`src/controllers/chat.rs`:

- Update every `ReadMark` literal to three values.
- `groups_say_everyone_once_all_who_were_there_have_read`: everyone read
  gives `all` and "Read by everyone". Someone who joined after the message
  and has not read it does not block `all`. A member who has not read it
  does. Someone not in `names` (removed or pending) does not count.
- `dms_say_read_once_the_other_person_has_read`: also asserts `all`.
- `nobody_left_to_read_means_no_receipt`: unchanged.

`src/controllers/chat_ws.rs`: the `read_by` helper builds `Receipt` with
`all: false`. No logic change.

## Model tests

New `tests/models/note_reactions.rs` (add to `tests/models/mod.rs`):

- `set` on twice gives one row; off twice gives none.
- "😀", a bare "❤" and "" are refused with a field error and leave no row.
- `for_notes` groups by note and ignores another organisation's rows.

`tests/models/task_notes.rs`: `find_on_task` refuses a note from another
task and from another organisation.

## Request tests

New `tests/requests/note_reactions.rs` (add to `tests/requests/mod.rs`):

- **Setting:** alice reacts 👍 on bob's note: one row, the response is
  `<article class="note" id="note-{id}">` with `class="react mine"` and
  count 1. Again with `on=1`: still one row. `on=0` twice: no rows and an
  empty `#note-reacts-{id}`.
- **Chips toggle:** the chip's `hx-vals` says `"on": "0"` for alice's own
  👍 and `"on": "1"` for bob's ✅. The quick bar's six buttons match.
- **Shared chip:** bob's 👍 and alice's 👍 show "👍 2, you reacted" for
  alice; bob sees the same with himself. Chips are in `EMOJI` order.
- **Whitelist:** the same bad inputs as chat get 422, `HX-Reswap: none` and
  no rows. A bad `on` gets 422.
- **Access:** gina (Globex) gets 404. A pending member gets 403. A removed
  member gets 403 (extend `removing_someone_locks_them_out_of_everything` in
  `tests/requests/remove_member.rs`). alice posting her own task's id with
  a note id from another Acme task gets 404.
- **Acting super admin:** a super admin acting in Acme (cookie
  `acting_org={acme.id}`, as in `tests/requests/claude.rs` line 174) gets
  403 and no row.
- **No notice:** after reacting, `notifications::Entity` count is 0.
- **Sheet:** `GET /tasks/{id}` shows the chips, the popover names, the
  "Who reacted" button only on a note with reactions, and the empty state
  "No notes yet" on a task with none.
- **Cascade:** deleting the task removes its note reactions.

`tests/requests/tenancy.rs`:

- `build_globex` keeps the note it creates (`note: task_notes::Model` on
  `Globex`).
- Add `(format!("/tasks/{}/notes/{}/reactions", globex.task.id,
  globex.note.id), json!({ "emoji": "👍", "on": "1" }))` to `posts`.
- New: alice posts to `/tasks/{acme_task.id}/notes/{globex.note.id}/reactions`
  (her own task, Globex's note). 404, and the body does not contain
  `SECRET`. Needs an Acme task in the test.
- Assert `note_reactions::Entity` count is 0 at the end.

`tests/requests/notifications.rs` (use `team_of_four`):

- `everyone_in_a_note_tells_the_whole_team_once`: samir writes "@everyone
  stock count Friday" on a task. raj, shusma and kreepa each get one
  `mention` notice "mentioned everyone on “…”", each with link
  `/tasks?open={task.id}`. samir gets none. A pending member gets none.
- `a_name_wins_over_everyone_in_a_note`: "@shusma @everyone" gives shusma
  one "mentioned you on" notice, the others one "mentioned everyone".
- `everyone_in_a_note_replaces_the_comment_notice`: an assignee gets the
  @everyone notice, not a second "commented" notice.
- `mentioning_yourself_and_everyone_in_a_note`: samir writing "@samir
  @everyone" gets only the self reminder; the others once each.
- `an_acting_super_admin_cannot_notify_everyone`: a super admin acting in
  Acme posts "@everyone hi" as a note. No @everyone notices; assignees still
  get "commented".
- The sheet's textarea has `data-mentions="everyone,..."` and
  `data-everyone-hint="Notify the whole team (3 people)"`, and the note
  shows `<span class="mention">@everyone</span>`.

`tests/requests/mcp.rs`:

- `the_connector_cannot_notify_everyone`: `add_task_note` with "@everyone
  hello" sends no `mention` notices; assignees still get "commented". With
  "@bob @everyone", bob gets one mention and nobody else gets one.

`tests/requests/chat.rs`:

- `the_sheet_says_who_read_your_message`: in a group, after bob opens it,
  alice's sheet shows "bob" under "Read by"; after carol too, "Everyone" and
  "2 people". bob's sheet for alice's message has no "Read by". A thread
  reply's sheet has none.
- `late_joiners_and_pending_people_do_not_block_the_read_tick`: alice posts
  in a group; bob and carol read it; dave then joins the group (a
  `conversation_members` row created after the message) and has not read
  it; a pending member has a `conversation_members` row with an old
  `last_read_at`. alice's page shows `class="tick tick-read"` for that
  message.
- Receipt tests (`dms_...`, `groups_count_who_has_read`): assert on
  `class="tick"` or `class="tick tick-read"` and the sr-only text instead of
  `msg-receipt`, avatars and `</svg>Read</span>`. "Only the sender sees
  receipts" becomes `!contains("id=\"receipt-")`.

Other chat tests to update:

- `tests/requests/chat_reactions.rs`: the chip markup is unchanged, so only
  the empty-slot string gains `role="group" aria-label="Reactions"`.
- `the_sheet_lists_you_first_then_others_in_order`: add "Reactions · N".
- `tests/requests/chat_threads.rs` line 222: `thread-link` becomes
  `thread-foot`.

## Render tests (`tests/views/render.rs`)

- `renders_read_receipts_as_live_swaps` becomes ticks: `id="receipt-42"
  hx-swap-oob="true"`, `tick-read` only when `all`, no avatars, and an empty
  receipt renders the grey tick.
- `renders_reactions_as_live_swaps`: add the `role` and `aria-label`; the
  rest stays.
- New `renders_a_message_as_one_block`: `chat/_message.html` with a thread
  line, two chips and `read_receipts: true` has, in order: the bubble,
  `.stamp` holding `id="receipt-{id}"` with `stamp-pad-tick`,
  `id="thread-{id}"` inside the bubble, then `id="reacts-{id}"` and
  `id="tools-{id}"` after it. No `msg-meta`. Without `read_receipts`, no
  `stamp-pad-tick`.
- New `renders_a_note_with_reactions`: `tasks/_note.html` has `id="note-5"`,
  six quick-bar buttons with `hx-post="/tasks/3/notes/5/reactions"` and
  `hx-sync="closest .note:replace"`, chips with the same, `hidden` on the bar
  and the who list, every `<button` with `type="button"`, and the note text
  escaped.

## Emoji data guard

New test in `tests/views/` that reads `frontend/emoji.ts` with
`include_str!`, pulls out the first quoted string of each entry, and checks:

- between 180 and 220 emoji, no duplicates;
- all six `reactions::EMOJI` present;
- none contains U+200D or U+1F3FB to U+1F3FF;
- every character below U+3300 is followed by U+FE0F, except a short list
  that draws as emoji by default (✅ ❌ ❓ ❗ ⭐ ⚡ ⚽ ⚾ ⏰ ⌛ ⏳ ✨ ➕ ➖ ⛔ ☕
  ⛄ ⛳ ⛺ ⛵ ✊ ✋ ⭕). This catches ✈ ⚠ ☺ ❤ and the like.

# PR body

**Migration:** "Adds a `note_reactions` table: one row per person, note and
emoji, with a unique index on (note, person, emoji) and foreign keys that
cascade from the note, the person and the organisation. Nothing else
changes. It can be rolled back: `down` drops the table, which loses only
note reactions. Older code ignores the table, so a code-only rollback is
also safe."

**Known differences from the mockup**, in plain words:

- The @ menu shows teammates as names, without their avatars.
- There is no "sending" clock and no "Not sent" message. Messages appear
  once the server saves them, as today.
- Who read a message is listed only in the message sheet. Desktop has no
  hover list for readers.
- A grey tick can turn read when someone leaves a group, because they no
  longer count.
- Chips stay in a fixed order instead of "most used first", because a tap
  now toggles and a moving chip could be tapped by mistake.
- Notes from the Claude connector still show "@everyone" highlighted, but
  nobody is told.
- New live messages from the same person do not get the joined run corners
  until the page reloads, as with run grouping today.
- The picker under the note box closes after a note is posted, because the
  task sheet re-renders.

**Real-phone check:** done on a real iPhone and Android phone (manual check
12). If no real phone was available, the PR says so plainly and lists that
check as not done.

# Screenshots

Folder `chat-emoji/` on `pr-screenshots`, linked as
`https://raw.githubusercontent.com/rajgurung/collaboration_tool/pr-screenshots/chat-emoji/<file>.png`.
Check each URL returns 200. Data set up by hand on a fresh local database
(sign up "Himalayan Ritual" as raj, join and approve shusma, kreepa and
samir, post the brief's messages, notes, reactions and replies). No seed
file committed.

Each at 1440px and 390px, light and dark, unless noted:

| Shot | Mockup |
|---|---|
| General: chips, thread footer, ticks, a run | 1 |
| Desktop hover popover on a chip (1440 only) | 3 |
| Action sheet from a bubble, with Reactions and "Read by" | 3 |
| Thread page | 4 |
| Picker open after two picks (phone docked, desktop popover) | 5 |
| Picker search "party" and no match (390 only) | 5 |
| Task sheet: note chips, quick bar open, "Who reacted" list (390) | 6 |
| Note box @ menu with "everyone" first | 6 |
| Picker in the note box (phone docked, desktop side sheet) | 6 |
| A note with @everyone as posted (390 light only) | 7 |

# Manual checks

On the local app with realistic data, at 390px and 1440px, light and dark:

1. A message with 👍 from two people shows one chip "👍 2" on the bubble's
   edge. Your own chip is amber. Chips start left on others' messages and
   right on yours. A short "Yes" with three chips does not run off screen.
2. Chips never cover the time. The avatar sits level with the bubble.
3. Every message shows its time inside the bubble. Your messages show a grey
   tick, then a `--link` tick once everyone has read. Open the conversation
   as bob in another browser and watch the tick change live.
4. A message with replies shows the footer inside the bubble. A reply from
   another browser updates it live. The first reply makes it appear.
5. Tapping a chip adds or removes your reaction, and no chip moves. Desktop:
   hovering or focusing a chip shows who reacted. Phone: tapping the bubble
   opens the sheet with Reactions and Read by. Tapping the footer opens the
   thread. Selecting text does not open the sheet.
6. The desktop hover toolbar still adds and removes reactions.
7. Live: a reaction from another browser appears without reload. Restart the
   server: the feed reloads with chips, footers and ticks.
8. Thread page: the parent shows chips, no footer. Replies show time, no
   tick.
9. Task sheet: smiley-plus, pick ❤️: chip "❤️ 1", bar closes. Tap the chip:
   it goes. Phone: "Who reacted" opens the list. Desktop: hover shows names.
   Escape closes the bar, not the sheet. Turn the network off and tap: one
   error toast.
10. Type "@" in the note box: "everyone" first with "Notify the whole team
    (3 people)" and a line under it; arrow keys move through every row.
    Post "@everyone stock count Friday" as samir: raj, shusma and kreepa
    each get one bell notice that opens the task.
11. Picker in chat, thread and note box: inserts at the cursor (put the
    cursor mid-text first), stays open, never sends the message or posts the
    note on its own, Recent fills after the first pick and survives a
    reload. Search "party" and "brb". Arrow keys, Enter, Escape on desktop.
    An outside click closes it on desktop. The note box text starts right of
    the smiley.
12. **Real phone (iPhone and Android), mandatory:** the picker replaces the
    keyboard, tapping the field or the keyboard button brings the keyboard
    back, the composer is not pushed off screen, and a feed at the bottom
    stays at the bottom. Brief example: 🎉 🎉 🙏 then send reads "🎉🎉🙏".
13. In private browsing, the picker works with no Recent row and no console
    errors.
14. Known and accepted: a connector note with "@everyone" is highlighted but
    tells nobody; new live messages do not get joined corners until reload;
    the note picker closes after posting.

# Build sequence

Small steps, one commit each. After every step:

```sh
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

and, when a step touches `frontend/` or templates, `bin/build-assets` (it
fetches the pinned Tailwind and esbuild into `.tools/` on first run, then
builds `assets/static/css/app.css` and `assets/static/js/app.js`). For live
work use `bin/dev`. esbuild does not type-check; if Node is available, `npx
-y -p typescript tsc -p frontend` is a useful extra check (optional, not in
CI, unverified on this machine).

1. **Move the reaction view types** to `src/views/reactions.rs`, add
   `ReactionRow::named`. No visible change.
2. **Ticks in the model:** `ReadMark` with join time, `Receipt.all`, new
   text. Unit tests.
3. **Chat templates:** `_message.html`, `_receipt.html`,
   `_thread_link.html`, `_reactions.html`, `_actions.html` with "Read by",
   and the `actions()` change. Update the chat request and render tests;
   add the late-joiner test.
4. **Chat CSS and TypeScript:** sections 4 and 5. Manual checks 1 to 8.
5. **Migration and model:** generate `note_reactions`, `db down`, add the
   index, migrate, entities. `note_reactions.rs`, `find_on_task`, truncate
   in `app.rs`. Model tests. Run `cargo loco db down` and `cargo loco db
   migrate` once more to prove the rollback.
6. **Note reactions route and UI:** `NoteView`, `react_note` (with
   `require_own_membership`), `_note.html`, `_sheet.html` (keep the empty
   state), CSS, the small TS. Request, tenancy and render tests. Manual
   check 9.
7. **@everyone on notes:** `everyone_allowed` inside `note_added`, the MCP
   tool text, `mention_names` and `everyone_hint`. Notification and MCP
   tests.
8. **@ menu "everyone" row:** `showMentions` changes, hint on the chat and
   thread composers, CSS. Manual check 10.
9. **Icons and the smiley button:** `icons.html`, the `.field` wrapper in
   three templates, the note textarea's `pr-4 pl-[46px]`, CSS. The button
   does nothing yet.
10. **Emoji data:** `frontend/emoji.ts` and the data guard test.
11. **Picker, desktop:** element, `pickerButton()`, popover, search, tabs,
    Recent, keyboard, insert, closing. The `createElement("button")` grep.
    Manual check 11 at 1440px.
12. **Picker, phone:** docking with `field.blur()`, back to typing, the
    keyboard button, `scrollToEnd`, the task sheet's scroll, Escape in the
    dialog. Manual checks 11 to 13 at 390px and on a real phone.
13. **Screenshots** and the PR body (migration, known differences,
    real-phone result).

# Risks

1. **Live swap targets.** The receipt moves into the stamp and the thread
   slot into the bubble; the ids stay. `renders_a_message_as_one_block` and
   the table in section 3 cover it. Check live updates with two browsers.
2. **Tenancy on note ids.** The route resolves the task in the member's
   organisation, then the note on that task. The "note from another task"
   and "own task, Globex note" tests catch a route that checks only one.
3. **Who can tell everyone.** `note_added` decides from `via_session` and
   `acting`, so a new caller cannot switch it on by mistake. Tests cover the
   connector and an acting super admin.
4. **Notice noise.** @everyone on a note tells the whole team, and unseen
   ones are emailed like chat @everyone. Fine for four people.
5. **Escape in the task sheet.** Unverified that `preventDefault()` on
   keydown stops a modal dialog's cancel in Safari. The `cancel` fallback
   covers it. Check in Safari and Chrome.
6. **Phone keyboard.** Unverified on real devices: that `field.blur()`
   drops the iOS keyboard, that `setRangeText` on a blurred field inserts at
   the old cursor, and that `field.focus()` inside the keyboard-button tap
   brings the keyboard back. Typing in the picker's search on a phone raises
   the keyboard over the dock; acceptable.
7. **Emoji rendering.** Emoji 12.0 or older, with U+FE0F where needed,
   should draw on any recent phone. Unverified per device.
8. **Popover clipping in the side sheet.** The `ep-below` flip handles a note
   box near the top. Check with a task that has no notes.
9. **Composer length.** `maxlength="1000"` counts emoji as two. The picker
   refuses an insert past the limit. Harmless; not fixed here.
10. **Tick meaning.** "Read by everyone" ignores people who joined after the
    message (choice 4). Someone leaving can turn a tick to read. Listed in
    the PR body.

# Open points for review

1. **No avatars in the @ menu** (choice 6). OK to leave for later?
2. **No "sending" and "Not sent" states** (choice 5). A separate change.
3. **Day in message times.** Every bubble shows "09:31" only, as today.
   Unchanged from the threads plan's open point 2.

# Review changes (round 1)

| Change | Asked for by |
|---|---|
| Chip taps toggle in chat and notes; the chip no longer opens the sheet. Desktop keeps the hover/focus popover on chips; phones see who reacted by tapping the bubble. | Raj (decision A) |
| Chips keep the fixed `EMOJI` order instead of "most used first", so a chip never moves under a finger (choice 1). | Raj (decision A) |
| Note "who reacted" on phones opens from a "Who reacted" text button after the chips (choice 2). | Raj (decision A) |
| One PR; the split discussion is removed. | Raj (decision B) |
| Every picker button is `type="button"` through one helper, plus a grep check. | Staff review 1 |
| `field.blur()` when docking on phones; back to typing on `pointerdown` or `focus` of the field. | Staff review 2 |
| The notes loop keeps its `{% else %}` empty state. | Staff review 3 |
| Note textarea classes change from `px-4` to `pr-4 pl-[46px]`. | Staff review 4 |
| The note reaction error toast skips `xhr.status === 422`. | Staff review 5 |
| No separator `<li>` in the @ menu; a CSS border on the everyone option instead. | Staff review 6 |
| `scrollToEnd` after docking or undocking when the feed was at the bottom. | Staff review 7 |
| `stamp-pad-tick` sizes the pad by whether there is a tick. | Staff review 8 |
| `hx-sync="closest .note:replace"` on note reaction buttons. | Staff review 10 |
| Request test: a late joiner and a pending member do not block the read tick. | Staff review 11 |
| Emoji data guard requires U+FE0F after text-default characters. | Staff review 12 |
| PR body and manual checks list connector @everyone highlighting, live run corners and the picker closing after posting a note. | Staff review 13, 14, 15 |
| Test asserts each @everyone note notice links to `/tasks?open={id}`. | Product manager |
| The real-phone check is mandatory; the PR says so if no phone was available. | Product manager |
| The PR body lists the known differences from the mockup in plain words. | Product manager |
| `everyone_allowed` is worked out inside `note_added` from `via_session && !acting`, not passed by the caller. The MCP test stays. | Security 1 |
| `react_note` calls `member.require_own_membership()?`; tests for an acting super admin on reactions and on @everyone. | Security 2 |
| Tenancy test: alice's own task id with Globex's note id gives 404 and no `SECRET`. | Security 3 |
| Stated: `set()` takes the organisation from the note, `for_notes` filters on `member.org.id`, the handler renders the note loaded from the database. | Security 4 |
| The "Read by" ownership check compares `message.user_id` with `member.user.id`; the "bob sees no Read by" test stays. | Security 7 |
| The @ menu's everyone row uses `createElement` and `textContent`; only the constant SVG goes through `innerHTML`. | Security 8 |
| The picker's "No emoji match" text uses `textContent`. | Security 9 |
