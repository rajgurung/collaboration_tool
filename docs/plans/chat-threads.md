# Chat threads and reactions: build plan

Status: Draft for staff review · Owner: Raj · 13 October 2026

Builds the approved brief `docs/briefs/chat-threads.md` and the approved
mockup (local copy
`/private/tmp/claude-501/-Users-rajg-Codes-collaboration-tool/83f0d096-c371-4303-b30b-b78218969c3d/scratchpad/threads-mockup.html`).
Section and frame numbers below (1a, 6c and so on) refer to the mockup.

## Summary

Two PRs. Reactions ship first, threads second.

- **PR 1, branch `chat-reactions` (cut from `main`):** the `reactions` table,
  toggling one of six emoji, live updates, who reacted, the tap sheet, the
  desktop hover toolbar, and the new two-row `.msg` grid.
- **PR 2, branch `chat-threads` (rebased on `main` after PR 1 merges):**
  `messages.parent_id`, keeping replies out of every main-feed query, the
  thread page, live reply counts, and the "reply" notice.

Why two: PR 1 lays the shared groundwork (message grid, `.msg-extra` slot,
tap sheet, toolbar, a second websocket event, reload on reconnect) with no
risk of leaking data. PR 2 then holds the risky part alone: the main-feed
exclusion and the new access checks on a parent message. Each PR is useful on
its own, and each is small enough for Michael Scott to review properly. One PR
would touch every chat file at once and be hard to review.

## Approved decisions this plan follows (Raj, 13 October 2026)

From the brief:

- Threads are one level deep, in channels, groups and DMs.
- No "also send to channel".
- A reply notifies the parent's author, earlier repliers and anyone
  @mentioned. Each person is told once, a mention wins, and the reply's
  author is never told.
- Replies do not count toward the chat unread number, the chat list preview
  or read receipts.
- Reactions come from a fixed set of six: 👍 ❤️ 😂 🎉 👀 ✅. Tapping one again
  removes it. They update live and send no notification.
- No emoji picker.
- If a parent is deleted with its author's account, its replies go too.

From the mockup:

- A tap on the bubble opens the action sheet. No long-press. A tap that ends a
  text selection does not open it. Taps on a chip, the reply line or a
  receipt do their own thing.
- Tapping a chip toggles your reaction directly.
- Who reacted shows in the tap sheet on a phone and in a hover popover on
  desktop.
- On desktop, hovering a message shows a toolbar. Clicking the bubble opens
  the sheet as a centred dialog.
- A thread has its own URL, opens full screen and replaces the conversation
  pane on desktop. It has a Back button. Escape also goes back on desktop.
- Notice wording and the green reply icon (section 5).
- A long thread opens at the newest reply. No "Threads" list.
- One "This thread isn't available" 404 page for a missing thread and for
  lost access.
- A failed reaction rolls back with a toast.
- `.msg` becomes a two-row grid, so all six chips fit at 390px.

## Review changes (13 Oct)

The staff review approved this plan with six required changes. They override
the sections below where they differ. PR 2 follows them too.

1. **Websocket filtering checks the conversation.** The routing function is
   `wants(event, conversation_id, thread_id)`. It checks the conversation
   first for every event, `Reactions` included. `ChatHub` is one broadcast
   for the whole process, so without this a socket would get another
   organisation's HTML. Each event type has an "other conversation gives
   false" unit test. PR 1 ships `wants(event, conversation_id)`, since no
   socket follows a thread yet; PR 2 adds the `thread_id` argument.
2. **Self-mentions (PR 2).** In a thread reply, a self-@mention keeps the
   existing "mentioned yourself" reminder. The reply's author gets no reply
   notice and no @everyone notice. This replaces section 16's step 1 rule
   for mentions and open point 1.
3. **Set, not toggle.** The client sends the state it wants (`on=1` or
   `on=0`). The server inserts with do-nothing-on-conflict, or deletes, so a
   retry is safe. `toggle` is renamed `set`. Tests run each direction twice.
   - Taps on a chip are ignored while its request is still running.
   - On failure the client undoes only its own change. It does not restore
     the saved outerHTML, which could erase someone else's live reaction.
   - SeaORM 2.0.4: `insert(..).on_conflict(..do_nothing..).exec()` returns
     `Err(RecordNotInserted)` on a conflict. Use
     `.on_conflict_do_nothing_on([...])` instead.
4. **Escape (PR 2).** Escape does nothing when the focus is in a textarea or
   input that has text.
5. **Access fixtures create the cases they claim.**
   - "Not a member": a group without bob (alice and carol), checked as bob.
   - "Pending gets 403": a person who has joined but is not approved yet.
   - Every reactions route is covered by these.
6. **The live thread line targets the parent (PR 2).** Render it with the
   parent id. A render test asserts `id="thread-{parent}"`.

---

# PR 1: reactions

## 1. Migration: `reactions` table

Generate it, then edit: `cargo loco generate model reactions emoji:string
organisation:references message:references user:references`. The file is
`migration/src/m20261013_HHMMSS_reactions.rs`.

```rust
create_table(
    m,
    "reactions",
    &[("id", ColType::PkAuto), ("emoji", ColType::String)],
    &[("organisation", ""), ("message", ""), ("user", "")],
)
.await?;
// then, with execute_unprepared:
CREATE UNIQUE INDEX "idx-reactions-message-user-emoji"
  ON reactions (message_id, user_id, emoji);
CREATE INDEX "idx-reactions-org" ON reactions (organisation_id);
```

`create_table` makes the three foreign keys with `ON DELETE CASCADE`, as
`m20261007_144248_messages.rs` does. So a reaction goes when its message,
its user or the organisation goes.

- **What it does:** stores one row per person, message and emoji. The unique
  index stops the same person adding the same emoji twice. It leads with
  `message_id`, so it also serves "all reactions for these messages".
- **No CHECK on the emoji column.** The model holds the whitelist, so the
  set can change later without a migration.
- **Rollback:** `down` drops the table. It can be rolled back at any time.
  Rolling back after use loses every reaction. Nothing else depends on it.
- Run `cargo loco db migrate` then `cargo loco db entities`. Check the diff
  of `src/models/_entities/` only adds `reactions.rs` and the new relations
  on `messages`, `users` and `organisations`.
- `src/app.rs` `truncate`: add `truncate_table(&ctx.db, reactions::Entity)`
  before `messages`.

## 2. Model: `src/models/reactions.rs`

- `pub const EMOJI: [&str; 6] = ["👍", "❤️", "😂", "🎉", "👀", "✅"];`
  This is also the display order, so chips never jump around (6c). Note that
  ❤️ is two code points (U+2764 U+FE0F). Compare exact strings. A bare ❤
  without U+FE0F is refused.
- `impl TenantEntity for Entity` on `OrganisationId`, like every other model.
- `Model::toggle(db, message: &messages::Model, user_id, emoji) ->
  ModelResult<bool>`. Refuses an emoji not in `EMOJI` with a field error
  ("Pick one of the six reactions."). Deletes the matching row. If nothing
  was deleted, inserts one with `on_conflict((message_id, user_id,
  emoji)).do_nothing()`, so a double tap never errors. Returns true when it
  added. The organisation comes from `message.organisation_id`.
- `Model::for_messages(db, org_id, message_ids: &[i64]) ->
  ModelResult<HashMap<i64, Vec<Self>>>`. One query, `in_tenant`,
  `message_id IN (...)`, ordered by `created_at, id`. This is the only
  reactions query a feed makes, so there is no N+1.

`src/models/messages.rs`, new:

- `Model::find_in_conversation(db, conversation: &conversations::Model, id)
  -> ModelResult<Self>`. `in_tenant(conversation.organisation_id)`,
  `id = ?`, `conversation_id = conversation.id`. Anything else is
  `EntityNotFound`. Every route that takes a message id goes through this.

## 3. Views in `src/controllers/chat.rs`

- `ReactionRow { emoji: String, user_id: i64, username: String }`. Built
  from `reactions::Model` plus the names map ("Former member" when the
  person has left; their reaction still counts).
- `ReactionChip { emoji, count, mine, names: Vec<String>, label }`.
  `fn chips(rows: &[ReactionRow], viewer_id) -> Vec<ReactionChip>` walks
  `EMOJI` in order, skips empty ones, puts "You" first in `names` and keeps
  the rest in reaction order (4a). `label` is the `aria-label`, for example
  "👍 3, you reacted".
- `MessageView` gains `conversation_id: i64` and `reactions:
  Vec<ReactionChip>`.
- `grouped()` gains a `&HashMap<i64, Vec<ReactionRow>>` argument and fills
  `reactions` per message. `show` and `feed` load it with one
  `reactions::Model::for_messages` call over the ids from `recent`.
- A new message (HTTP `send`, websocket `publish`) starts with no reactions.

## 4. Routes (in `src/controllers/chat.rs`)

| Route | Does |
|---|---|
| `POST /chat/{id}/messages/{message_id}/reactions` (form `emoji`) | Toggle. Returns `chat/_reactions.html` for the viewer. Publishes the live event. |
| `GET /chat/{id}/messages/{message_id}/actions` | The action sheet body: quote, the six buttons with your state, who reacted. |

Access checks, in this order, for both:

1. `CurrentMember`: signed in, approved, not removed. Pending and removed
   people get 403 as today.
2. `conversations::Model::find_for_member(db, org, id, me)`: the
   conversation is in this organisation and the viewer is a member. Else 404.
3. `messages::Model::find_in_conversation(db, &conversation, message_id)`:
   the message is in that same conversation and organisation. Else 404. A
   message id from another conversation the viewer can see still gets 404.
4. Toggle only: emoji in `EMOJI`, else 422 with `HX-Reswap: none` and an
   error toast, like `send` does today.

Reactions send no notification. Nothing calls `notifications::send`.

## 5. Websocket event

`src/data/chat_hub.rs`: add

```rust
/// Someone added or removed a reaction. Carries the message's full set, so
/// each socket renders it for its own viewer without a query.
Reactions {
    conversation_id: i64,
    message_id: i64,
    rows: Arc<Vec<ReactionRow>>,
},
```

PR 2 adds `parent_id: Option<i64>` to it.

`src/controllers/chat_ws.rs`:

- `pub async fn publish_reactions(ctx, org_id, message)`: loads
  `for_messages(&[message.id])` and the names map, then publishes.
- In `run`, match `ChatEvent::Reactions` for this conversation and render
  `chat/_reactions.html` with `oob: true` and `chips(&rows,
  session.user_id)`. It swaps `#reacts-{message_id}`.
- The existing `Ok(event)` arm for read receipts must not catch this
  variant. Match `ChatEvent::Read` explicitly there.
- **Reconnect:** nothing new. `htmx:wsOpen` already reloads `#chat-feed`
  from `data-chat-feed-url`, and `feed` now renders reactions. A socket that
  lags is closed and reloads the same way.
- **Out-of-order events:** two toggles on the same message from different
  people could publish in the opposite order to their commits. Each event
  carries the whole set read after commit, so the last one wins. A stale
  final state is possible but rare with four people, and the next toggle or
  reload fixes it. Accepted, not handled.

## 6. Templates

- `assets/views/chat/_message.html`:
  - The avatar wrapper becomes `<span class="msg-av ...">`.
  - The bubble gets `tabindex="0"` and `data-actions-url=
    "/chat/{{ message.conversation_id }}/messages/{{ message.id }}/actions"`.
    Enter or Space on a focused bubble opens the sheet, same as a tap.
  - Inside `.msg-body`, after `.msg-meta`, add the desktop toolbar
    `chat/_tools.html`: `<div class="msg-tools" role="toolbar"
    aria-label="Message actions">` with the six reaction buttons
    (`data-react`, `aria-pressed`, title "React 👍" or "Remove your 👍").
  - After `.msg-body`: `<div class="msg-extra">{% include
    "chat/_reactions.html" %}</div>`.
- New `assets/views/chat/_reactions.html`: `<div class="reacts"
  id="reacts-{{ message.id }}" {% if oob %}hx-swap-oob="true"{% endif %}>`.
  One `<span class="react-wrap">` per chip, holding `<button type="button"
  class="react {% if mine %}mine{% endif %}" data-react data-emoji="..."
  data-react-url="..." aria-pressed="..." aria-label="...">` and a
  `<span class="react-pop" role="tooltip">` with the names (1c). The div is
  always rendered, empty when there are no reactions, so a live swap always
  has a target.
- New `assets/views/chat/_actions.html` (sheet body, 2a, 2b, 4a, 4b):
  `.quote` (avatar, name or "You", time, body), `.pick` with six 52px
  buttons, then "Reactions" and one `.who-row` per emoji. The reply row is
  added in PR 2. It loads into the existing `#chat-dialog-body`.

## 7. CSS (`frontend/styles/app.css`, chat block around line 410)

Copy from the mockup, using the app's tokens:

- **Grid:** `.msg { display: grid; grid-template-columns: 28px minmax(0,
  1fr); column-gap: 8px; align-items: end; }`. `.msg > .msg-av` in column 1,
  row 1. `.msg-body` in column 2, row 1, `justify-self: start`,
  `max-width: 86%`. `.msg-extra` in column 2, row 2. `.msg-own` has one
  column; its body and extra sit at the end. Row 2 spans the full width, so
  six chips fit at 390px (6c).
- Check the receipt rule at lines 438 to 439 still hides all but the last
  receipt in a run. It uses sibling selectors on `.msg-own`, which the grid
  does not change.
- `.reacts` (flex, wrap, gap 6px, padding 6px 0), `.reacts:empty { display:
  none; }`, `.react` (32px chip, `::after` inset -6px -3px for a 44px tap
  area), `.react.mine` (`--active-bg`, `--accent` border).
- `.react-pop`: hidden by default. In the existing `@layer utilities` `(hover:
  hover)` block: `.react-wrap:hover .react-pop, .react-wrap:focus-within
  .react-pop { display: flex; }`. Same look as `.receipt-pop`.
- `.msg-tools`: `display: none` everywhere. In the `(hover: hover)` block,
  shown on `.msg:hover` and `.msg:focus-within`. Others' messages anchor it
  at the bubble's right edge, your own at the left edge, so it never runs out
  of the pane. It sits above the bubble (`top: -34px`). The first message in
  a short feed can be clipped by the feed's scroll box: put the toolbar below
  the bubble for `.chat-feed > .msg:first-child`.
- Desktop hover tint: `.msg` gets `padding: 2px 8px; margin: 0 -8px;
  border-radius: 12px` at 900px and up, and `color-mix(in srgb, var(--chip)
  55%, transparent)` on hover.
- Sheet: `.quote`, `.pick`, `.pick button.mine`, `.who-row`. The sheet
  itself is the existing `.sheet`, which is already a centred dialog at 768px
  and up.

## 8. TypeScript (`frontend/app.ts`)

- **Open the sheet.** One `click` listener: find `.bubble[data-actions-url]`
  from the target. Do nothing when the click is inside `.react`,
  `.thread-link`, `.msg-receipt`, a link or a button. Do nothing when
  `getSelection()` is not collapsed and is inside that bubble (a tap or drag
  that ended a selection). Otherwise `htmx.ajax("GET", url, { target:
  "#chat-dialog-body", swap: "innerHTML" })`, then `showModal()` on
  `#chat-dialog`. On failure, `showToast` "Couldn't open that message. Check
  your connection and try again." A `keydown` listener does the same for
  Enter or Space on a focused bubble.
- **Toggle a reaction.** One `click` listener on `[data-react]` (chip, sheet
  button, toolbar button):
  1. Save `#reacts-{id}` outerHTML.
  2. Apply the change at once: flip `mine`, add or subtract 1, insert a new
     chip in `EMOJI` order, or remove a chip that reaches 0.
  3. `fetch(url, { method: "POST", body: new URLSearchParams({ emoji }),
     headers: { "HX-Request": "true" } })`. The browser sends `Origin`, so
     the origin check passes.
  4. On success, replace `#reacts-{id}` with the response (server truth).
  5. On any failure, put the saved HTML back and `showToast({ kind: "error",
     message: "Couldn't add your reaction. Check your connection and try
     again." })` (6f). Use "remove" in the wording when the tap removed one.
  6. A tap inside the sheet closes the sheet (2a).

## 9. PR 1 tests

New file `tests/requests/chat_reactions.rs`, added to
`tests/requests/mod.rs`. Use the helpers in `tests/requests/chat.rs`
(`acme_with_bob`, `general`, `group_of_three`).

- **Toggling:** alice posts 👍 on bob's message: one row, response has "👍"
  with `react mine` and count 1. Again: zero rows, `#reacts-{id}` empty. 👍
  and ✅ together: two rows. bob's 👍 plus alice's 👍: count 2, "mine" only in
  alice's render.
- **Whitelist:** "😀", "❤" without U+FE0F, "👍👍", "", "<b>x</b>" and a
  missing field each get 422 and leave zero rows. All six in `EMOJI` are
  accepted.
- **Double insert:** call `reactions::Model::toggle` add path twice through
  the insert branch (insert a row directly first): no error, still one row.
- **Access (every new route, both verbs):** another organisation's member
  (gina in Globex) gets 404. carol pending gets 403. bob, not in a private
  group, gets 404 on a group message. alice, in both General and the group,
  posting the group's message id under General's id gets 404. A removed
  member gets 403 (extend `removing_someone_locks_them_out_of_everything` in
  `tests/requests/remove_member.rs`). Add both routes to
  `org_a_cannot_reach_org_b` in `tests/requests/tenancy.rs` with Globex's
  conversation and message ids.
- **Live:** subscribe to `ChatHub`, toggle, assert one `ChatEvent::Reactions`
  with the message id and one row.
- **No notifications:** after reacting, `notifications::Entity` count is 0.
- **Feed render:** `GET /chat/{id}` shows chips in the fixed order for three
  emoji added out of order. `GET /chat/{id}/feed` shows them too (the
  reconnect path).
- **Who reacted:** `GET .../actions` lists "You" first, then others in
  reaction order, and marks your emoji `aria-pressed="true"`.
- **Emoji in a message body:** post "Done ✅🎉" and a family emoji with ZWJ
  ("👨‍👩‍👧"). Both save byte for byte (read from the database) and render
  back unescaped. "✅" × 1000 is accepted; "✅" × 1001 gets 422 (the
  validator counts characters).
- **Render test** in `tests/views/render.rs`: `chat/_reactions.html` with
  `oob: true` has `id="reacts-42" hx-swap-oob="true"`, and an empty set
  still renders the div.
- Model test `tests/models/reactions.rs`: `for_messages` groups by message
  and stays inside the tenant.

## 10. PR 1 screenshots

Folder `chat-reactions/` on `pr-screenshots`. See "Screenshots" below.

---

# PR 2: threads

## 11. Migration: `messages.parent_id`

`migration/src/m20261013_HHMMSS_add_parent_id_to_messages.rs`. Loco's
`add_reference` makes a non-null column named after the target table, so use
SQL, as the messages migration does:

```sql
ALTER TABLE messages
  ADD COLUMN parent_id bigint NULL
  REFERENCES messages (id) ON DELETE CASCADE ON UPDATE CASCADE;
CREATE INDEX "idx-messages-parent-created"
  ON messages (parent_id, created_at) WHERE parent_id IS NOT NULL;
```

- **What it does:** a reply points at its parent. Existing messages get NULL
  and stay top-level. The partial index serves "replies of these parents"
  and stays small because main-feed rows are not in it.
- **Cascade:** deleting a parent (today only through `user:delete`) deletes
  its replies, and their reactions follow through the reactions cascade.
- **Not enforced by the database:** that the parent is top-level and in the
  same conversation. A plain CHECK cannot look at another row. The model
  enforces both, and tests cover them.
- **Rollback:** `down` runs `ALTER TABLE messages DROP COLUMN parent_id`,
  which drops the index and the key. It can be rolled back. After use,
  every reply becomes an ordinary message at its time in the main feed. The
  text survives, but threading is lost and those replies will show as unread.
- **Rolling back the code only** (redeploying an older commit with the
  column still there) has the same effect: old code ignores `parent_id`, so
  replies show in the feed and the list preview. Say this in the PR.
- `cargo loco db entities`, then check the `messages` entity only gains
  `parent_id: Option<i64>` and a self relation.

## 12. Model changes: keep replies out of the main feed

Every query below gets `.filter(Column::ParentId.is_null())`:

| Function | File | What leaks without it |
|---|---|---|
| `messages::Model::recent` | `src/models/messages.rs` | replies in the feed and in `/feed` reloads |
| `messages::Model::read_in_span` | `src/models/messages.rs` | read receipts on replies |
| `messages::Model::latest_per_conversation` | `src/models/messages.rs` | the chat list preview and its time |
| `conversation_members::Model::unread_counts` | `src/models/conversation_members.rs` | the unread badge per conversation and the chat tab total |

Not in the brief's list but found while reading: `unread_counts`. It runs
its own message count per membership.

Left as is, on purpose: `guide::steps` (`src/models/guide.rs`) counts any
message by you in General for "Say hello in #general". A reply there still
counts as saying hello.

New functions in `src/models/messages.rs`:

- `find_thread_parent(db, conversation, id) -> ModelResult<Self>`:
  `find_in_conversation` plus `parent_id IS NULL`. Else `EntityNotFound`.
- `create_reply(db, conversation, parent: &Self, user_id, params) ->
  ModelResult<Self>`. Refuses with `EntityNotFound` unless
  `parent.conversation_id == conversation.id`, `parent.organisation_id ==
  conversation.organisation_id` and `parent.parent_id.is_none()`. Same
  trim and 1 to 1,000 character check as `create`. Sets `parent_id`.
- `replies(db, parent) -> ModelResult<Vec<Self>>`: the latest
  `HISTORY_LIMIT` replies, oldest first, like `recent`.
- `thread_stats(db, org_id, parent_ids: &[i64]) -> ModelResult<HashMap<i64,
  Vec<ReplierStat>>>`. One grouped query, no N+1:

  ```sql
  SELECT parent_id, user_id, count(*) AS replies,
         min(created_at) AS first_at, max(created_at) AS last_at
  FROM messages
  WHERE organisation_id = $1 AND parent_id = ANY($2)
  GROUP BY parent_id, user_id
  ```

  Built with SeaORM `select_only` and a `#[derive(FromQueryResult)] struct
  ReplierStat`. From it: total count is the sum, the last reply time is the
  max `last_at`, and the faces are users ordered by `first_at` (3a shows
  R, K, S in order of their first reply). The notification code reuses it to
  find earlier repliers.

So the main feed costs three queries for messages, reactions and thread
stats, whatever the number of messages.

## 13. Views

In `src/controllers/chat.rs`:

- `ThreadLine { url, count, label, last, faces: Vec<Person>, more: usize }`.
  `label` is "1 reply" or "N replies". `faces` is at most 3, `more` counts
  the rest (6c shows "+1"). `last` uses a new `reply_when(at, tz)`: "14:05"
  today, "Yesterday", "Mon" this week, then "12 Oct" (6c). It wraps the
  existing `when()` with a Yesterday case. The chat list keeps `when()`
  unchanged (6g says the list does not change).
- `MessageView` gains `parent_id: Option<i64>`, `thread:
  Option<ThreadLine>` and `can_reply: bool`. `can_reply` is true for
  top-level messages in the main feed and false on the thread page.
- `grouped()` takes the stats map and sets `thread` on parents with replies.
- A reply's view has `read_receipts: false` everywhere.

## 14. Routes

| Route | Does |
|---|---|
| `GET /chat/{id}/threads/{message_id}` | The thread page. `?reply=1` focuses the composer. |
| `GET /chat/{id}/threads/{message_id}/feed` | Parent, divider and replies on their own, for reloading after a reconnect. |
| `POST /chat/{id}/threads/{message_id}/replies` (form `body`) | Plain HTTP reply when the socket is not available. Same shape as `send`. |
| `GET /chat/{id}/ws?thread={message_id}` | The thread's live socket (section 15). |

Access checks for all four:

1. `CurrentMember` (pending and removed: 403).
2. `find_for_member`: same organisation and a member of the conversation.
3. `find_thread_parent`: the message is in that conversation and
   organisation, and it is top-level. A reply's id, or a parent from another
   conversation, is 404.
4. Reply only: body 1 to 1,000 characters after trimming (422 and a toast).

The thread page turns any `EntityNotFound` from steps 2 or 3 into one
response: `chat/thread_missing.html` with status 404 (6e). The text is the
same whether the thread is gone or the viewer lost access, so it never
confirms that a private conversation exists. The other three routes return
the normal 404.

The thread page does **not** mark the conversation read, and neither does
its feed. Replies are not part of the unread count, and opening a thread
does not show the main feed.

The reply POST:

1. Checks as above.
2. `messages::Model::create_reply`.
3. `chat_ws::publish_reply(&ctx, org_id, &parent, &reply)`.
4. `notifications::reply_sent(&ctx, org_id, &conversation, &parent,
   &reply)`.
5. Non-HTMX: redirect to the thread. HTMX: render the reply. Error: 422,
   `HX-Reswap: none`, toast.

## 15. Websocket

`src/data/chat_hub.rs`:

```rust
/// A new thread reply. Main-feed sockets update the parent's reply line;
/// sockets on that thread append the reply.
Reply {
    conversation_id: i64,
    parent_id: i64,
    author_id: i64,
    message: Box<MessageView>,
    line: Box<ThreadLine>,
},
```

and `Reactions` gains `parent_id: Option<i64>` (the reacted message's
parent).

`src/controllers/chat_ws.rs`:

- `connect` takes `Query<WsQuery { thread: Option<i64> }>`. When set, it
  runs `find_thread_parent` before the upgrade (404 otherwise) and stores
  `thread_id: Option<i64>` on `Session`.
- `still_allowed` is unchanged. Membership is the check that matters, and a
  deleted parent is caught on the next send.
- `receive`: with `thread_id` set, it reloads the conversation and the
  parent (`find_thread_parent`) on every send, then `create_reply`,
  `publish_reply`, `reply_sent`. The parent id comes from the session,
  never from the browser's JSON.
- `publish_reply` loads the names map, the org zone and `thread_stats` for
  the one parent, then publishes `ChatEvent::Reply`.

What each socket does with each event:

| Event | Main-feed socket (`thread_id` None) | Thread socket (`thread_id` = P) |
|---|---|---|
| `Message` | append, mark read (today) | ignore |
| `Reply` with parent P | swap `#thread-{P}` with `_thread_link.html` (oob). No append, no read mark. | append with `_message_oob.html` (`own` set, no receipts), swap `#thread-count` |
| `Reactions` | only when `parent_id` is None | when `message_id == P` or `parent_id == Some(P)` |
| `Read` | receipts (today) | ignore |
| `SignedOut` | close (today) | close |

Put this choice in a small pure function, `fn wants(event, thread_id) ->
bool`, and unit-test it next to `receipts_for`.

**Reconnect:** the thread page sets `data-chat-feed-url` to the thread feed
route, so the existing `htmx:wsOpen` handler reloads parent, divider and
replies. The main page reloads its feed, which now carries fresh reply
lines and reactions. A lagging socket is closed and goes through the same
path.

## 16. Notifications

`src/models/notifications.rs`: add `kind::REPLY = "reply"`. The stored kind
is "reply", so the existing `notice-{{ n.kind }}` class gives
`notice-reply`, which is the mockup's class.

`src/controllers/notifications.rs`: new `pub async fn reply_sent(ctx,
org_id, conversation, parent, reply) -> Result<()>`:

1. `team`, and `members = conversation.member_ids()`. Every recipient below
   must be in `members` and must not be the reply's author.
2. `place`: `#name` for a channel, the name for a group, nothing for a DM.
3. **Mentions:** `mentioned_ids(reply.body, team)` filtered as in step 1.
   Notice kind `MENTION`, body "mentioned you in a thread in {place}" or
   "mentioned you in a thread" in a DM. `told` = who got it.
4. **@everyone:** if `mentions_everyone`, every other member not in `told`
   gets "mentioned everyone in a thread in {place}". Add them to `told`.
5. **Parent's author:** if not the reply's author and not in `told`:
   kind `REPLY`, "replied to your message in {place}" or "replied to your
   message". Add to `told`.
6. **Earlier repliers:** user ids from `thread_stats(&[parent.id])`, minus
   `told` and the author: kind `REPLY`, "replied in a thread in {place}" or
   "replied in a thread".
7. Every notice: excerpt is `excerpt(&reply.body)`, link is
   `/chat/{conversation_id}/threads/{parent_id}`.

Rules this gives: each person is told once, a mention wins, the parent's
author wording wins over the replier wording, and nobody outside the
conversation is told.

Self-mentions: today a chat message where you @mention yourself gives you a
"mentioned yourself" reminder (`notify` lets mention notices reach the
actor). Raj's rule says the reply's author is never told, so step 1 drops
the author from mentions too. Thread mentions differ from channel mentions
here. Flagged for review.

Thread mentions are kind `MENTION`, so the existing unseen-mention email
(`due_mention_emails`, `src/mailers/mentions.rs`) sends them like any other
mention, with the thread link. "Reply" notices are not emailed.

`assets/views/notifications/index.html`: add `{% elif n.kind == "reply"
%}{{<icon name="reply" class="h-3 w-3"/>}}`. CSS: `.notice-reply { color:
var(--done); }`. The `MENTIONED_YOU` rewrite still works, since thread
mention bodies start with "mentioned you".

## 17. Templates

- `assets/views/components/icons.html`: add `reply` and
  `message-square-reply` (Lucide paths are in the mockup's sprite).
- `_message.html`: inside `.msg-extra`, after the reactions,
  `{% include "chat/_thread_link.html" %}`. Toolbar: add a separator and
  "Reply in thread" when `message.can_reply`, linking to the thread with
  `?reply=1`.
- New `chat/_thread_link.html`: `<span id="thread-{{ message.id }}"
  class="thread-slot" {% if oob %}hx-swap-oob="true"{% endif %}>` holding,
  when there is a thread, `<a class="thread-link" href="{{ line.url }}">`
  with the face stack, `<b>{{ line.label }}</b>`, the time and a chevron.
  It is a link, not a button, since the thread has its own URL. The slot is
  always rendered so the first reply can swap it in. `.thread-slot:empty {
  display: none; }`.
- `_actions.html`: when the message is top-level and not on a thread page,
  add the "Reply in thread" row, with "N replies" when a thread exists (4a).
- New `chat/thread.html` (3a, 3c), extending `layouts/app.html` like
  `show.html`:
  - The list pane (`_list.html` with `conversation` set, so the row stays
    highlighted on desktop).
  - `section.chat-screen` with `hx-ext="ws"`, `ws-connect="/chat/{id}/ws?
    thread={pid}"` and `data-chat-feed-url` for the thread feed.
  - Header: Back link `href="/chat/{id}#message-{pid}"`, `data-thread-back`,
    `aria-label="Back to #general"` (shown at every width, unlike the
    conversation's). Title "Thread". Subtitle "# general", the group name,
    or "With Shusma" in a DM.
  - `#chat-feed.chat-feed.top[data-autoscroll]` with `chat/_thread_feed.html`.
  - Composer: same as `show.html`, `action` set to the replies route,
    placeholder "Reply in thread", same `data-mentions` as the conversation
    (with `everyone` outside DMs), `autofocus` when `?reply=1`.
  - The `#chat-dialog` sheet. `{% block tabbar %}{% endblock %}`.
- New `chat/_thread_feed.html`: the parent (no reply line, no "Reply in
  thread"), `<div id="thread-count" class="thread-divider">` ("3 replies",
  hidden at 0), the replies, and `.thread-empty` (6b): "No replies yet" and
  "Your reply stays with this message." plus "{name} will get a notice."
  when the parent's author is someone else still on the team.
- New `chat/_thread_count.html` for the live divider swap.
- New `chat/thread_missing.html` (6e): header with Back to chats, the
  `messages-square` icon, "This thread isn't available", "The message it
  started from may have been removed, or you may no longer be in this
  conversation.", and "Go to Chat". No composer.

## 18. CSS and TypeScript

CSS (copy from the mockup):

- `.thread-link` (44px tall, `--link` bold count, muted time), `.thread-link
  .stack` with `.stack-more`. Reuse `.receipt-stack` styles where they match.
- `.thread-divider`, `.thread-empty`. `.thread-empty:has(~ .msg) { display:
  none; }` so the first live reply hides the empty state.
- `.chat-feed.top > :first-child { margin-top: 0; }` for short threads.
- `.center-state` for the 404 page.

TypeScript:

- The sheet's "Reply in thread" row is a plain link.
- **Escape goes back** on desktop: on `keydown` Escape, when no
  `dialog[open]`, the mention menu is closed, and `matchMedia("(min-width:
  900px)")` matches, follow `[data-thread-back]`. The mention menu already
  stops Escape in the capture phase.
- **Back to the same spot:** on load, if `location.hash` is `#message-N`
  and that message is in a `[data-autoscroll]` feed, scroll it into view
  (centre) instead of to the end. Change the existing
  `document.querySelectorAll("[data-autoscroll]").forEach(scrollToEnd)` to
  do this.
- **Long threads open at the newest reply:** `data-autoscroll` already
  scrolls to the end.
- Unverified: iOS Safari may not raise the keyboard for `autofocus` after a
  navigation. If it does not, the composer is still focused on the first
  tap. Check on a real phone.

## 19. PR 2 tests

New file `tests/requests/chat_threads.rs`. Notification cases go in
`tests/requests/notifications.rs`.

**Feed exclusion**

- alice posts a parent; bob replies over HTTP. `GET /chat/{id}` (alice) does
  not contain the reply text and does contain "1 reply". `GET
  /chat/{id}/feed` the same.
- The chat list preview for General is the parent, not the reply, and the
  time is the parent's.
- alice reads General, bob replies: `GET /chat/unread` is empty for alice,
  and `unread_counts` has no entry for General.
- `read_in_span` (model test in `tests/models/messages.rs`): a span covering
  a reply returns only top-level messages. bob's reply carries no receipt
  slot.
- `latest_per_conversation` ignores a newer reply (model test).

**Threads**

- The thread page shows the parent, "2 replies", replies oldest first, and
  the composer posting to the replies route.
- A reply's id as `{message_id}` is 404 on the page, the feed, the reply
  POST and the socket. No nested reply is saved.
- A missing id and a thread in a private group bob is not in both return 404
  with "This thread isn't available", and the two bodies match.
- HTTP reply publishes one `ChatEvent::Reply` with the parent id and a line
  of "1 reply", and no `ChatEvent::Message`.
- Deleting the parent's author (`users::Model::delete`, as `user:delete`
  does) removes the parent, its replies and their reactions.
- Emoji in a reply body, as in PR 1.
- `wants()` unit tests in `chat_ws.rs` for every row of the table in
  section 15.

**Access (every new route)**

Same five cases as PR 1: other organisation 404, pending 403, not a member of
the group 404, mismatched conversation id 404, removed 403. Add the thread
page, thread feed, reply POST and `ws?thread=` to `org_a_cannot_reach_org_b`
in `tests/requests/tenancy.rs`, and the socket cases to
`websocket_checks_run_before_the_upgrade`.

**Notification recipients**

- Shusma posts; Kreepa replies: Shusma gets one "replied to your message in
  #general"; nobody else gets anything.
- Samir replies: Shusma gets "replied to your message", Kreepa gets
  "replied in a thread", Samir gets nothing. One notice each.
- Shusma replies in her own thread: Kreepa and Samir get "replied in a
  thread"; Shusma gets nothing.
- Samir replies "@shusma @raj": Shusma gets only the mention ("mentioned you
  in a thread in #general"), Raj gets the mention, Kreepa gets "replied in a
  thread".
- @everyone in a reply: each member once; the parent's author gets the
  everyone notice, not a second reply notice.
- The reply's author mentioning themselves gets nothing.
- Group: mentioning someone outside the group tells nobody. A parent author
  who has been removed is not told.
- DM: wording has no place ("replied to your message").
- `POST /notifications/{id}/open` on a reply notice redirects to
  `/chat/{cid}/threads/{pid}`.
- Reactions still send nothing.

## 20. PR 2 screenshots

Folder `chat-threads/` on `pr-screenshots`. See below.

---

# Screenshots (both PRs)

Take them from the local app with Playwright (headless Chromium), with data
from the mockup, not test leftovers. Set it up by hand on a fresh local
database: sign up "Himalayan Ritual" as Raj, join Shusma, Kreepa and Samir,
approve them, then post the mockup's messages, replies and reactions as each
person. Do not commit a seed file.

Each at 1440px and 390px, light and dark, unless noted:

| Shot | PR | Mockup |
|---|---|---|
| General with reaction chips (all six on one message at 390px) | 1 | 1a, 6c |
| Desktop hover toolbar and who-reacted popover (1440 only) | 1 | 1c |
| Action sheet with who reacted (phone sheet, desktop centred dialog) | 1 | 2a, 4a, 4b |
| Failed reaction toast (390 light only) | 1 | 6f |
| General with reply lines | 2 | 1a, 1b |
| Thread open (phone full screen, desktop replacing the pane) | 2 | 3a, 3b, 3c |
| Empty thread in a DM (390 only) | 2 | 6b |
| Bell with reply and thread-mention notices (390 only) | 2 | 5a, 5b |
| "This thread isn't available" (390 only) | 2 | 6e |

PNG files in `pr-screenshots/chat-reactions/` and
`pr-screenshots/chat-threads/`, linked as
`https://raw.githubusercontent.com/rajgurung/collaboration_tool/pr-screenshots/<folder>/<file>.png`.
Check each URL returns 200.

Also, after each deploy: type "Done ✅🎉" on a real iPhone and an Android
phone, and check it saves and shows on desktop. The brief marks this as
unverified.

# Checks (both PRs)

`bin/build-assets`, `cargo fmt --all`, `cargo clippy --all-targets -- -D
warnings`, `cargo test`. Each PR body says what its migration does and that
it can be rolled back, with the caveat in section 11 for PR 2.

# Open points for review

1. **Self-mentions in a reply** give no notice (section 16), unlike in the
   channel. This follows Raj's rule literally.
2. **Day in message times.** Frames 6b, 6c and 6f show "Mon 16:32" and
   "2 Sep" on message bubbles. Today every bubble shows only "16:32". The
   brief does not ask for this. Recommend: show the day only on the parent
   at the top of a thread, where it is most needed, and leave the main feed
   as it is.
3. **Sender names in DMs.** Frame 6b's note says DMs show no sender name
   "as today". Today's `_message.html` does show the name on the first
   message of a run in DMs. Keep today's behaviour.
4. **Thread length.** Like the main feed, a thread shows its latest 200
   replies. The divider shows the true total.
5. **Composer length.** The textarea's `maxlength="1000"` counts UTF-16
   units, so a message of only emoji stops at about 500. The server counts
   characters. Harmless; left as is (from the brief).
6. **`latest_per_conversation`** loads every message of every conversation
   to pick the newest. Not changed here beyond the filter. Worth a follow-up
   with `DISTINCT ON`.

# Riskiest parts

1. **Feed exclusion.** Four queries must skip replies. Missing one leaks
   replies into the feed, the preview, the badge or receipts. Each has its
   own test.
2. **Tenancy on message ids.** Every new route resolves the conversation for
   the member first, then the message inside that conversation. The
   mismatched-conversation test catches a route that checks only one.
3. **Websocket routing.** Main and thread sockets must each take only their
   events. `wants()` is pure and unit-tested. Reaction events can arrive out
   of order (accepted, see section 5).
4. **The `.msg` grid.** It touches every message. Check receipts, grouping,
   own messages and the hover toolbar at 390px and 1440px before the
   screenshots.
5. **Tap versus text selection on iPhone.** The selection check is
   unverified on a real phone.
