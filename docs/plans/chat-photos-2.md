# Chat photos 2: captions, a viewer, and photos in every conversation

Status: Draft, for staff and security review · One PR · Owner: Raj · 25 October 2026

Brief: `docs/briefs/chat-photos-2.md`. Mockup: `docs/designs/chat-photos-2.html`
(sections §1 to §5 below are the mockup's; its states are approved, names
are being touched up). Visuals and copy are the mockup's. This plan covers
what the server, the templates and `app.ts` must do.

## Summary

Three changes, one PR.

1. **Photos everywhere.** The photo button shows in #general, every group and
   every DM. Thread replies stay text only.
2. **Caption first, then send.** Picking a photo shows a tile in the composer.
   The text box becomes an optional caption. Send posts both in one request.
   A failed send keeps both.
3. **A viewer in the same tab.** Tapping a photo opens a full-screen viewer.
   Back, Escape, X or a tap on the dark area closes it. Download saves the
   original.

**Storage: group and DM photos live on the message, not in Documents.**
Four new nullable columns on `messages` hold the stored file's key, type,
size and name. The bytes go to `ctx.storage` under
`orgs/{org}/chat/{conversation}/{uuid}`. No `documents` row, no folder, so
they never show in Documents and `/documents/files/{id}` can't reach them.

**#general keeps the "Chat photos" folder.** The brief wants it to (Done
when: "A #general photo still shows in Documents"), moving old photos is out
of scope, and every approved member is in #general, so nothing new is
exposed.

**One route serves every chat photo:**
`GET /o/{org}/chat/{conversation}/photos/{message}`. It checks conversation
membership on every request, then streams either the message's own file
(groups, DMs) or its Documents file (#general). Both use the same headers as
`open_file`. So the templates, the viewer and the tests have one URL shape
and one access rule. Two storage homes stay behind it, because the brief
fixes #general's home.

Why not move #general to the new storage too, for one storage path? It would
break the "shows in Documents" promise, and old #general photos would still
need the Documents branch. Why not keep #general on `/documents/files/{id}`
and only route groups through the new path? Then the feed has two link
shapes and two sets of tests. One route with a two-arm `match` is the
smallest safe option.

One migration. It adds columns only and rolls back cleanly (see
"Migration"). No new crate.

## What changes, file by file

### 1. Migration and entity

**New `migration/src/m20261025_120000_add_private_photos_to_messages.rs`**,
registered in `migration/src/lib.rs` after the Claude grants migration.

```sql
SET LOCAL lock_timeout = '5s';
ALTER TABLE messages
  ADD COLUMN photo_key  text   NULL,
  ADD COLUMN photo_type text   NULL,
  ADD COLUMN photo_size bigint NULL,
  ADD COLUMN photo_name text   NULL,
  ADD CONSTRAINT "messages-own-photo" CHECK (
    (photo_key IS NULL AND photo_type IS NULL AND photo_size IS NULL AND photo_name IS NULL)
    OR (photo_key IS NOT NULL AND photo_size > 0 AND photo_name IS NOT NULL
        AND photo_type IN ('image/jpeg', 'image/png', 'image/webp')
        AND has_photo AND document_id IS NULL)
  );
```

What it does, in plain words:

- A message can now carry its own photo file, outside Documents.
- The four columns are all set or all empty.
- A message never has both its own file and a Documents file.
- The stored type is always one of the three photo types, so the route can
  never be made to send `text/html` or SVG, even by a bad row.
- `has_photo` stays the flag for "this message had a photo". The existing
  "Photo removed" rule (`has_photo` and no `document_id`) is changed in code
  to also look at `photo_key` (section 3).

`ADD CONSTRAINT ... CHECK` scans `messages` under its lock. The table is
small (four people), so this takes milliseconds. `lock_timeout` stops it
waiting on a busy table; the deploy then fails and is retried.

**Down:** `ALTER TABLE messages DROP CONSTRAINT "messages-own-photo", DROP
COLUMN photo_key, DROP COLUMN photo_type, DROP COLUMN photo_size, DROP
COLUMN photo_name;`. See "Rollback" for what happens to stored files.

**Entity.** Run `cargo loco db migrate` then `cargo loco db entities`
(generate, then edit: AGENTS.md rule 1). `src/models/_entities/messages.rs`
gains the four `Option` fields. The one hand-built `messages::Model` in
`src/controllers/chat_ws.rs` tests (`fn message`, around line 704) gets
`None` for each.

### 2. Shared file serving (no behaviour change)

`open_file` builds its headers inline (`src/controllers/documents.rs:481-524`).
The photo route needs the same headers, so lift them out once.

**`src/models/documents.rs`:**

- `pub fn disposition_for(name: &str, content_type: &str, download: bool) -> String`
  and `pub fn content_security_policy_for(content_type: &str) -> &'static str`,
  the bodies of today's methods. `Model::disposition` and
  `Model::content_security_policy` call them. `opens_inline` becomes
  `pub fn opens_inline_type(content_type)` for the same reason.
- `pub fn photo_name(file_name: &str, content_type: &str) -> String`: the
  stem plus extension logic from `upload` (`full_name` and `clean`), so a
  chat photo's download name is cleaned the same way ("photo\u{202E}gpj"
  still becomes "photogpj.png").
- `pub fn new_key(org_id, area: &str) -> String`: `orgs/{org}/{area}/{uuid}`.
  `upload` uses it with `docs`.

**`src/controllers/documents.rs`:** new
`pub async fn send_stored(ctx, key, content_type, size, name, download) -> Result<Response>`.
It opens `ctx.storage.download_stream`, logs and returns 500 on a storage
error, and sets exactly today's headers:

- `Content-Type`, `Content-Length`, `Content-Disposition` (inline unless
  `?download`)
- `X-Content-Type-Options: nosniff`
- `Cache-Control: private, no-store`
- `Cross-Origin-Resource-Policy: same-origin`
- `Content-Security-Policy` from `content_security_policy_for` (images:
  `default-src 'none'; ... sandbox; frame-ancestors 'none'`)
- **plus `X-Frame-Options: DENY`**. Documents routes get this from the
  `no_framing` layer; chat routes have no such layer, so the helper sets it
  for both.

`open_file` keeps its access checks and calls `send_stored`. The existing
Documents tests prove nothing changed.

### 3. The model: `src/models/messages.rs`

**`create_photo`** loses the channel-only refusal. Same checks in the same
order as today: trimmed caption up to 1,000 characters, not empty, at most
25 MB, sniffed type in `PHOTO_TYPES`. Then:

- `kind::CHANNEL` (#general): today's path. `folders::chat_photos`, then
  `documents::Model::upload`, then the message with `document_id`.
- `kind::GROUP` or `kind::DM`: key `orgs/{org}/chat/{conversation}/{uuid}`
  via `documents::new_key`. One `ctx.storage.upload`. Then insert the
  message with `has_photo = true`, `photo_key`, `photo_type` (the sniffed
  type, never the browser's), `photo_size`, `photo_name`. If the insert
  fails, delete the stored bytes and log on failure, as both paths do today.
- Any other kind: `ModelError::msg(PHOTOS_NOT_HERE)` ("Photos can't be posted
  here."). Defensive only; there are three kinds.

`CHANNELS_ONLY` goes. The doc comment says where each kind's photo lives and
why.

**New, on `Model`:**

```rust
pub enum PhotoFile<'a> {
    Own { key: &'a str, content_type: &'a str, size: i64, name: &'a str },
    Document(i64),
}
/// Where this message's photo is stored, if it still has one.
pub fn photo_file(&self) -> Option<PhotoFile<'_>>
/// The photo was posted and is gone (deleted from Documents).
pub fn photo_removed(&self) -> bool { self.has_photo && self.document_id.is_none() && self.photo_key.is_none() }
```

`photo_only` is unchanged.

Thread replies: `create_reply` has no photo path and none is added. The
photo POST always makes a top-level message.

### 4. Routes and access: `src/controllers/chat.rs`

**`send_photo` (`POST /chat/{id}/photos`).** Drop the
`kind != CHANNEL` 403 (line 732) and update the doc comment. Keep, in order:
`require_own_membership` (403 for an acting super admin),
`find_for_member` (404 for anyone not in the conversation), `read_upload`,
`create_photo`, publish, notify, render. Same 422 refusals.

**New `open_photo` (`GET /chat/{id}/photos/{message_id}`):**

```rust
member.require_own_membership()?;                                   // acting: 403
let conversation = conversations::Model::find_for_member(
    &ctx.db, member.org.id, id, member.user.id).await?;             // not in it: 404
let message = messages::Model::find_in_conversation(
    &ctx.db, &conversation, message_id).await?;                     // other conversation: 404
match message.photo_file().ok_or(Error::NotFound)? {
    PhotoFile::Own { key, content_type, size, name } =>
        documents::send_stored(&ctx, key, content_type, size, name, download).await,
    PhotoFile::Document(file_id) => {
        let file = documents::Model::find_in_org(&ctx.db, member.org.id, file_id).await?;
        documents::send_stored(&ctx, &file.storage_key, &file.content_type,
                               file.size_bytes, &file.name, download).await
    }
}
```

Who gets what, at `/o/acme/chat/{group}/photos/{message}`:

| Who | Stops at | Answer |
|---|---|---|
| Signed out | `CurrentUser` | 303 to `/login` |
| Member of Acme in the group | | 200, the bytes |
| Member of Acme not in the group (owner or admin too) | `find_for_member` | 404 |
| In the group, asking for a message id from another conversation | `find_in_conversation` | 404 |
| A message with no photo, a reply, or a deleted #general file | `photo_file` / `find_in_org` | 404 |
| Removed from Acme | `CurrentMember` (removed page) | 403, no bytes. Their `conversation_members` rows are gone too (`remove_everywhere`, `memberships.rs:801`). |
| Removed, then approved again | `find_for_member` | 404 for group photos (groups are not restored, `memberships.rs:655`). 200 for DM and #general photos, which come back with the DM. |
| Pending in Acme | `CurrentMember` (waiting page) | 403, no bytes. See Decision 2. |
| Member of another org only | `CurrentMember` | 404, the plain not-found page |
| Super admin acting in Acme | `require_own_membership` | 403, as `open_file` |

Every check is a database read per request. Nothing is cached, so removing
someone from a conversation takes effect on their next request.

The message id in the address is not a secret. The membership check is the
only gate, and the storage key never leaves the server.

**Routes.** In `routes()`, next to the POST:

```rust
.add("/chat/{id}/photos", photos)                       // POST, body limit, as today
.add("/chat/{id}/photos/{message_id}", get(open_photo))
```

`chat` is already in `TENANT_SECTIONS`, so `/o/{slug}/chat/...` strips as
usual.

**The Claude connector.** `src/mcp/` reads no messages or documents today.
This PR adds nothing there. A source test (section "Tests") fails if
`src/mcp` mentions `photo_key` or `open_photo`.

### 5. What the feed and lists show

**`MessageView`** (`chat.rs:200-284`):

- `photo: Option<i64>` becomes `photo: bool` ("there is a file to show"):
  `message.photo_file().is_some()`. The template builds the URL from
  `conversation_id` and `id`, so no file id reaches the page or the socket.
- `photo_removed` uses `message.photo_removed()`.
- New `photo_when: String`, "Today 09:40", "Yesterday 09:40", "Mon 09:40"
  or "3 Oct 09:40" in the org's zone, for the viewer. New
  `fn photo_when(at, tz)` next to `reply_when`, with unit tests.

The socket carries the same view (`chat_ws::publish`), so live messages get
it too.

**Chat list and Home** (`list_items`, `chat.rs:487-497`). A photo message
reads "Photo", plus " · caption" when there is one. The name rule stays:

| Message | Today | New |
|---|---|---|
| maya, photo and caption, in a group | `maya: Van fuel £62` | `maya: Photo · Van fuel £62` |
| maya, photo only, in a group | `maya sent a photo` | `maya: Photo` |
| You, photo only | `You sent a photo` | `You: Photo` |
| tom, photo and caption, in your DM | `Is this the right label?` | `Photo · Is this the right label?` |
| tom, photo only, in your DM | `Sent a photo` | `Photo` |
| A removed #general photo with a caption | caption | `Photo · caption` |

The caption part is cut so the whole line stays 80 characters, as today.
Home's chat card (`dashboard/index.html:108`) uses the same string. The
mockup puts a small image icon before "Photo"; this plan keeps it text only
(Decision 3).

### 6. Templates

**`assets/views/chat/_message.html`.** The photo becomes:

```html
<a class="photo-link" href="{{ photo_url }}" data-photo-view
   data-author="{{ message.author }}" data-color="{{ message.color }}"
   data-when="{{ message.photo_when }}" data-caption="{{ message.body }}"
   aria-label="Photo from {{ message.author }}. Open full screen">
  <img class="photo-thumb" src="{{ photo_url }}" alt="" loading="lazy" decoding="async">
</a>
```

with `photo_url = org_url(path="/chat/" ~ conversation_id ~ "/photos/" ~ id)`.
No `target`, no `rel`. Without the script, the link opens the photo in the
same tab (inline disposition), and Back returns. Tera escapes the
attributes; `app.ts` only ever reads them into `textContent`.

**`assets/views/chat/show.html`.**

- Remove `{% if conversation.kind == "channel" %}` around the photo input
  and button. The button's label becomes "Add a photo".
- Add the tile row as the composer's first child, hidden:
  `<div class="attach" data-photo-attach hidden>` with the tile `<img alt="Photo to send">`,
  a cover for the spinner or alert, the X button (`aria-label="Remove photo"`),
  the name line and a status line (`role="status"`). Markup follows the
  mockup's `pick()` but with `attach-*` class names: `.pick` already styles
  the reaction picker (`app.css:501`).
- `{% include "chat/_photo_viewer.html" %}` after `#chat-dialog`.

**New `assets/views/chat/_photo_viewer.html`.** One
`<dialog id="photo-viewer" class="viewer" aria-label="Photo"
data-place="{% if conversation.kind == "dm" %}Direct message{% else %}#{{ conversation.label }}{% endif %}">`
holding the mockup's §3 structure: the bar (Close, avatar slot, name,
"when · place", Download `<a download>`), the stage with one `<img>`, the
loading and "Couldn't load this photo" states with "Try again", and the
caption area. Empty slots; `app.ts` fills them. Outside `#chat-feed`, so a
feed reload never touches it.

**`assets/views/chat/thread.html`.** Includes the same viewer, since a
photo can start a thread and the parent shows there. No photo button.

**`assets/views/chat/_feed.html`.** Empty state line becomes "Share an
update, a photo, or ask a question" (§4).

`_actions.html` is unchanged: its quote already says "Photo" for a photo
with no caption.

### 7. `frontend/app.ts`: the composer (§1)

Replace the block at `app.ts:1575-1627`. One staged photo per composer, kept
in a `WeakMap<HTMLFormElement, Staged>` with `{ file, previewUrl, sending }`.

**Picking.** The photo button clicks the hidden input, as today. On
`change`:

- Over 25 MB: today's toast, no tile.
- `file.type` not JPEG, PNG or WebP: toast "That file type can't be sent.
  Use a JPG, PNG or WebP photo." No tile. (An empty `file.type` is let
  through; the server sniffs the bytes.)
- Otherwise stage it, replacing any staged photo: `URL.createObjectURL` for
  the tile, `namedPhoto(file)` for the name, `megabytes(size)` for the size,
  show `[data-photo-attach]`, remove the textarea's `required`, set its
  placeholder to "Add a caption", focus it, and relabel the button "Change
  photo".

**X.** Revoke the preview URL, hide the row, restore `required`, the
placeholder and the label. The typed text stays.

**Send.** A capture-phase `submit` listener on `document`. When the form has
a staged photo it calls `preventDefault()` and `stopPropagation()`, so the
`ws-send` socket never sees the submit. With nothing staged it does
nothing, and text goes over the socket as today. The Enter handler
(`app.ts:238-244`) also submits when a photo is staged and the text is
empty.

While sending:

- Tile spinner, status "Sending..." (mockup copy). The X hides.
- Textarea `readOnly`. Photo and emoji buttons disabled. Send shows a
  spinner with `aria-disabled="true"` and `aria-label="Sending"`.
- A second submit or Enter does nothing.
- No upload percentage: `fetch` can't report upload progress.

The request is today's: the `/orgs/current` probe, then one `POST` of
`FormData { body, file }` to `data-photo-url`, with `orgHeaders()`, and the
502 recheck.

Results:

- **Stale tab** (`staleRefusal`): today's behaviour. The page moves; the
  photo is lost (§5, accepted).
- **200:** clear the staged photo, `form.reset()`, `autogrow`, and keep the
  1.5 s fallback feed reload. The message arrives over the socket; no
  half-sent bubble is drawn.
- **422:** failed state, with the server's text in the status line, word for
  word.
- **Anything else, or a network error:** failed state, "Didn't send. Check
  your connection, then tap Send to try again."

Failed state: coral tile edge and alert mark, `role="alert"` on the line,
the X back, the caption editable again. Photo and caption stay. Send is the
retry. No toast.

### 8. `frontend/app.ts`: the viewer (§3)

**Open.** A `click` listener on `[data-photo-view]`. Modified clicks
(Cmd, Ctrl, Shift, middle button) are left to the browser. Otherwise
`preventDefault()`, then:

1. Fill the bar from the link's data: name, an avatar built with the same
   markup as the `avatar` component (`components/ui.html`), `"{when} · {place}"`.
   Caption via `textContent`; the caption area is hidden when empty.
2. Download `href` = the link's `href` plus `?download=1`.
3. Show the loading state, set the stage `<img>` `src` to the link's `href`
   (the same URL as the thumbnail), `alt` "Photo from {author}: {caption}".
   `load` shows the photo; `error` shows "Couldn't load this photo" and Try
   again (which clears `src` and sets it again).
4. `showModal()`, then `history.pushState({ chatPhoto: token }, "")`, where
   `token` is a counter for this open. Same URL, so nothing reloads and
   nothing navigates.

The existing bubble handler already ignores clicks inside an `a`
(`app.ts:318`), so tapping the photo never opens the message sheet. Tapping
the caption or the bubble edge still does.

**Close.** Every way of closing ends in the dialog's `close` event:

- X: `dialog.close()`.
- Escape: the dialog's own `cancel`, which closes it.
- A tap on the dark area: a click whose target is the stage itself or the
  dialog (the global backdrop rule at `app.ts:32` already covers the
  dialog).
- `close-dialogs` from the server: already closes every open dialog.
- Back: `popstate` with the viewer open calls `dialog.close()`.

The `close` handler clears the `img` `src`, so a slow load stops. Then, if
`history.state?.chatPhoto === token`, it calls `history.back()` to drop the
entry the open added. After Back, the state is already gone, so it does not
go back twice.

**Why this doesn't clash with HTMX history or the socket.**

- The vendored htmx is 2.0.11. Its `onpopstate` only restores when
  `event.state.htmx` is set; otherwise it hands the event on
  (`assets/static/vendor/htmx.min.js`, checked). Our state has no `htmx`
  key, so htmx ignores it.
- No chat page uses `hx-boost` or `hx-push-url` (checked), so htmx never
  writes history on these pages.
- `pushState` to the same URL fires no navigation, no `pageshow` and no
  `hashchange`. The socket stays open. The `#message-N` scroll rule only
  runs on load.
- A reconnect reloads `#chat-feed` only. The viewer is outside it.

**Edge cases.** A reload while the viewer is open leaves one extra history
entry for the same address: one more Back shows the same chat. Forward
after a close returns to an entry with our state; `popstate` with the
viewer closed does nothing. Both are harmless and not worth more code.

### 9. CSS: `frontend/styles/app.css`

From the mockup, using existing tokens:

- `.photo-link`: `cursor: zoom-in`, rounded, and the thumb dims to
  `brightness(0.94)` on hover (desktop only, `@media (hover: hover)`).
- `.composer` gets `flex-wrap: wrap` so `.attach` (`flex-basis: 100%`) sits
  above the text box. `.attach`, `.attach-tile` (72px),
  `.attach-x` (24px dot, 44px hit area), `.attach-cover`,
  `.attach-cover.failed`, `.attach-meta`, `.attach-name`, `.attach-sub`,
  `.attach-sub.err`: the mockup's `.pick*` rules, renamed.
- `dialog.viewer`: `position: fixed; inset: 0; width: 100%; height: 100%;
  max-width: none; max-height: none; margin: 0; padding:
  env(safe-area-inset-top) 0 env(safe-area-inset-bottom); border: 0;` and
  `display: flex` only when `[open]`. Always `#0b0b0c` with `#edece8` text in
  both themes (§3 decision 1). `::backdrop` transparent.
- `.viewer-bar`, `.viewer-who`, `.viewer-dl`, `.viewer-stage`
  (`img` fitted with `max-width/max-height: 100%`, never upscaled),
  `.viewer-state`, `.viewer-cap` (max 28% height, scrolls). At
  `min-width: 900px`: the labelled Download button, Close at the far right
  (`order`), stage margin `8px 64px`, caption 720px centred.
- Reduced motion: the existing `.spin` rule already stops the spinner.

Then `bin/build-assets`.

## Edge cases

- **Old #general photos.** They have `document_id`, so they render through
  the new route and the Documents branch. Deleting one in Documents still
  leaves "Photo removed". `/documents/files/{id}` keeps working for them.
- **A #general file deleted after the page was drawn.** The route 404s; the
  thumbnail breaks and the viewer shows "Couldn't load this photo". Same as
  today with the old link.
- **Removed from the org with the viewer open.** The next load fails, so
  "Couldn't load this photo" (§5).
- **Two tabs, one org.** Each tab's composer stages its own photo.
- **Stored bytes missing.** 500 and a logged error, as `open_file`.
- **Caption with mentions.** Notifications read `message.body`, as today.
- **Big photos.** Every thumbnail loads the full file, and `no-store` means
  no browser cache. Accepted for four people (brief risk). Shrinking is the
  next step.
- **No delete for group or DM photos.** No message can be deleted today
  (brief, assumed accepted). Decision 4.

## Tests

Run as usual (`cargo test`). Fixtures build PNG bytes with
`documents::PNG`. For tenancy, the stored photo is `PNG` followed by
`GLOBEX-SECRET`, so any leak is visible in the body.

### `tests/requests/chat_photos.rs` (update and add)

Update the module doc: photos in every conversation; #general in Chat
photos, groups and DMs on the message.

- **`a_photo_posted_in_a_channel_is_stored_in_chat_photos_and_shown`.**
  Keep the folder and Documents asserts. The socket now carries
  `live.photo == true`. The page has
  `src="/o/acme/chat/{general}/photos/{message}"`, `data-photo-view`, and no
  `target="_blank"` (flipped). Bob opens the chat route: 200, PNG bytes.
  `/documents/files/{id}` still 200.
- **`photos_are_refused_in_dms_and_groups` flips to
  `photos_work_in_dms_and_groups_for_members_only`.** For a DM and a group:
  POST is 200. `documents` count stays 0 and no "Chat photos" folder
  exists. The message has `photo_key` set, `document_id` none,
  `photo_type = "image/png"`. The page has `data-photo-pick` and
  `accept="image/*"`. Bob (a member) GETs the photo: 200, the bytes.
- **`group_and_dm_photos_stay_out_of_documents`.** After posting in a group:
  `/documents` and the Chat photos folder show nothing new; the message's
  id under `/documents/files/{id}` is 404.
- **`a_caption_and_a_photo_send_as_one_message`.** Caption "Van fuel £62"
  in a group: one message, body set, `has_photo`, `photo_key` set.
  Whitespace caption: empty body.
- **`group_and_dm_photos_open_only_for_current_members`.** Alice, Bob,
  Carol in Acme; a group of Alice and Bob; a DM Alice to Bob; a photo in
  each.
  - Carol (Acme member, not in either): 404 on both, and on the POST.
  - Carol's own #general message id under the group's address
    (`/chat/{group}/photos/{general_message}`), and the group photo's id
    under #general's address: 404 both. Bob too, for the crossed ids.
  - Signed out: 303 to `/login`.
  - Pending Dave: 403 waiting page; body has no PNG bytes.
  - Removed Bob: 403; then approved again: group photo 404, DM photo 200.
  - Mallory from another org, with Acme's address: 404. With her own org's
    address and Acme's ids: 404.
  - Super admin acting in Acme: GET and POST 403.
  - Carol made an admin, still not in the group: 404. Role does not matter
    (§5).
- **`chat_photos_are_sent_with_safe_headers`.** For a group photo and a
  #general photo: `Content-Type: image/png`, `nosniff`,
  `private, no-store`, `same-origin` resource policy, the image CSP with
  `sandbox` and `frame-ancestors 'none'`, `X-Frame-Options: DENY`,
  `inline` disposition with the cleaned name; `?download=1` gives
  `attachment`.
- **`only_photos_and_short_captions_are_accepted`.** Run for #general and a
  group: PDF named `.png` 422 "Only photos"; SVG 422; 1,001-character
  caption 422 "1,000"; no file 422; over 25 MB 422 "Too big". Nothing saved,
  and storage holds nothing under `orgs/{org}/chat/`.
- **`a_photo_with_no_caption_says_photo`** (was `..._says_sent_a_photo`).
  The list and Home say `alice: Photo`, `You: Photo`; a DM from Alice to Bob
  says `Photo`; with a caption, `alice: Photo · New office`. The sheet still
  says `<p>Photo</p>`.
- **`a_deleted_photo_leaves_a_note`.** Unchanged, plus: the chat route for
  that message is 404.
- **`a_failed_save_deletes_the_stored_photo`.** Model test: load a group,
  delete its row, then call `create_photo` with the stale model. The insert
  fails on the foreign key; storage holds nothing under
  `orgs/{org}/chat/`.
- **`thread_replies_have_no_photo_button`.** The thread page has no
  `data-photo-pick`; it does include `#photo-viewer`.
- **`people_outside_the_organisation_cannot_post_photos`.** Unchanged.

### `tests/requests/tenancy.rs`

- `Globex` gains `group_photo: messages::Model`, posted in its group through
  `create_photo` with the SECRET bytes.
- `org_a_cannot_reach_org_b`: Alice GETs
  `/chat/{globex.group}/photos/{group_photo}` and
  `/chat/{globex.general}/photos/{globex.message}` in her own org: 404, no
  SECRET. POSTs a photo to Globex's group: 404, nothing saved.
- `documents_and_removal_need_your_own_active_membership`: add the group
  photo GET to `gets` and the photo POST to `posts`. Waiting, removed and
  acting get 403 and no SECRET.

### Unit tests

- `chat.rs`: `photo_when` for today, yesterday, this week and older, in a
  non-UTC zone. The list preview strings in the table above, including the
  80-character cut.
- `messages.rs`: `photo_file` and `photo_removed` for an own photo, a
  Documents photo, a removed one and a text message.
- `documents.rs`: `disposition_for` and `content_security_policy_for` give
  the same strings as the old methods. `photo_name` cleans like `full_name`.
- A source test, `connector_cannot_reach_chat_photos`: no file under
  `src/mcp/` contains `photo_key`, `open_photo` or `send_stored`.

### Migration test

In `tests/models/` (a messages test file there, or a new one): a row with
`photo_key` and `document_id` both set is refused by the constraint, and so
is one with `photo_type = 'image/svg+xml'` or only some of the four columns
set. `down` then `up` is run by hand locally (`cargo loco db down` /
`cargo loco db migrate`) with a group photo present. Whether the harness can
run `down` in a test is unverified.

### The browser (by hand, then screenshots)

`tests/` has no browser runner, so the composer and viewer are checked by
hand. The list is under "Checks".

## Commits

One PR, in this order. Each commit builds and passes `cargo test`.

1. **Messages can carry their own photo file.** The migration, the entity
   regen, the `chat_ws` fixture. Migration test.
2. **Serve stored files through one helper.** `send_stored`,
   `disposition_for`, `content_security_policy_for`, `photo_name`,
   `new_key`. `open_file` uses it. No behaviour change; Documents tests
   pass, plus `X-Frame-Options` now set by the helper.
3. **Group and DM photos are stored on the message.** `create_photo`
   branches by kind; `photo_file`, `photo_removed`. Model tests.
4. **A chat photo route that checks conversation membership.**
   `open_photo`, the route, `send_photo` opened to every kind,
   `MessageView.photo` as a bool, `photo_when`, the new link in
   `_message.html`. All request and tenancy tests above.
5. **Chat list says "Photo · caption".** `list_items`, Home, the empty-state
   line. Updated list tests.
6. **Composer: preview the photo, then send it with a caption.** `show.html`,
   `app.ts` composer block, CSS, `bin/build-assets`.
7. **Photos open in a full-screen viewer in the same tab.**
   `_photo_viewer.html`, includes in `show.html` and `thread.html`, `app.ts`
   viewer, CSS, `bin/build-assets`.

## Out of scope

As the brief: photos in thread replies, several photos per message, swipe or
zoom in the viewer, video, HEIC or other files, paste or drag into the
composer, editing or deleting photo messages, thumbnails or shrinking, a
gallery, moving old #general photos out of Documents. Also: caching
headers for photos, and a "private" badge (§4 says no badge).

## Risks

- **Privacy is the main one.** A new route serves private files. The gate
  is `find_for_member` then `find_in_conversation`, both on every request,
  both already used by every chat route. The tenancy tests cover each kind
  of outsider, and crossed ids between conversations. The security reviewer
  must review this plan and the diff.
- **A bad stored type.** The type comes from sniffing, never the browser,
  and the database refuses anything but the three photo types. The image CSP
  with `sandbox` and `nosniff` back that up.
- **The composer and `ws-send`.** The capture-phase `submit` stop must run
  before htmx's ws extension sees the submit. Unverified until tried in the
  browser; if the extension listens differently, the fallback is to remove
  `ws-send` from the form while a photo is staged.
- **The viewer and history.** Checked against the vendored htmx 2.0.11
  code, not yet in a phone browser. iOS Safari's back swipe must be tested
  by hand.
- **Phone data and R2.** Full-size photos, no cache. Watch R2 storage
  after a week.
- **Thumbnail reuse in the viewer.** Whether the browser reuses the
  thumbnail it already has for the same URL under `no-store` is unverified.
  If not, the viewer shows its loading state while the file loads again.

## Rollback

**The code reverts safely on its own.** Without this PR's code, the extra
columns are ignored. Group and DM photo messages have `has_photo = true` and
no `document_id`, so the old code shows "Photo removed" with the caption.
Nothing breaks and nothing private is exposed. #general is unchanged.

**The migration's `down` drops the columns and the constraint.** That loses
the only record of where group and DM photo bytes are stored, so they would
be orphaned in R2. Before running `down` in production:

```sql
\copy (SELECT id, organisation_id, conversation_id, photo_key FROM messages WHERE photo_key IS NOT NULL) TO 'chat-photo-keys.csv' CSV HEADER
```

Keep the file. The bytes all sit under `orgs/*/chat/` in the bucket, so they
can be deleted later by prefix, or restored if the change comes back. The
migration's `down` doc comment says this too.

Recommended path if something goes wrong: revert the code, leave the
migration in place.

## Checks

```sh
bin/build-assets
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

After deploy, on production (should return no rows; the constraint already
enforces it):

```sql
SELECT id FROM messages WHERE photo_key IS NOT NULL AND document_id IS NOT NULL;
```

By hand, locally, with realistic data: Himalayan Ritual with Raj (owner),
Kreepa, Shusma and Samir; a group #finance (Raj, Kreepa, Shusma) made after
the change; a DM Shusma and Raj; a pending member; Raj also in a second org.

On a phone-sized window (390px) and desktop (1440px), light and dark:

1. In #finance, Kreepa picks a receipt. The tile shows with name and size.
   The caption box is focused and says "Add a caption". Nothing sends.
2. X clears the tile; nothing was sent; the typed text is still there.
3. Pick again, type "Van fuel £62", Send. "Sending...", spinner, Send locked,
   Enter does nothing. One message arrives with photo and caption, for Raj
   too, live.
4. DevTools offline, send again: the failed line, photo and caption kept.
   Back online, Send: it goes once.
5. A HEIC and a 30 MB file: the toasts, no tile.
6. Shusma does the same in the DM with Raj.
7. Raj taps the photo: full screen in the same tab, name, "Today 09:40 ·
   #finance", caption. Download saves the original with its name.
8. Close with X, Escape, a tap on the dark area, and the browser's Back.
   Each leaves Raj in #finance at the same scroll position, socket still
   live (send a message from Kreepa straight after). One more Back then
   goes to where Raj was before #finance.
9. On an iPhone (Railway preview or local over the LAN): the back swipe
   closes the viewer. On an Android phone, the system back button does too.
10. Tap the caption of a photo message: the message sheet opens. Tap the
    photo: the viewer, never the sheet.
11. Samir opens the receipt's address directly: not found. Documents shows
    no receipt. The pending member and Raj's other-org address: waiting page
    and not found.
12. A #general photo: still in Documents, Chat photos; opens in the viewer;
    delete it in Documents and the chat says "Photo removed".
13. The chat list and Home: "Kreepa: Photo · Van fuel £62".
14. With JavaScript off: tapping a photo opens it in the same tab.

### Screenshots needed

On the `pr-screenshots` branch under `chat-photos-2/`, linked from the PR
body by `https://raw.githubusercontent.com/rajgurung/collaboration_tool/pr-screenshots/chat-photos-2/<file>.png`.
From the local app with the data above, not test leftovers. Phone 390px and
desktop 1440px; dark where marked.

- Composer: picked, caption typed, sending, failed (dark).
- #finance feed with the receipt (light and dark).
- Viewer: receipt with caption (phone), wide photo no caption (phone),
  couldn't load (phone), open over #finance (desktop), over a DM with dark
  behind it (desktop).
- The DM with the photo button, the chat list with "Photo · ..." previews.
- No visible change for the access rule; the PR says how it was tested
  (the tenancy test names and their output).

The PR body also says what the migration does and that it can be rolled
back, with the key export step.

## Decisions for Raj

Each with a recommendation.

1. **One route for every chat photo, #general included, while #general
   files stay in Documents.** Recommend: yes. One link shape, one access
   rule, and the brief's "#general shows in Documents" still holds.
2. **A pending member gets the waiting page (403), not "not found".** The
   brief's Done when says "not found". The waiting page is what every
   `/o/{org}/...` address shows a pending member today; it says nothing
   about the photo and is the same for any address. Changing it for one
   route would be a special case. Recommend: keep the waiting page.
3. **Chat list preview is text only: "maya: Photo · Van fuel £62".** The
   mockup adds a small image icon. Recommend: text only in this PR (one
   string feeds both the list and Home). The icon is a small follow-up.
4. **No way to remove a group or DM photo.** As the brief assumes. Recommend:
   accept for now; a "delete message" feature would cover photos too.
5. **An acting super admin gets 403 on chat photos**, as on Documents files.
   Recommend: yes.

## Amendments after review

Binding. Where these differ from the plan above, these win.

### Security

- **S1.** In `open_photo`'s #general `Document` arm, return 404 unless
  `file.content_type` is in `messages::PHOTO_TYPES`. Test: a #general
  message pointed at a PDF document gets 404.
- **S2.** Tests pin the server-derived stored values for a group photo. PNG
  bytes named `evil.html` store `photo_type = image/png` and a `photo_name`
  ending `.png`. A name with `"`, `\r\n` and `\u{202E}` gives a valid header
  and a cleaned name. `photo_size` equals the byte length.
- **S3.** Request test: a caption `"><img src=x onerror=alert(1)>` and a
  group named the same way render escaped in the page and in the socket
  render (`chat_ws::render`). `app.ts` builds the viewer bar only with
  `createElement`, `textContent` and `setAttribute`. No `innerHTML` with any
  `data-*` value (constant icons only).
- **S4.** Test that the page HTML and the socket payload never contain
  `orgs/` or the message's `photo_key`.
- **S5.** Add a comment in `src/mcp/mod.rs` that any future message tool
  must go through `find_for_member`. Drop the brittle
  `connector_cannot_reach_chat_photos` source test.

### Staff

- **R1.** The CHECK constraint's second branch adds explicit `IS NOT NULL`
  for `photo_key`, `photo_type`, `photo_size` and `photo_name`. Migration
  test: rows with `photo_key` set and `photo_type` NULL, and with
  `photo_size` NULL, are refused.
- **R2.** Composer: a document capture-phase `submit` listener that calls
  `stopPropagation()` and `preventDefault()`. The "remove `ws-send`"
  fallback is deleted from the plan.
- **R3.** `group_and_dm_photos_stay_out_of_documents` becomes: the
  `documents` row count is unchanged, no `documents.storage_key` equals the
  `photo_key`, and the Documents pages don't show the file name.
- **R4.** "Storage holds nothing" checks use
  `ctx.storage.list(Path::new(&format!("orgs/{org}/chat")), true)`. Verify
  the memory driver supports `list`; otherwise find another way and say so.
- **R5.** The manual checks list adds the Android back button.

### Nice-to-haves (included)

- `popstate` closes the viewer only if `history.state?.chatPhoto !==
  currentToken`.
- Clear the viewer image with `removeAttribute("src")`, and ignore `error`
  while the viewer is closed.
- A staged photo counts in `hasUnsavedWork()`.
- List preview: cut the body to 80 characters as today, and test that.
- Only lift `opens_inline_type` if `send_stored` needs it.

### Product decisions

- A pending member keeps the existing 403 waiting page.
- No upload percentage: a spinner and "Sending...".
- The chat list preview is "Photo · caption", text only.
