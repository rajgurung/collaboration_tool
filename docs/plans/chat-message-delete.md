# Delete a chat message

Status: Approved with changes (staff and security), built · One PR · Owner: Raj · 25 October 2026

**Review changes, applied in the build:** a failed delete that already took
the photo still publishes `PhotoRemoved`; the client regroups only the two
neighbours of a removed message; the mention-email claim and
`due_mention_emails` skip chat notices with no `message_id`; the migration's
`UPDATE` only touches unsent chat mentions; the deadlock note below is
corrected; step 6's guard is dropped (`replies_never_carry_photos` pins it);
the handler is `destroy`, and `_thread_gone.html` links "Back to chat".

Brief: `docs/briefs/chat-message-delete.md`. Mockup:
`docs/designs/chat-message-delete.html` (sections §1 to §5 below are the
mockup's). Visuals and copy are the mockup's, except where the change below
overrides them. This plan covers what the database, the server, the
templates and `frontend/app.ts` must do. It builds on photo delete (PR #59,
`docs/plans/chat-photo-delete.md`).

**Changed 25 October 2026: hard delete.** Raj dropped the soft delete. The
messages row is deleted. There is no `deleted_at`, no "Message deleted"
placeholder and no wiped-row CHECK. Mockup §3 (the placeholder) no longer
applies; its §1, §2 and §4 do, with the thread rule below.

Decisions taken:

1. Hard delete. The row goes, with its reactions and the bell notices about
   it.
2. Unsent mention emails for it are never sent.
3. The sheet's "Delete photo" row becomes one "Delete message" row.
   "Delete photo" stays only in the full-screen viewer.
4. Threads: the sender may delete their own message only if it has no
   replies. Owners and admins in the conversation may delete any message,
   and deleting a parent deletes its replies.
5. No notice to the sender. Permanent. One PR.

The review items accepted on the first draft that still apply are folded in
and marked (S1) to (S8) for security and (T) for staff.

## Summary

The sender, or an owner or admin who is in the conversation, can delete a
message for everyone, for good. It disappears from every open feed and
thread. Its photo, reactions and bell notices go with it. A thread parent
can only be deleted by an owner or admin once it has replies, and its
replies go too.

- **One migration:** `notifications.message_id`, a nullable foreign key
  `ON DELETE CASCADE`, so the notices about a message are deleted with it.
  It also stops unsent mention emails made before the column existed.
- **One new route:** `POST /o/{org}/chat/{id}/messages/{message_id}/delete`.
  Same checks as `delete_photo`, in the same order, plus the thread rule.
- **One new model method:** `messages::Model::delete_message`. The photo
  first, through the existing `delete_photo` (bytes, then the row). Then
  one transaction: lock the row, recount replies, delete the row. The
  database cascades replies, reactions and notices.
- **Race-safe locks:** reactions, replies and chat notices are saved under
  a `FOR SHARE` lock on their message, in a real transaction. The delete
  holds `FOR UPDATE`. Whichever comes second sees the truth.
- **One new socket event:** `ChatEvent::MessageDeleted`. Open feeds remove
  the bubble and regroup their neighbours; a thread page whose parent went
  says "This thread was deleted".
- **The sheet** gets "Delete message" in place of "Delete photo" (§1), or,
  for the sender of a message with replies, a line saying to ask an owner
  or admin. One confirm, `#message-delete` (§2).

No new crate. A repeat delete gets 404 (the row is gone); the client treats
a 404 after its own earlier attempt as success.

## What the code does today

Read before writing this plan:

- `src/models/messages.rs`. `may_delete_photo` (line 55) is
  `can_manage || sender_id == user_id`. `delete_photo` (431) returns
  `(Self, bool)`. It returns `EntityNotFound` with no photo, and also for a
  #general message whose Documents file is not a photo type (457). It is
  `(self, false)` when already removed. Group and DM photos lose their bytes,
  then their four columns. #general photos call `documents::Model::delete_file`.
  `insert` (480), used by `create` and `create_reply`, sets no photo field,
  and `create_photo` (255) always makes a top-level message. **So a reply can
  never carry a photo**, confirmed. A cascade that deletes replies can never
  leave photo bytes behind.
- Foreign keys into `messages`:
  - `reactions.message_id`: made by Loco's `create_table` with a required
    reference (`m20261013_140537_reactions.rs`), which is `ON DELETE CASCADE`
    (`loco-rs-1.2.0/src/schema.rs:698`).
  - `messages.parent_id`: `ON DELETE CASCADE` (`m20261013_145508_add_parent_id_to_messages.rs`).
    A reply's reactions cascade through its own row.
  - `messages.document_id` points out to `documents` with `SET NULL`;
    nothing in `documents` points back.
  - Nothing else references `messages`.
- `src/models/_entities/notifications.rs`: `kind`, `body`
  (`"headline\nquote"`), `link`, `read_at`, `actor_id`, `emailed_at`. No
  link to a message. The latest migration is
  `m20261025_120100_add_private_photos_to_messages`.
- `src/controllers/notifications.rs`. `message_sent` (379) and `reply_sent`
  (448) build `Notice { kind, body, link }` quoting `excerpt(&message.body)`
  or the reply's body. `send` inserts per person on `ctx.db` and publishes
  on `NotifyHub`. Twelve `Notice {` literals in two files.
- `src/mailers/mentions.rs` `send_due` (73), run every minute by
  `src/initializers/mention_emails.rs`. It calls `due_mention_emails`
  (mention, unread, not emailed, older than 15 minutes), then
  `mark_emailed(ids)`, then builds the emails from the rows it loaded first.
- `src/models/reactions.rs` `set` (39): generic `C: ConnectionTrait`,
  insert with `on_conflict_do_nothing_on`, no lock.
- `src/models/conversation_members.rs` `unread_counts` (173): counts
  top-level rows from others since `last_read_at`.
- `src/controllers/chat.rs`. `MessageView` (209), `allow_photo_delete`
  (299), `grouped` and `mark_runs` (387, 421: `start` when the author
  differs from the previous one, `show_time` when the next one differs),
  `last_line` (502), `list_items` (521, also Home at `dashboard.rs:190`).
  The `delete_photo` handler (872): `require_own_membership`,
  `find_for_member`, `find_in_conversation`, `has_photo`,
  `may_delete_photo`, the Documents file's own `can_delete`, the model, an
  info log with ids only, then `publish_photo_removed` with a logged error.
  `actions` (1037), `thread_feed_data` (1070), `thread` (1133; a missing
  parent renders `thread_missing.html`, 404), HTTP `reply` (1197).
- `src/controllers/chat_ws.rs`. `Session.can_manage`. `run` keeps
  `sent_replies` so a late `Reply` event never lowers a count (`grows`,
  533). `wants` (549). `receive` (279) creates replies on a thread socket.
- `src/data/chat_hub.rs`: `ChatEvent::PhotoRemoved` (54).
- Templates. `_message.html`: the `.msg` div with run classes, the avatar
  (`visible`/`invisible`), the `.msg-name` (only on `start` and not own),
  the bubble, `.msg-foot`, `_tools.html`. `_thread_link.html` (`#thread-N`,
  inside the bubble) and `_thread_count.html` (`#thread-count`) are
  top-level OOB targets. `_actions.html:20` is the "Delete photo" row.
  `thread.html:18-28`: `#chat-feed` then the composer form (no id).
- `frontend/app.ts`. `openActions` (290). `fetchRefusalText` (602). The
  photo-delete block (1993-2141): `photoDeleteParts`, `photoDeleteBusy`,
  `openPhotoDelete`, `deletePhoto`, the click router, `syncViewer` (2127,
  which returns early when `#message-N` is missing), and the `pageshow`
  feed reload (2144).
- Logging: `DB_LOGGING` and `LOG_LEVEL` are unset in Railway (confirmed by
  Raj), so SQL statements are not logged and the level is `info` (S6). No
  message text reaches the logs through the queries below.

## Migration

`migration/src/m20261025_140000_add_message_id_to_notifications.rs`, added
to `lib.rs` above the inject comment:

```sql
SET LOCAL lock_timeout = '5s';
ALTER TABLE notifications
  ADD COLUMN message_id bigint NULL
  REFERENCES messages (id) ON DELETE CASCADE ON UPDATE CASCADE;
CREATE INDEX "idx-notifications-message"
  ON notifications (message_id) WHERE message_id IS NOT NULL;
-- Chat mentions saved before this column existed can't be found by a
-- delete, so none of them is emailed after deploy. Task-note mentions link
-- to /tasks?open=... and are left alone.
UPDATE notifications SET emailed_at = now()
 WHERE kind = 'mention' AND emailed_at IS NULL AND link LIKE '/chat/%';
```

**`message_id`, `ON DELETE CASCADE`.** The notice exists to point at a
message and quote it. Once the message is gone, both the link and the quote
are wrong, and the quote may be the very text someone deleted. Cascade
deletes the notice in the same statement as the message, so there is no
code path that can forget it. It also covers a parent's replies: their
notices cascade through the reply rows.

- Only chat mention and reply notices set it. Task, project, closed,
  reopened and connector notices keep NULL and are untouched.
- Side effect, on purpose: deleting a user account (`src/tasks/user_delete.rs`)
  already cascades that user's messages. Now it also removes other people's
  notices quoting them. Before, those quotes stayed.
- The partial index serves the cascade lookup on every message delete,
  including an account delete over many messages. It stays small because
  most notices have no message.

**No messages change.** No `deleted_at`, no CHECK. Rows simply go.

**The deploy window (S4).** Notices made before this ships have
`message_id` NULL. A delete cannot find them, so their quote stays in the
bell (the brief accepts this). Their emails are stopped instead: the
`UPDATE` marks every unsent mention as emailed. The cost is that mentions
less than 15 minutes old at deploy are never emailed. The people tagged
still see them in the bell. The PR says so. Railway runs migrations as the
new app boots, so the old instance can still make notices in the seconds
before it stops. Those have NULL too. The window is closed in code:
`due_mention_emails` and the claim skip rows with `message_id IS NULL AND
link LIKE '/chat/%'`, so those are never emailed either.

**Reversible.** `down`:

```sql
DROP INDEX "idx-notifications-message";
ALTER TABLE notifications DROP COLUMN message_id;
```

The emails skipped at deploy stay skipped, and deleted messages and their
notices never come back.

**Entities.** After migrating, `cargo loco db entities`. Check that the diff
only adds `message_id` and the relation between `notifications` and
`messages`.

## What changes, file by file

### 1. The model: `src/models/messages.rs`

**Who may delete.** Rename `may_delete_photo` to `may_delete` ("a message,
or its photo"), same body, and update its callers. Add the thread rule:

```rust
/// Shown, and returned with a 403, when the sender tries to delete a
/// message that has replies.
pub const HAS_REPLIES: &str = "This has replies. Ask an owner or admin to delete it.";

/// Whether this person may delete a message with `replies` replies: owners
/// and admins any, the sender only one with none. The caller must already
/// know the person is in the conversation.
#[must_use]
pub fn may_delete_message(sender_id: i64, user_id: i64, can_manage: bool, replies: u64) -> bool {
    can_manage || (sender_id == user_id && replies == 0)
}
```

**`lock_live`,** for everything that saves against a message:

```rust
/// This message, locked with `FOR SHARE` until the caller's transaction
/// ends, or `None` once it is deleted. A delete holds `FOR UPDATE`, so
/// whichever comes second sees the other's result.
pub async fn lock_live<C: ConnectionTrait>(db: &C, org_id: i64, id: i64) -> ModelResult<Option<Self>>
```

`Entity::find().in_tenant(org_id).filter(Id.eq(id)).lock_shared().one(db)`.
`lock_shared` is in SeaORM 2.0.4's `QuerySelect`.

**`create_reply`** takes `&DatabaseConnection` (callers pass `&ctx.db`) and
runs in a transaction:

1. `begin`.
2. `lock_live` on the parent. If it is `None`, or the parent is a reply or
   in another conversation, `EntityNotFound`.
3. `insert`, then commit.

A reply landing during a delete therefore either commits first, and the
sender's delete then counts it and refuses, or waits and then finds the
parent gone (404).

**`reply_count`:** `Entity::find().in_tenant(org).filter(ParentId.eq(id)).count(db)`.

**`delete_message`:**

```rust
/// Deletes this message for everyone, with its photo, reactions and bell
/// notices; a thread parent takes its replies. Returns how many replies
/// went. `by_manager` is an owner or admin; anyone else may delete only a
/// message with no replies, checked again under the lock.
///
/// The photo goes first, as `delete_photo` does it (bytes, then the row),
/// because storage can't join a transaction. The rest is one transaction.
/// The caller checks membership and [`may_delete`].
///
/// # Errors
/// `Message(HAS_REPLIES)` when the sender's message has replies;
/// `EntityNotFound` when it is already gone; storage or database errors.
pub async fn delete_message(self, ctx: &AppContext, by_manager: bool) -> ModelResult<u64>
```

In order:

1. **Early refusal.** If `!by_manager` and `reply_count > 0`, return
   `Message(HAS_REPLIES)` before touching the photo.
2. **Photo.** If `photo_file()` is `Some`, call `self.clone().delete_photo(ctx)`:
   bytes first, then the columns (group, DM) or the Documents file
   (#general).
   - On a storage or database error, return it. Nothing else has changed.
   - On `EntityNotFound`, carry on. That comes from the non-photo Documents
     branch: the #general file is no longer a photo type, so this chat must
     not delete it. The code comment says this can't happen today: a file's
     `content_type` is set only at upload, and chat uploads are sniffed to
     photo types. The file stays in Documents; the row's `document_id` goes
     with the row.
3. `let txn = ctx.db.begin().await?` and `SET LOCAL lock_timeout = '5s'`, so
   a stuck lock fails the delete rather than hanging it.
4. Re-read the row with `FOR UPDATE` (`lock_exclusive`), by id, organisation
   and conversation. If it is `None`, someone else deleted it: return
   `EntityNotFound`.
5. **Recount** `reply_count(&txn)` under the lock. If `!by_manager` and it is
   more than 0, roll back and return `Message(HAS_REPLIES)`. A reply that
   committed between step 1 and here is seen. One still in flight holds
   `FOR SHARE` on this row, so step 4 waited for it.
6. (Dropped in review: a reply can never carry a photo, and
   `replies_never_carry_photos` pins that.)
7. Collect `user_id`s of notices for this message and its replies, for the
   badge refresh after commit.
8. `Entity::delete_by_id(id)` with the organisation filter. Postgres
   cascades replies, the reactions on both, and the notices on both.
9. Commit. Return the reply count.

**Why this order.** The photo first, for the reason the photo plan gives:
the other order loses the storage key if R2 then fails, leaving private
bytes nothing points at. The early refusal in step 1 keeps a sender from
losing their photo to a delete that would then be refused. A reply that
lands between step 1 and step 5 still leaves the sender with the photo
deleted and the message refused. They were allowed to delete the photo
anyway; the bubble shows "Photo removed" and the alert says why.

**The transaction failing after the photo went (S5).** The handler returns
the error (500), or 403 when a reply landed after the photo went. In both
cases it first re-reads the row and, if `photo_removed()`, publishes
`PhotoRemoved`, so open pages drop the thumbnail. The client shows the
retry alert. The bubble shows "Photo
removed" with its caption, the same state as after photo delete, and safe.
A retry skips step 2 (no photo left) and finishes. A test forces this
(below).

**Reactions** (`reactions::Model::set`): takes `&DatabaseConnection` (T).
With `on = true` it runs begin, then `lock_live`, then `EntityNotFound` if
`None`, then the insert, then commit, so the lock is held inside a real
transaction (S1). With `on = false` it is a plain delete, no lock needed.

**Deadlocks (S4).** Reactions, replies and notices take the message row
first, then write children. The mention-email claim is different: it locks
notification rows directly, and the delete's cascade locks the same rows
after the message. So the claim and a delete can deadlock. Both fail safe:
Postgres aborts one transaction. A delete then answers 500 and the client
shows the retry alert; a claim errors and the next minute's run tries
again. A reaction or reply shows its existing error. Nothing is half-done.

**Unit test:** `may_delete_message` for sender with and without replies,
owner/admin with replies, and another member.

### 2. Notices: `src/models/notifications.rs` and `src/controllers/notifications.rs`

**`Notice` gains `message_id: Option<i64>`,** and `notify` saves it. The ten
non-chat literals get `None`. The chat ones get the message whose text they
quote:

- `message_sent`: `Some(message.id)` on "mentioned you" and "mentioned
  everyone".
- `reply_sent`: `Some(reply.id)` on all four kinds. Deleting a reply
  cascades its notices. Deleting a parent cascades the replies, and with
  them their notices.

**Saved under the lock, in the same transaction (S1).** `message_sent` and
`reply_sent` keep their rules and change how they save:

1. Work out recipients as today, before any lock.
2. `let txn = ctx.db.begin()`. Call `messages::Model::lock_live(&txn, org_id, id)`
   on the message, or the reply. If it is `None`, the message is already
   deleted: commit, return `Ok(())`, nobody is told.
3. Each `send` becomes `notifications::Model::notify(&txn, ...)`. `told`
   works as now.
4. Commit, then publish everyone saved on `NotifyHub` once.

Without the lock, a notice racing a delete would fail its foreign key and
answer 500 on the sender's send. With it, the notice is skipped cleanly.

**The badge after a delete.** `delete_message` collected the recipients of
the cascaded notices (step 7). The handler publishes them on `NotifyHub`
after commit, so their open pages reload the bell badge.

**The bell list** needs no change: the rows are gone.

### 3. Mention emails: `src/mailers/mentions.rs` (S4)

Cascade removes the notice, so `due_mention_emails` can't find a deleted
message's mentions. The gap between loading and sending is closed by
turning `mark_emailed` into a claim:

```sql
UPDATE notifications SET emailed_at = now()
 WHERE id IN (...) AND kind = 'mention' AND read_at IS NULL AND emailed_at IS NULL
RETURNING *
```

via `UpdateMany::exec_with_returning` (SeaORM 2.0.4). `send_due` sorts the
returned rows by id (RETURNING has no order) and builds the emails from
those only.

- If a delete's transaction holds a notice row (its cascade locks it), the
  claim waits, then finds the row gone, and skips it.
- Chat notices with no `message_id` (made by the old code during a deploy)
  are never claimed.
- If the claim wins, the email was in effect sent before the delete, which
  is the brief's "already gone out" case.
- `kind` and `read_at` in the `WHERE` mean a notice read since it was
  loaded is not emailed either.

### 4. Route and access: `src/controllers/chat.rs`

```rust
/// Deletes a message for everyone, for good, with its photo, reactions and
/// notices. The sender may delete it while it has no replies; owners and
/// admins who are in the conversation may delete any, and a parent's
/// replies go with it.
#[debug_handler]
async fn delete_message(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    Path((id, message_id)): Path<(i64, i64)>,
) -> Result<Response> {
    member.require_own_membership()?;                                    // acting: 403
    let conversation = conversations::Model::find_for_member(
        &ctx.db, member.org.id, id, member.user.id).await?;              // not in it: 404
    let message = messages::Model::find_in_conversation(
        &ctx.db, &conversation, message_id).await?;                      // elsewhere, or gone: 404
    let manager = member.can_manage();
    if !messages::may_delete(message.user_id, member.user.id, manager) {
        return Err(forbidden(
            "Only the person who sent a message, owners and admins can delete it.",
        ));                                                              // other member: 403
    }
    if !photo_file_allows(&ctx, &member, &message).await? {
        return Err(forbidden(
            "Only the person who sent a message, owners and admins can delete it.",
        ));                                                              // Documents rule: 403
    }
    let had_photo = message.photo_file().is_some();
    let (conversation_id, author_id) = (message.conversation_id, message.user_id);
    let parent_id = message.parent_id;
    let deleted = message.delete_message(&ctx, manager).await;
    let replies = match deleted {
        Ok(replies) => replies,
        Err(ModelError::Message(text)) => return Err(forbidden(&text)), // sender, has replies: 403
        Err(err) => return Err(err.into()),                              // gone meanwhile: 404; else 500
    };
    tracing::info!(
        org_id = member.org.id,
        conversation_id,
        message_id,
        parent_id,
        actor_id = member.user.id,
        by_manager = author_id != member.user.id,
        had_photo,
        replies,
        "chat message deleted"
    );
    // Saved; tell open pages, but a failed broadcast must not undo the answer.
    if let Err(err) = super::chat_ws::publish_message_deleted(&ctx, member.org.id, conversation_id, message_id, parent_id).await {
        tracing::error!(error = %err, message_id, "could not broadcast a deleted message");
    }
    format::render().empty()                                             // 200
}
```

- Same access order as `delete_photo`: own membership, conversation,
  message, rule, then the work.
- `photo_file_allows` is the #general Documents check lifted out of
  `delete_photo` (lines 890-899), shared by both handlers.
- **The log (S7)** carries ids, two flags and a count. It never carries the
  body or a caption. It is the only record a delete happened; the PR says
  so.
- **A repeat delete (T).** The row is gone, so `find_in_conversation`
  answers 404, the same as for an id that never existed. The server can't
  tell "you already deleted it" from "someone else did" without keeping a
  record, which hard delete rules out. The client resolves it: a 404 on the
  confirm's first attempt means someone else got there first ("This message
  was already deleted."). A 404 after this confirm's own earlier failed or
  lost attempt means that attempt landed, and it shows success.

**Route,** next to the reactions route:

```rust
.add("/chat/{id}/messages/{message_id}/delete", post(delete_message))
```

**Other handlers:**

- `actions`, `react`, `open_photo`, `delete_photo`, `thread_feed` on a
  deleted id: 404 through `find_in_conversation` or `find_thread_parent`,
  with no code change. `react` also refuses under the lock.
- `thread` on a deleted parent: the existing `thread_missing.html` (404),
  "This thread isn't available".
- HTTP `reply` and the socket's `receive`: `create_reply` refuses a parent
  deleted meanwhile (404, or a logged rejection on the socket).

**Who gets what,** at `/o/juniper-tea-co/chat/{finance}/messages/{message}/delete`.
#finance is Ana's (owner) group with Maya (admin), Tom and Leo; #suppliers is
Tom and Leo only.

| Who | Stops at | Answer |
|---|---|---|
| Signed out | `CurrentUser` | 303 to `/login` |
| Tom, his message, no replies | | 200 |
| Tom, his message with replies | `delete_message` step 1 or 5 | 403 "This has replies. Ask an owner or admin to delete it." |
| Ana (owner) or Maya (admin), any #finance message | | 200; a parent's replies go too |
| Leo, Tom's #finance message | `may_delete` | 403, nothing changed |
| Leo, his own reply | | 200 |
| Ana or Maya, a #suppliers message (not in it) | `find_for_member` | 404 |
| In #finance, a #suppliers message id | `find_in_conversation` | 404 |
| A message already deleted | `find_in_conversation` | 404 |
| Pending in Juniper Tea Co | `CurrentMember` (waiting page) | 403 |
| Removed from Juniper Tea Co | `CurrentMember` (removed page) | 403 |
| Member of another org only | `CurrentMember` | 404 |
| Super admin acting in Juniper Tea Co | `require_own_membership` | 403 |
| Tab drawn for another org (`X-Org-Id` mismatch) | `CurrentMember` claim | 409 + `HX-Redirect` |
| Cross-site form or fetch | `origin_check` | 403 |

### 5. The view: `MessageView`, the sheet and the confirm

**`MessageView`** gains:

```rust
/// The viewer may delete this message: the sheet's "Delete message" row.
/// Display only; the delete route checks again.
pub can_delete: bool,
/// The viewer sent it but it has replies: the sheet says to ask an owner
/// or admin instead of offering the row.
pub delete_blocked: bool,
```

`allow_photo_delete` becomes `allow_delete(viewer_id, can_manage, replies)`:

```rust
pub fn allow_delete(&mut self, viewer_id: i64, can_manage: bool, replies: u64) {
    self.can_delete = messages::may_delete_message(self.author_id, viewer_id, can_manage, replies);
    self.delete_blocked = !self.can_delete && self.author_id == viewer_id;
    self.can_delete_photo = self.photo && messages::may_delete(self.author_id, viewer_id, can_manage);
}
```

`replies` comes from the `thread_stats` the feed and the sheet already load
(the thread line's `count`; 0 for replies). Called where
`allow_photo_delete` is called today, and also in HTTP `reply` and socket
`render_reply` with `replies = 0`. A new message's live render gets 0.
When the first reply arrives, the open sender's page doesn't re-render the
parent. The sheet is fetched fresh on open, so it always shows the current
row or line. The route checks again under the lock.

`_message.html`: the `.msg` div gains `data-author-id="{{ message.author_id }}"`
and `data-author="{{ message.author }}"`, for the client-side regroup
(section 7). No other change: there is no placeholder.

**`_actions.html`** (§1):

- Quote line 6 follows the mockup: a live photo shows "Photo · {caption}"
  or "Photo"; "Photo removed" and text messages as today.
- The "Delete photo" row (line 20) is replaced:

```html
{% if message.can_delete %}
<div class="list"><button type="button" class="list-row action-row text-danger w-full border-0 bg-transparent text-left" data-message-delete="{{ message.id }}">{{<icon name="trash" class="h-6 w-6"/>}}<span class="flex-1">Delete message</span></button></div>
<template data-message-delete-body>{% include "chat/_message_delete.html" %}</template>
{% elif message.delete_blocked %}
<p class="m-0 px-1 text-[15px] text-muted">This has replies. Ask an owner or admin to delete it.</p>
{% endif %}
```

  Same classes and place as the row it replaces. On the thread page there
  is no "Reply in thread", so the row sits under the reactions (§1
  decision 4). Replies get it too.
- A hidden block for "deleted while open" (§4 decision 4):
  `<div data-sheet-gone hidden><p class="m-0 text-[15px] text-muted">This message was deleted while you had it open.</p><button type="button" class="btn btn-lg btn-secondary w-full" data-dialog-close>Close</button></div>`.
  Every sheet gets it.
- `.msg-actions` gains `data-message="{{ message.id }}"`.
- `actions` loads `member_ids` and `label` (for the confirm's place) and
  `thread_stats` for any top-level message, not only when `can_reply`.

**New `assets/views/chat/_message_delete.html`:** the confirm body (§2),
rendered by the server into the sheet's `<template>`, so every word is
Tera-escaped and nothing comes from `data-*`:

```html
<div class="flex flex-col gap-4">
  <h2 id="message-delete-title" class="m-0 text-[19px] font-bold">Delete this message?</h2>
  <div class="quote">
    <div class="quote-head">{{<avatar name={message.author} color={message.color} size="xs" label={false}/>}}<span>{% if message.own %}You{% else %}{{ message.author }}{% endif %}</span><span class="meta">{{ message.at }}</span></div>
    <p>...the same quote text as the sheet...</p>
  </div>
  <p class="m-0 text-[15px] text-muted">{% if conversation.kind == "dm" %}It's removed for you and {{ conversation.label }}.{% else %}It's removed for everyone in #{{ conversation.label }}.{% endif %}{% if message.photo %} The photo is deleted too.{% endif %}{% if message.photo and conversation.kind == "channel" %} It's also deleted from Documents.{% endif %}{% if replies == 1 %} Its reply is deleted too.{% elif replies > 1 %} Its {{ replies }} replies are deleted too.{% endif %} This can't be undone.</p>
  <div class="alert alert-error" role="alert" data-message-delete-alert hidden>{{<icon name="circle-alert" class="h-[18px] w-[18px] shrink-0"/>}}<span data-message-delete-error>Couldn't delete the message. Check your connection and try again.</span></div>
  <div class="flex flex-col gap-2" data-message-delete-buttons>
    <button type="button" class="btn btn-lg btn-danger-solid w-full" data-message-delete-confirm data-url="{{ org_url(path="/chat/" ~ (message.conversation_id) ~ "/messages/" ~ (message.id) ~ "/delete") }}"><span data-message-delete-spin hidden>{{<icon name="loader-circle" class="spin h-[18px] w-[18px]"/>}}</span><span data-message-delete-label>Delete</span></button>
    <button type="button" class="btn btn-lg btn-secondary w-full" data-message-delete-cancel data-dialog-close>Cancel</button>
  </div>
  <div data-message-delete-gone hidden><button type="button" class="btn btn-lg btn-secondary w-full" data-dialog-close>Close</button></div>
</div>
```

- The sentences go in the mockup's fixed order: where, photo, Documents,
  replies, can't be undone. The replies line only appears for an owner or
  admin, since only they can open this on a parent with replies.
- No password advice in the confirm (S8). The PR text says "if it was a
  password, change it": deleting removes it from the chat, not from
  anyone's memory, screenshots or sent emails.
- `.confirm`, `.lead2` and `.btns` are mockup-only. It uses utilities with
  the same sizes, as `_photo_delete.html` does.

**Dialog shell** in `show.html` and `thread.html`, after `_photo_delete.html`:

```html
<dialog id="message-delete" class="sheet" aria-labelledby="message-delete-title"><div class="sheet-handle"></div><div class="sheet-body" data-message-delete-slot></div></dialog>
```

**`thread.html`:** the composer form gains `id="thread-composer"`. A new
partial, `_thread_gone.html`, holds the "deleted while open" state:

```html
<div class="center-state" data-thread-gone>{{<icon name="messages-square" class="h-9 w-9"/>}}<h2>This thread was deleted</h2><a href="{{ org_url(path="/chat/" ~ (conversation_id)) }}" class="btn btn-secondary">Back to chat</a></div>
```

A plain "Back to chat", so the socket's `Session` needs no label. The
server never closes the socket. On a reload the page is the existing
`thread_missing.html`.

**`last_line`, the chat list and Home:** no change. They read
`latest_per_conversation`, which now returns the message before the deleted
one, or nothing ("No messages yet"; Home skips a conversation with no last
message, `dashboard.rs:194`).

### 6. Live update: `src/data/chat_hub.rs` and `src/controllers/chat_ws.rs`

**A new event, not a wider `PhotoRemoved`.** They differ in who wants them
(replies), what they carry (a reply's delete changes the parent's thread
line) and what they do (remove versus swap). `PhotoRemoved` stays for the
viewer's "Delete photo" and for a Documents delete.

```rust
/// A message was deleted, and with a parent its replies. Main-feed sockets
/// remove a top-level message, or update a parent's reply line when a reply
/// went. A thread socket shows "This thread was deleted" for its parent, or
/// removes a reply and updates the count. Ids and counts only: no text.
MessageDeleted {
    conversation_id: i64,
    message_id: i64,
    parent_id: Option<i64>,
    /// After a reply's delete: the parent's new line (`None` when no replies
    /// are left) and total.
    line: Option<Box<ThreadLine>>,
    count: i64,
},
```

**`publish_message_deleted(ctx, org_id, conversation_id, message_id, parent_id)`:**
for a reply, it calls `thread_stats` and `thread_line` for the parent, as
`publish_reply` does. `ThreadLine` carries replier names and times, never
text.

**`wants()`:** add it to the conversation-id match, then:

- main feed: always. Either the message itself or its parent's line is
  there.
- thread socket: when `message_id == thread` (the parent went) or
  `parent_id == Some(thread)`.

**Render** (`render_deleted`). Every element carrying `hx-swap-oob` is
top-level in the fragment, never nested (T):

| Socket | Deleted | Fragment |
|---|---|---|
| Main feed | top-level | `<div id="message-N" hx-swap-oob="delete"></div>` |
| Main feed | reply | `_thread_link.html` with `oob`, parent id, `line` (empty slot when `None`) |
| Thread | its parent | `<div id="chat-feed" hx-swap-oob="innerHTML">` + `_thread_gone.html` + `</div>`, then `<div id="thread-composer" hx-swap-oob="outerHTML"></div>` |
| Thread | a reply | `<div id="message-N" hx-swap-oob="delete"></div>` then `_thread_count.html` with `oob` and `count` |

`hx-swap-oob="delete"` is a standard htmx 2 swap style. It is checked by hand
in the Checks below; the fallback is an empty `outerHTML` swap plus the
client's regroup removing empty `.msg` shells.

A deleted parent also cascades its replies. A main-feed socket never shows
replies, and the thread socket replaces the whole feed, so one event is
enough.

**Reply counts and `grows`.** A delete legitimately lowers a count. On
`MessageDeleted` for a reply, the socket sets `sent_replies[parent] = count`
directly, so the next `Reply` event (count + 1) passes `grows`. A `Reply`
event arriving late with a count above the post-delete truth could still
show one too many until reload. That is rare and self-correcting, and
accepted.

**A page that missed the event** reloads its feed on reconnect
(`app.ts:276-284`) and on a back-forward restore (`pageshow`). Both draw
from the database.

### 7. `frontend/app.ts`

All new text comes from server fragments or `textContent`. The confirm body
moves from the sheet's `<template>` with `importNode`, the same trust as
`openActions`' `innerHTML` of its own fragment.

**Share the confirm machinery.** `photoDeleteParts` and `photoDeleteBusy`
take a prefix: `confirmParts(dialog, "photo-delete" | "message-delete")`
and `confirmBusy(dialog, prefix, busy)`. The photo code's behaviour is
unchanged. `fetchRefusalText`, `staleRefusal`, `orgHeaders` and the global
backdrop rule's `data-busy` skip are reused. `#message-delete` gets the same
`cancel` listener (Escape does nothing while busy).

**The sheet row.** Remove the `[data-photo-delete]` branch from the click
router; the sheet no longer has that row. A click on `[data-message-delete]`:

1. If `#message-N` is no longer on the page, show the sheet's gone state
   and stop.
2. Copy `template[data-message-delete-body]` into `#message-delete`'s slot,
   set `dataset.message = N`, and set `dataset.tried = ""`.
3. Close `#chat-dialog` and call `showModal()`.

**Deleting**, as `deletePhoto`:

1. Busy: `data-busy`, spinner and "Deleting...", both buttons locked, and
   `deletingMessage = N`.
2. `fetch(url, { method: "POST", headers: { "HX-Request": "true", ...orgHeaders() }, credentials: "same-origin" })`.
3. `staleRefusal(res)`: un-busy, close, stop.
4. `res.ok`: close and toast "Message deleted" (§3 decision 6). The socket
   removes the bubble. Fallback after 1.5 s: if `#message-N` is still
   there, reload `#chat-feed` from `data-chat-feed-url`.
5. 404: if `dataset.tried` is set, our earlier attempt landed, so do as
   step 4. Otherwise show the gone state (§4 decision 3): the alert says
   "This message was already deleted.", Delete and Cancel hide, and one
   Close shows.
6. 403: the alert shows `fetchRefusalText` (the thread rule's text or the
   member refusal).
7. Anything else (a 500 from a lock timeout, a deadlock or the transaction
   failing after the photo went), or a network error: set
   `dataset.tried = "1"`, un-busy, and show "Couldn't delete the message.
   Check your connection and try again." Delete is the retry. Show a toast
   instead only if the confirm was already closed (S4, S5).

**Regroup after a removal.** Only around a removal, never on every socket
message. `htmx:oobBeforeSwap` on a `#chat-feed > .msg` (only the
`hx-swap-oob="delete"` swap targets a whole message) records its two
neighbours; `htmx:wsAfterMessage` regroups just those two. A feed reload
is drawn by the server and needs nothing. A message that came over the
socket carries `data-live` and always keeps its run start, as the CSS
straddle rule expects. Any non-`.msg` element (the thread divider, the
empty state) is a break. It does what `mark_runs` does on the server:

- `msg-start` when the previous `.msg`'s `data-author-id` differs.
- The avatar `.msg-av` is `visible` when the next one's differs, else
  `invisible`.
- For a message that is not `msg-own` and is now a start, insert a
  `.msg-name` with `textContent = data-author` if missing. Remove it when
  the message is no longer a start.

So when Tom's middle message of three disappears, his two neighbours stay
one run. When Leo's only message between two of Tom's goes, Tom's two
merge: the second loses its start and name, and the first loses its
avatar. Bubble corners follow `msg-start` through the existing CSS. On a
thread page the parent is above the divider, so it is never merged with a
reply.

**Other screens** (`syncDeleted`, on the same events):

- `#message-delete` open on N, not busy, and `#message-N` gone: the gone
  state, as in step 5.
- `#chat-dialog` open with `.msg-actions[data-message="N"]` and `#message-N`
  gone (§4 decision 4): the quote text becomes "Message deleted" in
  italics; picks, lists, the section label and the row hide;
  `[data-sheet-gone]` shows. It never says who deleted it.
- On a thread page that switched to "This thread was deleted": close any
  open sheet, confirm or viewer for its messages.
- **The viewer.** `syncViewer` returns early today when `#message-N` is
  missing. Change it: if the viewer is open on N and `#message-N` is gone,
  show the removed state ("Photo removed") and hide the caption too. If it
  is present but `#photo-N` is no longer a link, show the removed state as
  today and keep the caption. Skip `deletingMessage`, as it skips
  `deletingPhoto`.

**`pageshow`.** The existing reload covers deleted messages; its comment
says so.

### 8. CSS: `frontend/styles/app.css`

No placeholder, so the mockup's `.bubble-deleted`, `.msg-deleted` and
`.excerpt-deleted` rules are not needed. The one style still used is the
sheet's gone-state quote: `.excerpt-deleted { font-style: italic; }`. The
shared `.photo-removed, .msg-deleted` rule the designer suggested has no
second user now, so `.photo-removed` stays as it is. Then
`bin/build-assets`.

## Read paths, all together

Verified against the code above. The row is gone, so nothing needs a
filter:

| Place | After the delete |
|---|---|
| Main feed (`recent`, `grouped`) | Not there; neighbours regroup live, and are right from the server on reload |
| Thread page (`replies`, `thread_feed_data`) | A reply is gone and the count drops; a deleted parent gives `thread_missing.html` |
| Reply line (`thread_stats`) | Counts the remaining replies; the live line is updated |
| Unread (`unread_counts`) | Counts rows, so it drops by one if unread |
| Read receipts (`read_in_span`, `receipt`) | Computed from remaining rows and read marks; nothing points at the gone row |
| Chat list and Home (`latest_per_conversation`, `last_line`) | The message before it, or "No messages yet" |
| Bell (`notifications::index`, `unread_count`) | Notices cascaded away; the badge reloads |
| Mention email (`send_due`) | Nothing to claim |
| Photo (`open_photo`) | 404 |
| Welcome checklist (`guide.rs:33`) | "Say hello" goes back to not done if it was their only #general message. Accepted. `tagged` (line 40) can also go back if the mention notices were the only ones |
| Search | None exists |
| Claude connector | No tool reads chat (`src/mcp/mod.rs:6`) |

## Edge cases

- **A parent with replies, deleted by an admin.** Gone with every reply,
  their reactions and their notices. Open thread pages show "This thread
  was deleted". The main feed removes the bubble.
- **The sender's parent gets its first reply while the sheet is open.** The
  row shows; a tap gives the 403 text in the alert.
- **A photo message.** The photo goes first. A #general photo's Documents
  file goes too, unless it is no longer a photo type, in which case it is
  left in Documents.
- **The viewer open on someone else's screen.** Removed state, caption gone.
- **The 200 history limit.** A message older than the last 200 is not on
  the page. The delete OOB finds no target and htmx logs `oobErrorNoTarget`.
- **A role change with a socket open.** Display only; the route checks the
  role.
- **@everyone in a deleted message.** Every notice it made cascades.

## Tests

Run as usual (`cargo test`, Postgres, `#[serial]`). Memory storage driver,
as in `chat_photo_delete.rs`. Names are made up: Juniper Tea Co with Ana
(owner), Maya (admin), Tom and Leo. The `juniper` helper moves from
`chat_photo_delete.rs` to a shared module. #finance has all four; #suppliers
is Tom and Leo. Texts: "@ana I quoted Leeds £4.20 a tin by mistake. Can we
talk before they pay?", "Who can do the stall from 7 to 10 on Saturday?"
with three replies, and "Courier invoice from Tuesday, £18.40" with
`secret_png()`.

**Race tests force the order (S1).** They hold a lock in a test transaction
on a second connection instead of relying on `tokio::join!` timing:

- Start the other side with `tokio::spawn`.
- Prove it is waiting: it hasn't finished after 200 ms, and `pg_locks`
  shows a waiting lock.
- Commit or roll back the test transaction, then assert the outcome.

### `tests/models/messages.rs`

- `deleting_removes_the_row_reactions_and_notices`: the row, its reactions
  and its notifications are gone.
- `deleting_a_parent_cascades_its_replies`: three replies, their reactions
  and their notices are gone.
- `the_sender_cannot_delete_a_message_with_replies`: `Message(HAS_REPLIES)`.
  The photo is still there (early refusal) and the row too.
- `deleting_a_group_photo_message_removes_the_bytes` and
  `deleting_a_general_photo_message_deletes_its_documents_file`.
- `a_general_file_that_is_no_longer_a_photo_is_left_in_documents`: set the
  file's `content_type` to PDF. The message is deleted and the file stays.
- `a_failed_transaction_after_the_photo_keeps_the_message` (S5): a test
  transaction holds `FOR UPDATE` on the message. `delete_message` fails on
  the 5 s lock timeout. Bytes are gone, the row and text are kept, and
  `photo_removed()` is true. Release, retry, and it is deleted.
- `a_reply_waiting_on_a_delete_is_refused` (S1): the test holds `FOR UPDATE`
  and deletes the parent; a spawned `create_reply` waits. Commit, and it is
  `EntityNotFound`.
- `a_reply_in_flight_stops_the_senders_delete` (S1): the test locks the
  parent `FOR SHARE` and inserts a reply uncommitted; a spawned sender
  `delete_message` waits. Commit, and it is `Message(HAS_REPLIES)`.
- `a_reaction_waiting_on_a_delete_is_refused` (S1): the same shape for
  `reactions::Model::set`; no reaction row afterwards.
- `a_notice_waiting_on_a_delete_is_skipped` (S1): the same for
  `message_sent`. It is `Ok`, with no notification rows and no foreign key
  error.
- `replies_never_carry_photos`: `create_reply` rows have `has_photo` false
  and no photo columns. This holds the cascade guard's assumption.

### `tests/models/notifications.rs`

- `a_claim_returns_only_unread_unsent_mentions_in_order`: a read one, an
  emailed one and a note kind are left out, and the rows come back sorted
  by id.
- `a_claim_waiting_on_a_delete_skips_the_notice` (S4): the test transaction
  deletes the message (cascading the notice) and holds. A spawned claim
  waits; commit; it returns nothing.

### New `tests/requests/chat_message_delete.rs`

Registered in `tests/requests/mod.rs`. It subscribes to `ChatHub` before
acting.

- `the_sender_deletes_a_message_for_everyone`: Tom's Leeds text, with
  Leo's reaction. Tom POSTs delete: 200. One `MessageDeleted` with ids and
  no parent. Maya's `/chat/{general}` has no `message-N` and no "Leeds". A
  raw `SELECT count(*) FROM messages WHERE body LIKE '%Leeds £4.20%'` is 0.
- `owners_and_admins_delete_anyones_message_and_its_replies`: Maya deletes
  Tom's stall question with three replies: 200. The parent and replies are
  gone, `/threads/{id}` is 404 "This thread isn't available", and the
  repliers' notices are gone. Ana deletes Leo's DM message: 200.
- `the_sender_is_refused_once_there_are_replies`: Tom on his stall question
  gets 403 with "This has replies. Ask an owner or admin to delete it.",
  and nothing changed. Tom's sheet shows that line and no "Delete
  message"; Maya's sheet shows the row and "Its 3 replies are deleted too."
- `a_member_cannot_delete_someone_elses_message`: Leo on Tom's message
  gets 403, nothing changed and no event. Leo's sheet has neither the row
  nor the line.
- `people_outside_the_conversation_cannot_delete`: a #suppliers message.
  - Ana and Maya: 404.
  - Cross-conversation ids: 404.
  - Signed out: 303. Pending: 403 waiting page. Removed: 403. Acting:
    403. Cross-site `Origin`: 403.
  - The text is still in the database and no event was published.
- `a_photo_message_loses_its_photo_too`: Leo's #finance photo. Bytes gone,
  the photo GET 404, no `SECRET` or caption in Maya's feed. The #general
  variant: the Documents row and bytes are gone.
- `a_repeat_delete_is_not_found`: the second POST is 404.
- `mentions_go_with_the_message_and_are_never_emailed`: Tom mentions @ana,
  and Ana's notice has `message_id`. Tom deletes the message. Ana's
  `/notifications` has no "Leeds" and her unread count dropped. Age every
  remaining mention past 15 minutes: `send_due` returns 0. A live mention
  elsewhere still sends 1.
- `reply_notices_go_with_the_reply`: Leo's reply mentioning @maya.
  Deleting the reply removes Maya's mention and Tom's "replied to your
  message".
- `a_deleted_reply_leaves_the_thread`: Leo deletes his reply. The thread
  page lists two, the count says "2 replies", and the main feed's line
  says 2. The event has the parent id, `count: 2` and a line.
- `the_list_and_home_show_the_message_before`: deleting the last message
  shows the previous one in Ana's list and Home. Ana's unread count drops
  by one.
- `the_confirm_names_what_goes`: #general text; #finance photo ("The photo
  is deleted too."); #general photo as Maya ("It's also deleted from
  Documents."); a parent with one reply as Maya ("Its reply is deleted
  too."); Leo's DM ("It's removed for you and ana."); a reply's sheet
  (`?thread=1`) has the row and no replies line.
- `the_socket_payload_carries_no_text` (T): for each case in the render
  table, build the event and render the fragment as Maya's socket would.
  Neither the `Debug` of the event nor the HTML contains "Leeds" or the
  caption.
- `viewer_delete_photo_still_keeps_the_caption`: the sheet has no "Delete
  photo". The photo link keeps `data-delete-url`. The photo route still
  deletes only the photo.
- `a_failed_delete_answers_500_and_a_retry_works` (S5): the request-level
  twin of the model test, with the test transaction holding the row.

### `tests/requests/tenancy.rs`

- `org_a_cannot_reach_org_b`: add the #general and group message delete
  routes to `posts`: 404. The closing asserts check Globex's messages are
  still there.
- `documents_and_removal_need_your_own_active_membership`: add the group
  message delete: waiting, removed and acting get 403.

### `tests/requests/stale_tab.rs`

No edit. The route walk (line 89) picks up the new POST.

### Unit tests

- `messages.rs`: `may_delete` and `may_delete_message`.
- `chat.rs`: `allow_delete` for sender with and without replies, manager,
  and other member; `can_delete_photo` is unchanged by replies.
- `chat_ws.rs`: `MessageDeleted` joins `every_event`.
  - Top-level: the main feed and its own thread page.
  - Reply: the main feed and its parent's thread page.
  - Never another conversation.
- `chat_ws.rs`: after a reply's delete, `sent_replies` accepts the lower
  count and the next reply passes `grows`.

### Source checks (T)

In `chat_message_delete.rs`, in the shape of
`photo_bytes_are_only_deleted_by_the_models`. It walks `src/` with all
whitespace stripped from each file's text, so formatting can't hide a call:

- `messages_are_only_deleted_by_the_model`:
  - Outside `src/models/messages.rs`, no file contains `messages::Entity::delete`,
    `Messages::delete` or `messages::ActiveModel::delete`. `app.rs`'s
    `truncate_table(.., messages::Entity)` matches none of them.
  - Inside it, `Entity::delete` (also a prefix of `Entity::delete_by_id`)
    appears exactly once, in `delete_message`.
- The existing photo check still holds unchanged.

### Not covered by `cargo test`

No JS runner. The confirm's busy lock, Escape and backdrop while busy, the
gone states, the regroup, the thread-gone state, the toast, the fallback
reload and the viewer caption are all checked by hand.

## Commits

One PR. Each commit builds and passes `cargo test`.

1. **Notices know their message.** Migration, entities, `Notice.message_id`,
   the locked save in `message_sent` and `reply_sent`, the claim in
   `send_due`. Tests.
2. **Messages can be deleted.** `may_delete`, `may_delete_message`,
   `lock_live`, `create_reply`'s lock, the `reactions::set` lock,
   `delete_message`. Model and race tests.
3. **A route to delete a message, and a live event.** The handler and
   route, `photo_file_allows`, `ChatEvent::MessageDeleted`,
   `publish_message_deleted`, `wants`, `render_deleted`, `sent_replies`,
   the `MessageView` fields, the `.msg` data attributes, `_thread_gone.html`,
   the composer id. Request, tenancy, source and unit tests.
4. **Delete message in the sheet.** The row and the blocked line,
   `_message_delete.html`, the dialog shell, the gone block, `app.ts`
   (confirm, regroup, sync, viewer), CSS, `bin/build-assets`. Template
   tests.

## Out of scope

As the brief: editing; undo, a bin or a grace period; telling the sender or
showing who deleted it; deleting several messages or a whole conversation;
recalling a sent email; long-press menus; the Claude connector. Also:

- Making the chat list, Home or the bell update live, beyond the badge
  reload.
- Backfilling `message_id` on older notices.
- An audit log beyond the info line.
- A placeholder, which hard delete replaces.

## Risks

- **Privacy and access.** A new write route that destroys data, now
  including other people's replies. Same gate as `delete_photo`, plus the
  thread rule under a lock, tenancy and per-role tests. The security
  reviewer must review this plan and the diff.
- **Permanent, and wider than one message.** An admin deleting a parent
  takes everyone's replies. The confirm says how many. The info log is the
  only record (S7); the PR says so.
- **Text that outlives the delete.** Emails already sent, quotes in notices
  made before deploy, and Railway database backups. Assumption from the
  brief: backups age out; Raj to confirm Railway's retention before merge.
  R2 versioning must be off, as for photo delete. The PR says "if it was a
  password, change it" (S8).
- **Mention emails skipped at deploy.** Up to 15 minutes' worth, by design
  (S4). Their bell notices stay.
- **Lock waits.** A reaction, reply or notice save can wait on a delete for
  one small transaction. There is no network call inside it; the photo goes
  before. `lock_timeout` caps the delete at 5 s, and a timeout fails safe
  with the retry alert.
- **The photo-then-transaction gap.** A failure between them leaves "Photo
  removed" with the caption until a retry (S5). A reply landing in that gap
  refuses the sender's delete after their photo went. Rare, and the photo
  delete was within their rights.
- **Client-side regroup.** It must match `mark_runs`. If it drifts, the
  next feed load is right. Covered by the hand checks.
- **`hx-swap-oob="delete"`.** A standard htmx 2 style, checked by hand; the
  fallback is described in section 6.
- **Late `Reply` events after a delete** may show a count one too high
  until reload.

## Rollback

Revert the PR. Leaving the migration in place is safe: old code ignores
`message_id`. Rows deleted while the PR was live stay deleted, with their
replies, reactions and notices. Running `down` drops the column and index.
The deploy-time `emailed_at` marks stay.

## Checks

```sh
bin/build-assets
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

By hand, locally, with realistic data: Juniper Tea Co with Ana (owner),
Maya (admin), Tom and Leo; #general, #finance and a DM between Leo and Ana;
a pending member; Ana also in a second org. Use the mockup's messages:

- Tom's wrong Leeds price, mentioning @ana.
- Tom's stall question, with three replies.
- Leo's courier invoice photo in #finance.
- Leo's meter reading, in the DM.

No real password, and no fake-looking one, in the data or the screenshots.

On a phone-sized window (390px) and desktop (1440px), light and dark:

1. Tom taps his Leeds message: "Delete message" in red at the bottom. On
   his stall question: "This has replies. Ask an owner or admin to delete
   it." and no row. Leo's sheet on Tom's message has neither.
2. Tom deletes the Leeds message. The confirm says "It's removed for
   everyone in #general. This can't be undone." Cancel changes nothing.
   Delete shows the spinner and locks both buttons; Escape and the
   backdrop do nothing. Then the "Message deleted" toast, and the bubble is
   gone.
3. Maya has #general open on her phone: the bubble disappears without a
   reload. Tom's messages either side merge into one run (one name, one
   avatar). With that message's sheet open on her screen, the sheet shows
   the gone state.
4. DevTools offline: the red alert, and the message stays. Back online,
   Delete: it goes once (the 404-after-retry path).
5. Maya and Ana open the confirm on the same message; Maya deletes. Ana's
   confirm shows "This message was already deleted." and Close.
6. Ana's bell: the mention is gone and the badge drops. No email arrives
   after 15 minutes (dev mailer).
7. Leo deletes his #finance photo message: "The photo is deleted too." Ana,
   with the photo open full screen, sees "Photo removed" and the caption
   goes.
8. Maya deletes Tom's stall question: "Its 3 replies are deleted too." An
   open thread page shows "This thread was deleted" with "Back to
   #general", and the composer goes. Leo deletes his reply in another
   thread: it disappears, the divider and the main feed's line drop by one.
9. The chat list and Home show the previous message. Ana's unread badge
   drops by one on reload.
10. In the viewer, Tom can still delete only the photo; the caption stays.
11. Back-forward: leave #general, delete from another tab, come back: the
    feed is redrawn.
12. Removing via `hx-swap-oob="delete"` works in Safari, Chrome and Firefox.

### Screenshots needed

On the `pr-screenshots` branch under `chat-message-delete/`, linked by
`https://raw.githubusercontent.com/rajgurung/collaboration_tool/pr-screenshots/chat-message-delete/<file>.png`.
Taken from the local app with the data above. Phone 390px and desktop
1440px; dark where listed.

- **Sheet.**
  - Tom's message with "Delete message": phone light and dark, desktop
    light.
  - Tom's parent with the "has replies" line: phone light.
  - Maya on Leo's photo: phone light.
  - Leo on Tom's message, with no row: phone light.
  - Leo's thread reply: phone dark.
- **Confirm.**
  - Text in #general: phone light, desktop light.
  - Photo in #finance: phone light.
  - #general photo: phone dark.
  - Parent with 3 replies as Maya: phone light, desktop dark.
  - DM: phone light.
- **Busy and failed.**
  - Deleting: phone.
  - Couldn't delete: phone, desktop dark.
  - The 403 "has replies" alert: phone.
  - Already deleted: phone.
  - Sheet open when deleted: phone dark.
- **After.**
  - Tom's feed with the toast: phone light, desktop dark.
  - Maya's live copy, regrouped: phone dark.
  - The "This thread was deleted" page: phone light, desktop light.
  - The thread after a reply's delete: phone light.
  - The viewer's removed state: phone.
  - The chat list and Home after deleting the last message: phone light
    and dark, desktop light.

The PR body states:

- What the migration does, that it can be rolled back, and what a rollback
  leaves.
- That unsent mention emails at deploy are skipped.
- That the info log line is the only record of a delete (S7).
- "If it was a password, change it" (S8).
- How access was tested: the test names and their output.

## Decisions for Raj

Each with a recommendation.

1. **A repeat delete is 404,** with the client treating a 404 after its own
   failed attempt as success. Recommend: yes. Hard delete keeps no record
   to answer "already".
2. **Skip mention emails pending at deploy** (S4's simpler option).
   Recommend: yes.
3. **Account deletion now also removes others' notices quoting that
   person,** a side effect of `ON DELETE CASCADE`. Recommend: yes.
4. **Live regroup in the client** rather than re-rendering neighbours on the
   server. Recommend: yes. A socket can't know each viewer's neighbours.
5. **Drop the shared `.photo-removed`/`.msg-deleted` rule,** which has no
   second user under hard delete. Recommend: yes.

## Where this differs from the brief or mockup

- **Hard delete** (Raj's change) replaces the placeholder. Mockup §3 is
  superseded. The brief and the mockup's notes carry a change note.
- **Thread rule** (Raj's change): the sender's sheet shows "This has
  replies. Ask an owner or admin to delete it." instead of the row. The
  mockup has no frame for this line; it uses the sheet's muted text style.
- **Replies copy:** "Its {n} replies are deleted too." / "Its reply is
  deleted too.", in place of "The 3 replies stay in the thread."
- **"This thread was deleted"** with a link back is new, for an open thread
  page.
- The confirm's text is rendered by the server into the sheet and moved
  into `#message-delete`, instead of being filled from `data-*` as the photo
  confirm is.
- The confirm uses utilities, not the mockup-only `.confirm`, `.lead2` and
  `.btns`.
- The sheet quote for a live photo becomes "Photo · {caption}", as the
  mockup draws it.
- The "Message deleted" toast comes from `app.ts`, since the delete is a
  raw `fetch`.
- A pending member gets the waiting page (403), as the brief allows.
- The tests and screenshots use the mockup's wrong Leeds price, not the
  brief's pasted password.
