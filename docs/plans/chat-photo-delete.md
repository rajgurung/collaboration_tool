# Delete a chat photo

Status: Draft, for staff and security review · One PR · Owner: Raj · 25 October 2026

Brief: `docs/briefs/chat-photo-delete.md`. Mockup:
`docs/designs/chat-photo-delete.html` (sections §1 to §5 below are the
mockup's). Visuals and copy are the mockup's. This plan covers what the
server, the templates and `frontend/app.ts` must do.

Decisions already made by Raj: owners and admins may delete someone else's
photo without telling them; the deleter gets the "Photo deleted" toast; the
DM confirm says "It's removed for you and {name}."; one shared
`<dialog id="photo-delete" class="sheet">` serves the sheet row and the
viewer; the thread page's sheet gets the same row for its top-level message;
one PR.

## Summary

The sender, or an owner or admin who is in the conversation, can delete a
photo from a chat message. The message stays, with its caption, reactions and
thread. The bubble shows "Photo removed", the state #general already has.

- **One new route:** `POST /o/{org}/chat/{id}/photos/{message_id}/delete`.
  Same checks as `open_photo`, then sender or owner/admin.
- **One new model method:** `messages::Model::delete_photo`. Group and DM
  photos: delete the bytes from storage first, then clear the four photo
  columns. #general photos: call the existing
  `documents::Model::delete_file`, so the Documents file and its bytes go,
  and the foreign key clears `document_id`.
- **One new socket event:** `ChatEvent::PhotoRemoved`. Every open feed and
  thread page swaps that bubble's photo for "Photo removed". An open viewer
  shows its removed state.
- **Two doors, one confirm:** "Delete photo" at the bottom of the message
  sheet (§1) and Delete next to Download in the viewer (§2). Both open
  `#photo-delete` (§3).
- **Chat list and Home** say "Photo removed" (§4).

No migration: the `messages-own-photo` CHECK already allows a message with
`has_photo = true` and all four photo columns empty (checked below). No new
crate.

A second delete of the same photo answers 200 and changes nothing. A delete
is safe to retry after any failure.

## What the code does today

Read before writing this plan:

- `src/models/messages.rs`. `create_photo` (line 247) sends #general photos
  to `create_documents_photo` (a Documents file in "Chat photos", the message
  holds `document_id`) and group and DM photos to `create_own_photo` (bytes at
  `orgs/{org}/chat/{conversation}/{uuid}`, the message holds `photo_key`,
  `photo_type`, `photo_size`, `photo_name`). `photo_file()` (line 366)
  returns `PhotoFile::Own` when all four columns are set, else
  `PhotoFile::Document(id)`, else `None`. `photo_removed()` (line 389) is
  `has_photo && photo_file().is_none()`. Replies are saved by `insert` with no
  photo fields, and `send_photo` only makes top-level messages, so **a thread
  reply can never carry a photo.** Only a thread's parent can.
- `migration/src/m20261025_120100_add_private_photos_to_messages.rs:23-30`.
  The CHECK is: all four columns NULL, **or** all four set with a photo type,
  `has_photo` and no `document_id`. The first branch does not look at
  `has_photo`, so clearing the four columns on a photo message is allowed.
  `m20261022_120000_add_photo_to_messages.rs:19` makes `document_id`
  `ON DELETE SET NULL`.
- `src/models/documents.rs:326` `delete_file`: storage delete first, then the
  row. "The row stays if the bytes could not be deleted, so the delete can be
  tried again." `can_delete` (line 337): uploader, or `can_manage`.
- `src/controllers/documents.rs:582-598` `delete_file`:
  `require_own_membership`, `find_in_org`, `can_delete` (403 "Only the person
  who added a file, owners and admins can delete it."), then
  `file.delete_file`. Route `POST /documents/files/{id}/delete`. The app has
  no `DELETE` routes; every delete is a `POST .../delete`.
- `src/controllers/chat.rs`. `open_photo` (line 806): `require_own_membership`
  (403 acting), `find_for_member` (404 not in it), `find_in_conversation` (404
  elsewhere), `photo_file()` (404 none). `MessageView` (line 209) has
  `photo: bool`, `photo_removed: bool`, `photo_when`. `last_line` (line 486)
  says "Photo" or "Photo · caption" whenever `has_photo`, so a removed
  #general photo still says "Photo". `actions` (line 962) renders the sheet;
  `?thread=1` only hides "Reply in thread".
- `src/controllers/chat_ws.rs`. `publish`, `publish_reply`,
  `publish_reactions` put a `ChatEvent` on the one in-process `ChatHub`
  (`src/data/chat_hub.rs:16`). Each socket's `run` loop filters with
  `wants()` (line 506) and renders for its own viewer. Fragments are
  out-of-band swaps by id (`_message_oob.html`, `_thread_link.html` with
  `oob`). `Session` (line 35) knows `user_id`, not the role.
- `assets/views/chat/_message.html:8`: the photo link or the
  `.photo-removed` line, inline in `.bubble-main`, with no id of its own.
  `_actions.html:6`: the quote says the body, or "Photo".
  `_photo_viewer.html`: Close, avatar, name, when, Download, stage with
  loading and error states, caption. `show.html:47-48` and
  `thread.html:31-32` hold `#chat-dialog` and include the viewer.
- `frontend/app.ts`. The global rule at line 32 closes any dialog on a
  backdrop click. `openActions` (line 290) fetches the sheet into
  `#chat-dialog`. The viewer (lines 1845-1967) fills itself from the link's
  `data-*` with `textContent`, pushes `{ chatPhoto: token }` on open, and
  every close path ends in its `close` event. `sendStagedPhoto` shows the
  pattern for a raw `fetch`: `{ "HX-Request": "true", ...orgHeaders() }`,
  `staleRefusal(res)` first, and a 1.5 s fallback feed reload.
- CSRF and tenancy: `src/initializers/origin_check.rs` refuses any non-GET
  with a foreign `Origin` (403), on top of `SameSite=Lax`. `CurrentMember`
  checks the tab's `X-Org-Id` claim on every request (409 stale tab).
  `tests/requests/stale_tab.rs:89` walks every non-GET route, so a new POST
  route is covered without listing it.
- Notifications: `controllers/notifications.rs` builds bell and email text
  from `message.body` only (`excerpt(&message.body)`, around line 401). No
  notification code mentions photos or documents. The brief's assumption
  holds: notices never carry the photo.

## What changes, file by file

### 1. The model: `src/models/messages.rs`

**Who may delete.** A free function next to `PHOTO_TYPES`, so the model, the
controller and the view share one rule:

```rust
/// The sender, owners and admins may delete a message's photo, as with a
/// Documents file. The caller must already know the person is in the
/// conversation.
#[must_use]
pub fn may_delete_photo(sender_id: i64, user_id: i64, can_manage: bool) -> bool {
    can_manage || sender_id == user_id
}
```

**`delete_photo`:**

```rust
/// Deletes this message's photo for everyone and returns the message as it
/// now is. The message, caption, reactions and thread stay; `has_photo`
/// stays true, so it shows "Photo removed".
///
/// The stored bytes go first, then the row: if storage fails nothing has
/// changed and the delete can be tried again. A photo that is already gone
/// is not an error. The caller checks membership and `may_delete_photo`.
///
/// # Errors
/// `EntityNotFound` for a message that never had a photo; storage or
/// database errors otherwise.
pub async fn delete_photo(self, ctx: &AppContext) -> ModelResult<Self>
```

In order:

1. `!self.has_photo`: `Err(EntityNotFound)`. A text message or a reply.
2. `photo_file()` is `None`: already removed. `Ok(self)`, nothing touched.
3. `PhotoFile::Own { key, .. }`:
   - `ctx.storage.delete(Path::new(key))`. On error, return it
     (`ModelError::Any`). The row is untouched.
   - Then set `photo_key`, `photo_type`, `photo_size`, `photo_name` to `None`
     on the active model and `update`. `before_save` sets `updated_at`. The
     CHECK's first branch accepts the row.
4. `PhotoFile::Document(file_id)`:
   - `documents::Model::find_in_org(&ctx.db, self.organisation_id, file_id)`.
     `EntityNotFound` here means someone else just deleted it: carry on.
   - Otherwise `file.delete_file(ctx)`: bytes, then the `documents` row. The
     foreign key sets `messages.document_id` to NULL.
   - Reload the message with `find_in_conversation`-style lookup by id and
     org, and return it.

**Why bytes first.** The other order loses the key if storage then fails: the
row would say "removed" while the private bytes stay in R2 with nothing
pointing at them, and no retry could find them. That is exactly what the
brief asks us to avoid. Bytes first matches `documents::delete_file`. The
cost: if the row update fails after the bytes are gone, the row still points
at a missing key until the next try. In that window `open_photo` returns 500
(as it does today for missing bytes) and the viewer shows "Couldn't load this
photo". The next delete runs the storage delete again, which succeeds on a
missing key, and then clears the row.

**Retry and idempotency.** OpenDAL documents `delete` as returning Ok for a
path that does not exist, and Loco's `StoreDriver::delete` passes straight
through (`loco-rs-1.2.0/src/storage/drivers/opendal_adapter.rs:66`). This
holds for the memory driver (tests), the local driver (dev) and S3/R2
(prod). A model test proves it on the memory driver by deleting twice. R2
itself is unverified beyond the S3 contract.

**Second delete: 200, not 404.** Picked because the commonest second request
is a retry after a lost response: the phone sent Delete, the photo went, and
the answer never arrived. A 404 would show "Couldn't delete" for something
that worked. The outcome the person asked for holds, so success is the
honest answer. A message that never had a photo is still 404, so the route
does not become a probe that says "yes" to everything.

**Two deletes at once.** No lock is needed:

- Group or DM: both delete the bytes (both succeed), both clear the columns
  (the second writes NULLs over NULLs). Both answer 200.
- #general: both may find the file; both delete the bytes; the second row
  delete affects 0 rows, which SeaORM returns as Ok. If the second loads the
  message after the first finished, step 2 returns early. If it loads the
  file after the row is gone, step 4 carries on.
- Holding a row lock across a network call to R2 would buy nothing here.

**Delete while someone's viewer is streaming.** `open_photo` has already read
the row and opened `download_stream`. An S3 GET in flight normally finishes
after a delete (unverified on R2), so that person may see the photo load,
then the socket event swaps their viewer to "Photo removed" (section 4). Any
request that reads the row after the update gets 404. One that read the row
just before and opens the stream just after gets 500 and "Couldn't load",
then the same swap. No bytes are served after both steps finish.

**Unit test** in the existing `tests` module: `may_delete_photo` for sender,
owner/admin and other member.

### 2. Route and access: `src/controllers/chat.rs`

```rust
/// Deletes a photo from a message, for everyone, for good. Only the sender,
/// and owners and admins who are in the conversation. The message stays and
/// says "Photo removed". A photo that is already gone answers 200.
#[debug_handler]
async fn delete_photo(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    Path((id, message_id)): Path<(i64, i64)>,
) -> Result<Response> {
    member.require_own_membership()?;                                   // acting: 403
    let conversation = conversations::Model::find_for_member(
        &ctx.db, member.org.id, id, member.user.id).await?;             // not in it: 404
    let message = messages::Model::find_in_conversation(
        &ctx.db, &conversation, message_id).await?;                     // elsewhere: 404
    if !message.has_photo {
        return Err(Error::NotFound);                                    // text or reply: 404
    }
    if !messages::may_delete_photo(message.user_id, member.user.id, member.can_manage()) {
        return Err(forbidden(
            "Only the person who sent a photo, owners and admins can delete it.",
        ));                                                             // other member: 403
    }
    let message = message.delete_photo(&ctx).await?;
    // Saved; tell open pages, but a failed broadcast must not undo the answer.
    if let Err(err) = super::chat_ws::publish_photo_removed(&ctx, &message) {
        tracing::error!(error = %err, message_id = message.id, "could not broadcast a removed photo");
    }
    format::render().empty()                                            // 200
}
```

`forbidden` comes from `extractors::current_member`, as in Documents.

**Route,** next to the photo GET in `routes()`:

```rust
.add("/chat/{id}/photos/{message_id}/delete", post(delete_photo))
```

`POST .../delete`, as Documents does; the app has no `DELETE` routes and the
stale-tab walk only knows POST bodies and headers. `chat` is already in
`TENANT_SECTIONS`. No body limit layer (empty body).

**Who gets what,** at `/o/juniper-tea-co/chat/{finance}/photos/{message}/delete`.
#finance is Ana's (owner) group with Maya (admin) and Tom; Leo is a member
who is not in it.

| Who | Stops at | Answer |
|---|---|---|
| Signed out | `CurrentUser` | 303 to `/login` |
| Tom, the sender | | 200, photo gone |
| Ana (owner) or Maya (admin), in #finance | | 200, photo gone |
| Another member in the conversation (Leo on a #general photo of Tom's) | `may_delete_photo` | 403, nothing changed |
| Owner or admin **not** in the group (Ana, if left out of a group) | `find_for_member` | 404 |
| Leo, not in #finance | `find_for_member` | 404 |
| In #finance, a message id from another conversation | `find_in_conversation` | 404 |
| A text message or a thread reply | `has_photo` | 404 |
| A photo already removed, by someone allowed | `delete_photo` step 2 | 200, nothing changed |
| A photo already removed, by someone not allowed | `may_delete_photo` | 403 |
| Pending in Juniper Tea Co | `CurrentMember` (waiting page) | 403 |
| Removed from Juniper Tea Co | `CurrentMember` (removed page) | 403 |
| Member of another org only | `CurrentMember` | 404 |
| Super admin acting in Juniper Tea Co | `require_own_membership` | 403 |
| Tab drawn for another org (`X-Org-Id` mismatch) | `CurrentMember` claim | 409 + `HX-Redirect` |
| Cross-site form or fetch | `origin_check` | 403 |

The pending member gets the waiting page, as on every `/o/{org}/...`
address and on the photo GET (chat-photos-2 Decision 2). The brief allows
"not found or the waiting page".

The access check comes before the "already removed" check, so the 200 for a
repeat never tells a member who could not have deleted it anything new.

### 3. The view: `MessageView` and templates

**`MessageView`** (`chat.rs:209`) gains:

```rust
/// The viewer may delete this photo: the sheet row, and the viewer's Delete.
pub can_delete_photo: bool,
```

`false` in `new`. One setter, used everywhere a viewer is known:

```rust
pub fn allow_photo_delete(&mut self, viewer_id: i64, can_manage: bool) {
    self.can_delete_photo =
        self.photo && messages::may_delete_photo(self.author_id, viewer_id, can_manage);
}
```

Called in: `grouped` (main feed; gains a `can_manage` argument, it already
has `#[allow(clippy::too_many_arguments)]`), `thread_feed_data` (the parent),
`actions`, `send` and `send_photo` (the HTTP answers), and the socket's
`render` (section 4). Replies never have `photo`, so it stays false for them.

**New `assets/views/chat/_photo.html`.** The photo area moves out of
`_message.html:8` into its own partial, so a socket can swap it by id:

```html
{% if message.photo %}{% set photo_url = org_url(path="/chat/" ~ (message.conversation_id) ~ "/photos/" ~ (message.id)) %}<a class="photo-link" id="photo-{{ message.id }}" href="{{ photo_url }}" data-photo-view data-message="{{ message.id }}" data-author="{{ message.author }}" data-color="{{ message.color }}" data-when="{{ message.photo_when }}" data-at="{{ message.at }}" data-caption="{{ message.body }}"{% if message.can_delete_photo %} data-delete-url="{{ photo_url }}/delete"{% endif %} aria-label="Photo from {{ message.author }}. Open full screen"><img class="photo-thumb" src="{{ photo_url }}" alt="" loading="lazy" decoding="async"></a>{% elif message.photo_removed %}<span class="photo-removed" id="photo-{{ message.id }}"{% if oob | default(value=false) %} hx-swap-oob="true"{% endif %}>{{<icon name="image" class="h-4 w-4"/>}}Photo removed</span>{% endif %}
```

- Same markup and classes as today, plus `id`, `data-message`, `data-at` and
  `data-delete-url`. `.photo-link` and `.photo-removed` are unchanged
  (`app.css:613-616`), so no layout moves. No wrapper element.
- The out-of-band swap replaces the `<a id="photo-N">` with the
  `<span id="photo-N">` (outerHTML, as `_thread_link.html` does).
- `data-delete-url` is not a secret; the server checks every request. It only
  decides whether the viewer shows Delete.
- `_message.html:8` becomes `{% include "chat/_photo.html" %}` in that spot.

**`_actions.html`** (§1):

- Quote line 6 becomes: "Photo removed · {caption}" or "Photo removed" when
  `message.photo_removed`; else the body; else "Photo" (mockup §1 decision 5).
- New last block, after Reactions, only when `message.can_delete_photo`:

```html
{% if message.can_delete_photo %}
<div class="list"><button type="button" class="list-row action-row text-danger w-full border-0 bg-transparent text-left" data-photo-delete="{{ message.id }}">{{<icon name="trash" class="h-6 w-6"/>}}<span class="flex-1">Delete photo</span></button></div>
{% endif %}
```

  Mockup classes `list-row action-row text-danger`, plus the button reset the
  Documents Delete row uses (`documents/_files.html:47`). No chevron. A
  removed photo has `photo = false`, so the row is gone. On the thread page
  (`?thread=1`) the parent's sheet gets the row too; replies never do.

**New `assets/views/chat/_photo_delete.html`,** included in `show.html` and
`thread.html` after `_photo_viewer.html`. The shared confirm (§3):

```html
<dialog id="photo-delete" class="sheet" aria-labelledby="photo-delete-title">
  <div class="sheet-handle"></div>
  <div class="sheet-body">
    <div class="flex flex-col gap-4">
      <h2 id="photo-delete-title" class="m-0 text-[19px] font-bold">Delete this photo?</h2>
      <div class="quote">
        <div class="quote-head"><span data-photo-delete-avatar></span><span data-photo-delete-name></span><span class="meta" data-photo-delete-at></span></div>
        <p data-photo-delete-text></p>
      </div>
      <p class="m-0 text-[15px] text-muted">{% if conversation.kind == "dm" %}It's removed for you and {{ conversation.label }}.{% else %}It's removed for everyone in #{{ conversation.label }}.{% endif %}{% if conversation.kind == "channel" %} It's also deleted from Documents.{% endif %} This can't be undone.</p>
      <div class="alert alert-error" role="alert" data-photo-delete-alert hidden>{{<icon name="circle-alert" class="h-[18px] w-[18px] shrink-0"/>}}<span>Couldn't delete the photo. Check your connection and try again.</span></div>
      <div class="flex flex-col gap-2">
        <button type="button" class="btn btn-lg btn-danger-solid w-full" data-photo-delete-confirm><span data-photo-delete-spin hidden>{{<icon name="loader-circle" class="spin h-[18px] w-[18px]"/>}}</span><span data-photo-delete-label>Delete</span></button>
        <button type="button" class="btn btn-lg btn-secondary w-full" data-dialog-close>Cancel</button>
      </div>
    </div>
  </div>
</dialog>
```

- The sentence depends only on the conversation, so the server writes it.
  The quote depends on the message, so `app.ts` fills it with `textContent`.
- `conversation.label` is the other person's name in a DM, for this viewer
  (`label`, `chat.rs:423`). An owner or admin deleting in a DM is one of its
  two people, so "you and {name}" is always right.
- `kind == "channel"` is #general: the only channel.
- `.quote`, `.quote-head`, `.alert-error`, `.btn-danger-solid`,
  `.btn-secondary` all exist. The mockup's `.confirm`, `.lead2` and `.btns`
  rules are mockup-only (not in `app.css`); this uses the same sizes as
  utilities, as Documents' delete pane does (`documents/_files.html:60-64`).

**`_photo_viewer.html`** (§2):

- After Download:

```html
<button type="button" class="viewer-dl viewer-del" data-viewer-delete aria-label="Delete photo" hidden>{{<icon name="trash" class="h-[22px] w-[22px]"/>}}<span class="viewer-dl-label">Delete</span></button>
```

  Icon button on a phone, labelled outline button from 900px, exactly like
  Download.
- In the stage, the removed state from the mockup:

```html
<div class="viewer-state" data-viewer-removed role="status" hidden>{{<icon name="image" class="h-8 w-8"/>}}<strong>Photo removed</strong><span>It's no longer in this chat.</span><button type="button" class="viewer-btn" data-viewer-close>Close</button></div>
```

### 4. Live update: `src/data/chat_hub.rs` and `src/controllers/chat_ws.rs`

**New event:**

```rust
/// A message's photo was deleted. Sockets on the main feed, and on that
/// message's thread page (where it is the parent), swap the photo for
/// "Photo removed". Replies never carry photos.
PhotoRemoved { conversation_id: i64, message_id: i64 },
```

**`publish_photo_removed(ctx, message) -> Result<()>`,** next to
`publish_reactions`: no query, just the ids.

**`wants()`:** add `PhotoRemoved` to the conversation-id match, then
`(PhotoRemoved, None) => true` and
`(PhotoRemoved { message_id, .. }, Some(thread)) => *message_id == thread`.
The conversation check still runs first, so no other conversation's or
org's socket renders anything.

**`run` loop:** on `PhotoRemoved { message_id, .. }`, render
`chat/_photo.html` with
`{ "message": { "id": message_id, "photo": false, "photo_removed": true }, "oob": true }`
through `session.fragment`, and send it. The same for every viewer, so no
query and no per-viewer data.

**`Session`** gains `can_manage: bool`, set in `connect` from
`member.can_manage()`. `render` calls
`message.allow_photo_delete(session.user_id, session.can_manage)` after it
sets `own`. A role change while a socket is open shows a stale Delete until
the next reconnect or reload; the route checks the role on every request, so
nothing unsafe follows.

**A page that missed the event** (socket down, phone asleep) reloads the feed
on reconnect (`app.ts:276-282`, existing). The feed is drawn from the
database, so it says "Photo removed".

**Chat list and Home.** Neither updates live today; this PR does not change
that. `last_line` (`chat.rs:486`) becomes:

| Message | Today | New |
|---|---|---|
| tom, photo and caption | `tom: Photo · Van fuel £62` | unchanged |
| tom, removed, with caption | `tom: Photo · Van fuel £62` | `tom: Photo removed · Van fuel £62` |
| you, removed, no caption | `You: Photo` | `You: Photo removed` |
| leo in your DM, removed, no caption | `Photo` | `Photo removed` |
| A #general photo deleted in Documents | `maya: Photo · Whiteboard` | `maya: Photo removed · Whiteboard` |

The match uses `message.photo_removed()` first, then `has_photo`. The
80-character cut on the body stays. Home's chat card
(`dashboard/index.html:109`) shows the same string.

### 5. Documents deletes tell chat too (`src/controllers/documents.rs`)

Today, deleting a #general photo in Documents leaves open chat pages showing
a broken thumbnail until reload. With the event in place this is a few
lines (Decision 2):

- `messages::Model::for_document(db, org_id, file_id) -> ModelResult<Option<Self>>`
  (uses the existing partial index `idx-messages-document`).
- In `delete_file`, read it before `file.delete_file`, then after the delete
  call `chat_ws::publish_photo_removed` for it, logging a broadcast error as
  section 2 does.

No change to who may delete in Documents.

### 6. `frontend/app.ts`

All new DOM text is set with `textContent`; the avatar is built with
`createElement`, as `viewerAvatar` does. No `innerHTML` with any `data-*`
value (chat-photos-2 amendment S3).

**The global backdrop rule** (line 32) skips a dialog with `data-busy`, so
the confirm can't be dismissed while deleting (§3 decision 6). The confirm's
`cancel` event (Escape) calls `preventDefault()` while busy.

**`viewerAvatar(name, color, size = "sm")`** gains the size, so the confirm's
quote can use `xs`.

**Opening the confirm.** `openPhotoDelete(link, from: "sheet" | "viewer")`:

1. Read the link's data: author, colour, `data-at`, caption,
   `data-delete-url`, `data-message`. Name is "You" when the link is inside
   `.msg-own`.
2. Fill the quote: avatar, name, time, "Photo · {caption}" or "Photo"
   (mockup's `confirmBody`).
3. Hide the alert, reset the button (label "Delete", spinner hidden,
   enabled), store the URL and message id on the dialog.
4. From the sheet: close `#chat-dialog` first, so only one sheet shows. From
   the viewer: leave it open underneath; the confirm opens over it in the
   app's theme, and Cancel or Escape returns to the photo (§3 decision 5).
5. `showModal()`.

**The two doors.**

- Sheet row: a click on `[data-photo-delete]` finds
  `#photo-{id}` as `a[data-photo-view]` in the page. If it is not there
  (removed while the sheet was open), close the sheet and toast "That photo
  was already removed." Otherwise `openPhotoDelete(link, "sheet")`.
- Viewer: `openViewer` stores `data-message` and `data-delete-url` on the
  dialog and keeps `[data-viewer-delete]` hidden. The image `load` handler
  shows it when there is a delete URL. Loading, error and removed states
  hide it (§2 decision 4). A click on it calls
  `openPhotoDelete(link, "viewer")` with the feed's link for that message.

**Deleting.** A click on `[data-photo-delete-confirm]`:

1. Busy: `data-busy` on the dialog, Delete disabled with the spinner and
   "Deleting...", Cancel disabled. A second click does nothing.
2. `fetch(url, { method: "POST", headers: { "HX-Request": "true", ...orgHeaders() }, credentials: "same-origin" })`.
3. `staleRefusal(res)`: un-busy, close the confirm, stop. The stale-tab
   banner or move takes over, as for photo sends.
4. `res.ok`: un-busy, close the confirm, close the viewer if it shows this
   message (its `close` handler drops the history entry as today), and
   `showToast({ kind: "success", message: "Photo deleted" })`. The socket
   event swaps the bubble. Fallback: after 1.5 s, if `#photo-{id}` is still
   a link, reload `#chat-feed` from `data-chat-feed-url`, as
   `sendStagedPhoto` does.
5. Anything else, or a network error: un-busy, show the alert. Its text is
   the 403's `description` when there is one (the `refusalText` shape),
   else the mockup's "Couldn't delete the photo. Check your connection and
   try again." The photo stays; Delete is the retry. No toast (§3 decision 7).

**The open viewer on other screens.** `syncViewer()`: if the viewer is open
on message N, `#message-N` is on the page, and `#photo-N` is no longer
`a[data-photo-view]`, switch to the removed state: clear the `img` `src`,
hide img, loading and error, hide Download and Delete, show
`[data-viewer-removed]`. The caption stays (§2 decisions 5 and 6). It runs on
`htmx:wsAfterMessage` and on `htmx:afterSwap` of `#chat-feed` (the reconnect
reload). While this tab is deleting message N itself, `syncViewer` skips N,
so the deleter's viewer closes rather than flashing the removed state when
the event beats the answer.

**Back.** The existing `popstate` handler closes the viewer. If the confirm
was opened from the viewer and is not busy, the viewer's `close` handler
closes it too. While busy it stays; the answer then closes it.

**Removed state close.** `[data-viewer-close]` already closes the viewer,
so the new Close button needs no new code.

### 7. CSS: `frontend/styles/app.css`

- Base `.viewer-dl` (line 628) gains `border: 0; background: transparent;
  font: inherit; cursor: pointer;`, so a `<button>` looks like the Download
  link. The 900px rule (line 644) already adds the outline after it.
- From the mockup: `.viewer .viewer-del { color: #ff8466; }` and
  `.viewer .viewer-del:hover { background: rgba(255, 132, 102, 0.12); }`.
  Always coral: the viewer is dark in both themes (§2 decision 2).
- Nothing else. `.text-danger` on `.action-row` must beat `.action-row`'s
  `color: var(--text)` (line 580). Utilities come after components in the
  build, so it should; checked on the light and dark sheet screenshots.

Then `bin/build-assets`.

## Edge cases

- **Photo with no caption.** The bubble keeps "Photo removed" and the time,
  so the thread and reactions keep their home (§4 decision 3).
- **A thread under the photo.** The thread page still opens; its parent
  shows "Photo removed" live. Replies are untouched.
- **Old #general photos moved out of Chat photos.** Same file, still
  deleted (brief recommends yes). The confirm already says "It's also
  deleted from Documents."
- **Sheet open on another screen when the photo goes.** Its Delete row is
  stale. A click finds no link and says "That photo was already removed."
- **Removed from the conversation with the confirm open.** Delete answers
  404; the alert shows the generic text. The next reload shows the 404 page.
- **The photo was downloaded or is still in someone's image memory.** Out of
  our hands. The route sends `no-store`, so browsers keep no disk copy.
- **Two tabs, same person.** Both get the socket event. The tab that did not
  delete shows the removed state if its viewer was open.
- **The 200 history limit.** A photo older than the last 200 messages is not
  on the page, so its swap finds no target. htmx logs `oobErrorNoTarget` and
  nothing breaks.

## Tests

Run as usual (`cargo test`, Postgres, `#[serial]`). Storage in tests is the
memory driver (`src/app.rs:135`), so "the bytes are gone" is
`ctx.storage.download::<Vec<u8>>(Path::new(&key)).await.is_err()` plus the
existing `stored_chat_files` listing being empty.

Names are made up: Juniper Tea Co, with Ana (owner), Maya (admin), Tom and
Leo (members). A helper `juniper(&request, &ctx) -> Team` signs Ana up with
`sign_up_member`, joins and approves the others with `join` and
`approve_as_owner`, makes Maya an admin by setting `role` on her membership
(as `bare_addresses.rs:213` does), and creates #finance (Ana, Maya, Tom) with
`conversations::Model::create_group`. Photos use `PNG` from
`tests/requests/documents.rs`; group photos use `secret_png()` so a leak
shows. Captions: "Van fuel £62", "Courier invoice from Tuesday, £18.40".

### `tests/models/messages.rs` (model)

- `deleting_a_group_photo_removes_the_bytes_then_the_columns`: after
  `delete_photo`, the key downloads as an error, the four columns are
  `None`, `has_photo` is true, `document_id` is `None`, the body is kept.
  The row was saved, so the CHECK accepted it.
- `deleting_a_general_photo_deletes_its_documents_file`: the `documents` row
  is gone, its `storage_key` downloads as an error, the message's
  `document_id` is `None`.
- `deleting_twice_is_a_no_op`: the second call returns Ok and the same
  columns, for both kinds. Also proves the memory driver's delete of a
  missing key is Ok.
- `two_deletes_at_once_both_succeed`: `tokio::join!` two `delete_photo`
  calls on clones, for both kinds. Both Ok; one end state.
- `a_text_message_has_no_photo_to_delete`: `EntityNotFound`; nothing else
  touched.

A failing storage delete can't be made with the memory driver. That branch
(error returned, row untouched) is covered by reading the code only.

### New `tests/requests/chat_photo_delete.rs`

Registered in `tests/requests/mod.rs`. Each test subscribes to `ChatHub`
before acting where it checks the event, as `chat_photos.rs` does.

- `the_sender_deletes_a_group_photo_for_everyone`. Tom posts "Van fuel £62"
  with a photo in #finance. Tom POSTs delete with `X-Org-Id`: 200. Bytes gone
  and nothing left under `orgs/{org}/chat/`. One `PhotoRemoved` event with
  #finance's id and the message's id. The photo GET is 404 for Tom and Maya.
  Maya's `/chat/{finance}` has "Photo removed" and "Van fuel £62", no
  `/photos/{id}` link and no `SECRET`.
- `owners_and_admins_in_the_conversation_delete_anyones_photo`. Maya deletes
  Tom's #finance photo: 200. Leo sends Ana a DM photo; Ana deletes it: 200.
- `a_member_cannot_delete_someone_elses_photo`. Tom posts in #general. Leo
  POSTs delete: 403 with the refusal text; bytes and Documents row still
  there; no event. Leo's `/chat/{general}` has no `data-delete-url`; Leo's
  sheet (`/messages/{id}/actions`) has no "Delete photo". Tom's, Maya's and
  Ana's sheets have it; Tom's feed link has `data-delete-url`.
- `people_outside_the_conversation_cannot_delete`. With a group #suppliers of
  Tom and Leo only, and a photo in it:
  - Ana (owner, not in it) and Maya (admin, not in it): 404. Role does not
    matter.
  - A #finance message id under #suppliers' address, and the other way
    round: 404.
  - A text message and a thread reply under the right address: 404.
  - Signed out: 303 to `/login`.
  - A pending member: 403 waiting page.
  - A removed member: 403.
  - Super admin acting in Juniper Tea Co: 403.
  - A cross-site POST (`Origin: https://evil.example`): 403.
  - After all of these, the photo still opens for Tom and its bytes exist.
- `deleting_a_general_photo_deletes_the_documents_file`. Tom posts in
  #general; Maya deletes it from chat. The Documents row is gone, its bytes
  are gone, `/documents/files/{id}` is 404, the Chat photos folder page
  does not show the name, and #general says "Photo removed". Variant: the
  file was first moved to another folder (set `folder_id`); still deleted.
- `a_second_delete_succeeds_and_changes_nothing`. Two POSTs: both 200. The
  `updated_at` and columns after the second equal those after the first.
- `the_list_home_and_sheet_say_photo_removed`. After Tom's captioned photo
  is deleted: Maya's chat list and Home show
  `tom: Photo removed · Van fuel £62`; Tom's show
  `You: Photo removed · Van fuel £62`. With no caption, `You: Photo removed`.
  The sheet quote is `<p>Photo removed · Van fuel £62</p>` and has no
  "Delete photo".
- `the_confirm_names_where_the_photo_goes`. `/chat/{finance}` has
  `id="photo-delete"` and "It's removed for everyone in #finance. This can't
  be undone." Leo's DM with Ana says "It's removed for you and ana."
  `/chat/{general}` adds "It's also deleted from Documents."
- `a_thread_under_a_removed_photo_still_opens`. Maya replies in the photo's
  thread; Tom deletes the photo. The thread page is 200, has the reply, the
  parent says "Photo removed", and includes `#photo-delete`. Before the
  delete, the parent's sheet with `?thread=1` has "Delete photo".
- `a_documents_delete_tells_chat`. Tom deletes his #general photo's file in
  Documents: one `PhotoRemoved` event (Decision 2).

### `tests/requests/tenancy.rs`

- `org_a_cannot_reach_org_b`: add to `posts`
  `/chat/{globex.group}/photos/{group_photo}/delete` and
  `/chat/{globex.general}/photos/{globex.message}/delete`: 404. The existing
  closing asserts already check the group photo's bytes and Globex's file
  still exist.
- `documents_and_removal_need_your_own_active_membership`: add the group
  photo delete to `posts`. Waiting, removed and acting get 403 and no
  `SECRET`.

### `tests/requests/stale_tab.rs`

No edit. The route walk (line 89) picks up the new POST, so the header,
form and no-claim refusals all run against it.

### Unit tests

- `chat.rs` `list_lines_say_photo_then_the_caption`: the `message` fixture
  today has `has_photo` and no file, which `photo_removed()` now reads as
  removed. Live photos get `document_id: Some(9)`; add the removed rows from
  the table above, and keep the 80-character cut.
- `chat_ws.rs`: `PhotoRemoved` joins `every_event`. Wanted by the main feed
  and by the message's own thread page; not by another thread; never by
  another conversation.
- `chat_ws.rs`: `render` sets `can_delete_photo` for the sender and for a
  manager, not for another member (call `allow_photo_delete` on a fixture
  view).
- `messages.rs`: `may_delete_photo`.

### Source check

The codebase has the pattern (`roadmap_done.rs:320-346`, `links.rs:620`).
Add `photo_bytes_are_only_deleted_by_the_models` to
`chat_photo_delete.rs`: walking `src/`, `storage.delete(` appears only in
`src/models/messages.rs` and `src/models/documents.rs`. That keeps the
bytes-first rule in one place. No source test on `app.ts`: the last review
dropped a brittle one.

### Not covered by `cargo test`

No JS runner. The confirm's busy lock, Escape and backdrop while busy, the
viewer's Delete and removed state, Back with both dialogs open, the toast
and the fallback reload are checked by hand (see "Checks").

## Commits

One PR. Each commit builds and passes `cargo test`.

1. **Messages can lose their photo.** `may_delete_photo`, `delete_photo`,
   model tests.
2. **A route to delete a chat photo, and a live event.** `delete_photo`
   handler and route, `ChatEvent::PhotoRemoved`, `publish_photo_removed`,
   `wants`, the socket render, `Session.can_manage`, `can_delete_photo` on
   `MessageView`, `_photo.html`, `last_line`, the `_actions.html` quote.
   Request, tenancy and unit tests. Nothing in the UI calls it yet.
3. **Deleting in Documents tells chat.** `for_document`, the publish in
   `documents::delete_file`, its test.
4. **Delete photo in the sheet and the viewer.** The sheet row,
   `_photo_delete.html`, the viewer's Delete and removed state, `app.ts`,
   CSS, `bin/build-assets`. Template tests (sheet row, confirm wording).

## No migration

Checked against `m20261025_120100_add_private_photos_to_messages.rs:23-30`.
A cleared group or DM photo has all four columns NULL, which is the CHECK's
first branch whatever `has_photo` and `document_id` are. A #general photo
loses `document_id` through the existing `ON DELETE SET NULL`. The PR body
says "No migration."

## Out of scope

As the brief: deleting or editing text, captions or replies; deleting a
whole message; undo, a bin or a grace period; telling the sender or showing
who removed it; an audit log; long-press menus; anything through the Claude
connector (`src/mcp/mod.rs:6` already says no tool reads chat). Also: making
the chat list or Home update live; a notice to the sender.

## Risks

- **Privacy and access.** A new write route on private files. The gate is
  the same as `open_photo` (`require_own_membership`, `find_for_member`,
  `find_in_conversation`) plus `may_delete_photo`. Tenancy and per-role
  tests cover each outsider. The security reviewer must review this plan and
  the diff.
- **Permanent by design.** The confirm is the only guard. It names the place
  and, for #general, Documents.
- **R2 really removes the bytes.** Assumes bucket versioning or lifecycle
  copies are off. Unverified; Raj to confirm in the Cloudflare dashboard
  before merge.
- **An in-flight stream after a delete.** Whether R2 cuts a GET already in
  progress is unverified. Either way the viewer ends in "Photo removed".
- **The 500 window** between bytes gone and row cleared, if the update
  fails. Logged, and closed by any retry.
- **Stale Delete after a role change** on an open socket. Display only; the
  route checks the role.
- **`.text-danger` against `.action-row`.** Depends on CSS order in the
  build; caught by the sheet screenshots if wrong.
- **Two modal dialogs at once** (viewer, then confirm). Native top-layer
  stacking; checked by hand on iOS Safari and Android Chrome.

## Rollback

Revert the PR. No migration to undo. Old code reads a cleared message as
`has_photo` with no file, which it already shows as "Photo removed" with the
caption. Deleted photos stay deleted; nothing private comes back.

## Checks

```sh
bin/build-assets
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

By hand, locally, with realistic data: Juniper Tea Co with Ana (owner),
Maya (admin), Tom and Leo; #finance (Ana, Maya, Tom); a DM Leo and Ana;
#general; a pending member; Ana also in a second org. Photos: a fuel
receipt, a courier invoice, a tea tin, a market stall. The "wrong photo" in
#finance is a neutral one (a blurry photo of the van's dashboard), not the
mockup's passport.

On a phone-sized window (390px) and desktop (1440px), light and dark:

1. Tom taps his #finance photo's bubble: "Delete photo" in red at the
   bottom, no chevron. Leo's sheet on Tom's #general photo has no row.
2. Tom taps it: the sheet closes, the confirm opens with his quote and
   "It's removed for everyone in #finance. This can't be undone."
3. Cancel: nothing changed. Again, Delete: spinner and "Deleting...", both
   buttons locked, Escape and a backdrop tap do nothing. Then the toast
   "Photo deleted"; his bubble keeps "Van fuel £62" with "Photo removed".
4. Maya has #finance open on her phone: her copy changes without a reload.
   With the photo open full screen, her viewer shows "Photo removed" and
   Close; Download and Delete are gone; the caption stays.
5. DevTools offline, delete another photo: the red alert in the confirm, the
   photo stays. Online, Delete again: it goes once.
6. Leo opens his DM photo full screen: Delete next to Download (icon on the
   phone, labelled on desktop, coral). Not shown while loading or on
   "Couldn't load". Delete: the confirm opens over the viewer, "It's removed
   for you and ana." Cancel returns to the photo. Delete: the viewer closes,
   the toast shows, Back then leaves the chat as normal (no extra history
   entry).
7. With the confirm open over the viewer, the browser's Back: both close.
8. Maya deletes Tom's #general photo: the confirm adds "It's also deleted
   from Documents." Documents, Chat photos no longer lists it.
9. Tom deletes a #general photo's file in Documents with #general open in
   another tab: that tab shows "Photo removed" live.
10. The chat list and Home: "tom: Photo removed · Van fuel £62", "You: Photo
    removed".
11. The thread under a removed photo opens with all its replies.
12. iPhone back swipe and Android back button with the viewer open after a
    delete elsewhere.

### Screenshots needed

On the `pr-screenshots` branch under `chat-photo-delete/`, linked from the
PR body by
`https://raw.githubusercontent.com/rajgurung/collaboration_tool/pr-screenshots/chat-photo-delete/<file>.png`.
From the local app with the data above, not test leftovers. Phone 390px and
desktop 1440px, light and dark as listed.

- Sheet with "Delete photo": Tom in #finance, phone light and dark, desktop
  light. Leo's sheet with no row, phone light.
- Viewer with Delete: Leo's DM photo, phone light (the viewer is always
  dark); Tom's #finance photo, desktop.
- Confirm: #finance from the sheet (phone light, phone dark); #general as
  Maya with the Documents line (phone dark, desktop light); DM over the
  viewer (phone, desktop dark); Deleting (phone); Couldn't delete (phone).
- After: Tom's bubble with the "Photo deleted" toast (phone light, desktop
  light and dark); Maya's live copy (phone); Ana's viewer in the removed
  state (phone).
- Chat list with "Photo removed · Van fuel £62" (phone light and dark); Home
  chat card (desktop light).

The PR body also says "No migration", and how the access rule was tested
(the test names above and their output).

## Decisions for Raj

Each with a recommendation.

1. **A second delete answers 200.** Recommend: yes, for retries after a lost
   answer. Never-a-photo stays 404.
2. **Deleting a #general photo in Documents also updates open chats live.**
   Not in the brief; a few lines once the event exists, and it fixes today's
   broken thumbnail. Recommend: yes, in this PR.
3. **The deleter's viewer closes after the delete,** with the toast; other
   people's open viewers show the removed state. Matches §2 decision 5 and
   §4 decision 4. Recommend: yes.
4. **New copy not in the mockup:** "That photo was already removed." (toast)
   for a stale sheet row. Recommend: yes.
5. **Chat list and Home are not live,** as today. Recommend: leave for now.

## Where this differs from the brief or mockup

- The confirm uses utility classes instead of the mockup's `.confirm`,
  `.lead2` and `.btns`, which exist only in the mockup. Same sizes.
- The "Photo deleted" toast is shown by `app.ts` on the 200, not by an
  `HX-Trigger` header, because the delete is a raw `fetch`.
- A pending member gets the waiting page (403), as the brief allows.
- The passport example stays in the mockup only. No code, test or
  screenshot uses it.
