# Chat photos, round 2

Status: Draft · Owner: Raj · 25 October 2026

## Summary

Make chat photos easy and work everywhere. Picking a photo shows a preview in
the composer with room for a caption, and both send together. Tapping a photo
opens a full-screen viewer in the same window. Photos work in #general, every
group (old and new) and direct messages, and stay private to the people in
that conversation.

## Who and why

Raj, after PR #51:

> "it's a bit rough. You can add photos, but it's very hard to add text onto
> it. Also, when clicking on the photo itself, it takes you to a new tab. I
> sort of want that to be opened in the same tab, in the same window. We have
> only managed to do this in general for the way we add photos. Let's make
> that possible on other channels, finance channels as well. Every time
> somebody creates a new channel, they should be able to add photos. Let's
> make it more intuitive."

The team is four people, mostly on phones. Three problems:

1. Captions are hard. The photo sends the moment you pick it, with whatever is
   already in the text box as its caption (`frontend/app.ts:1575-1627`). There
   is no preview and no chance to type after picking. To caption a photo you
   must type first, then pick, and hope.
2. Tapping a photo leaves the app. The thumbnail is a link with
   `target="_blank"` to the raw file (`assets/views/chat/_message.html:8`). On
   a phone that opens a new browser tab with no way back except the tab
   switcher.
3. Photos only work in #general. This is by design, not a bug:
   - Only `create_general` makes a conversation of kind "channel"
     (`src/models/conversations.rs:51-67`). It runs once, when an organisation
     is created (`src/models/organisations.rs:300`).
   - The only way to make a new conversation is "New group"
     (`assets/views/chat/_list.html:4`), which makes kind "group"
     (`src/models/conversations.rs:154-200`). So #finance is a group.
   - The photo button shows only when `conversation.kind == "channel"`
     (`assets/views/chat/show.html:28`). The server refuses the rest
     (`src/controllers/chat.rs:732`, `src/models/messages.rs:235`).
   - Groups still show a "#" in the header (`show.html:14`), so #finance looks
     like a channel. That is why it feels like a bug.
   - The reason for the rule is privacy (`chat.rs:718-720`). Photos are filed
     in the shared "Chat photos" Documents folder (`messages.rs:249`,
     `src/models/folders.rs:207`). Any approved member can open any document:
     `open_file` checks only the organisation (`src/controllers/documents.rs:488`).
     A #finance photo filed there would be visible to the whole team.

So "photos everywhere" needs a new home for group and DM photos, one that only
the conversation's members can open.

## What people can do (first version)

Sending:

- A photo button in every conversation: #general, every group including ones
  made later, and direct messages.
- Picking a photo puts a preview tile above the text box, with an X to remove
  it. Nothing sends yet.
- The text box becomes the caption. It gets focus and says "Add a caption".
- Send (button or Enter) posts the photo and caption as one message. Caption
  stays optional.
- While sending, the tile shows progress and the composer can't send twice.
  If it fails, the photo and caption stay so you can retry.
- One photo per message, JPG, PNG or WebP, up to 25 MB, as today.

Viewing:

- Tapping a photo opens a full-screen viewer in the same window. The photo
  fits the screen on a dark backdrop, with the sender, time and caption.
- Close with the X, Escape, a tap on the backdrop, or the phone's back
  button. Back closes the viewer; it does not leave the chat.
- A Download button saves the original (`?download` already exists on the
  file route).
- Tapping the photo opens the viewer only. Tapping the rest of the bubble
  still opens the message sheet.
- Works at 390px and on desktop, in light and dark. The designer owns the look.

Privacy:

- A group or DM photo opens only for people who are members of that
  conversation right now. Someone removed from the organisation loses access
  (`conversation_members.rs:206`).
- Group and DM photos never appear in Documents and never open through
  `/documents/files/<id>`.
- #general photos keep going to the "Chat photos" folder. Every approved
  member is in #general (`src/models/memberships.rs:651`), so nothing new is
  exposed.

## Decisions made while Raj is away

1. Direct messages get photos too. Raj said "every channel"; a DM is where a
   quick photo is most natural. Once the privacy rule exists for groups, a DM
   is the same check.
2. Thread replies do not get photos yet. The thread page has its own composer
   (`assets/views/chat/thread.html`) and replies have their own save path
   (`create_reply`). A photo can still start a thread. Add it next if people
   ask.
3. Group and DM photos live outside Documents. The alternative, a folder per
   group, would need folder-level access rules across Documents. That is a
   bigger change than this feature.
4. We keep the words "group" and "#". Renaming groups to channels is a
   separate change.

No true blockers.

## Not in this version

- Photos in thread replies.
- Several photos in one message, or a swipe between photos in the viewer.
- Pinch or double-tap zoom inside the viewer.
- Video, HEIC, PDFs or other files in chat.
- Paste or drag a photo into the composer.
- Editing a caption or deleting a photo message after it is sent. Messages
  cannot be edited or deleted today either.
- Shrinking photos or making thumbnails on upload.
- A "photos in this chat" gallery.
- Moving existing #general photos out of Documents.

## Done when

- In #finance, a group made after this ships, Tom picks a receipt photo,
  sees it in the composer, types "Van fuel £62" and sends. Raj sees one
  message with the photo and the caption.
- Maya does the same in a DM with Ana.
- Raj taps the photo on his phone. It fills the screen in the same tab. He
  taps Download and gets the file. He presses back and is still in #finance.
- Leo, who is not in #finance, cannot find the receipt in Documents. Using
  its address directly gets "not found".
- A pending member and someone from another organisation get "not found" for
  the same address.
- Picking a photo and tapping X sends nothing. A failed send on a weak signal
  keeps the photo and caption for a retry.
- A #general photo still shows in Documents, Chat photos, as today.

## Risks and open questions

- Privacy is the main risk. This puts private files behind a new access
  rule. The security reviewer must review the plan. It needs tenancy tests
  like `tests/requests/tenancy.rs`, and checks for non-members, removed
  members and pending members. The current test
  `photos_are_refused_in_dms_and_groups` (`tests/requests/chat_photos.rs:173`)
  flips to "allowed for members only".
- The photo route must check conversation membership on every request, as
  `find_for_member` does. It must keep today's headers: `no-store`,
  `nosniff`, same-origin and the content security policy
  (`documents.rs:504-523`).
- Migration. Group and DM photos need storage that is not a Documents folder.
  The plan picks the shape and says how it rolls back. Rolling back must not
  orphan stored files without a note.
- Cost. Photos are full size, up to 25 MB each, and each one loads in full
  for its thumbnail (`no-store` means no caching). Fine for four people.
  Watch R2 storage and phone data. Shrinking is the next step if it hurts.
- No way to remove a wrong photo from a group or DM. #general photos can be
  deleted from Documents, which leaves "Photo removed". Assumption: Raj
  accepts this for now, since no message can be deleted. Confirm when he is
  back.
- The back button. The viewer adds a history entry so back closes it. This
  must not clash with HTMX page history or the chat's socket reconnect.
- The Claude connector does not read Documents today (no Documents calls in
  `src/mcp/`). It must not gain access to group or DM photos.
