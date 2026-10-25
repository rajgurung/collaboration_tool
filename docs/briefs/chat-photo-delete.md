# Delete a chat photo

Status: Draft · Owner: Raj · 25 October 2026

## Summary

Let the sender remove a photo they posted to chat by mistake. Owners and
admins can remove one too. The photo goes for everyone, its stored file is
deleted, and the message stays with its caption and a "Photo removed" note.

## Who and why

Raj, after chat photos round 2 (PR #56): "lets do the delete photo next".

That round left a known gap. Its brief says "No way to remove a wrong photo
from a group or DM" and that "no message can be deleted"
(`docs/briefs/chat-photos-2.md:158`). The plan accepted it for now
(`docs/plans/chat-photos-2.md:468`, `:739`).

What people do today:

- A group or DM photo cannot be removed by anyone. It stays in R2 and in the
  chat for good. In a DM or #finance that may be a bank screen, an ID or a
  personal photo sent to the wrong place.
- A #general photo can only be removed by finding it in Documents, Chat
  photos, and deleting it there. Chat then shows "Photo removed", because
  `document_id` goes NULL on delete
  (`migration/src/m20261022_120000_add_photo_to_messages.rs:3`). Nobody would
  guess that from the chat.

Example: in Juniper Tea Co, Tom means to send a fuel receipt to #finance and
sends a photo of his passport. Today he has to ask Ana, the owner, and she
can't help either.

## What people can do (first version)

Who can delete a photo:

- The person who sent it.
- Owners and admins, in conversations they are a member of. This matches
  Documents, where the uploader, owners and admins can delete a file
  (`src/models/documents.rs:335`).
- Nobody else. Members can't remove someone else's photo. A pending member
  and an acting super admin are refused, as on the photo route
  (`require_own_membership`, `src/controllers/chat.rs:812`).
- An admin who is not in a group or DM can't see it, so can't delete it.

What a delete does:

- It removes the photo only. The message, caption, reactions and any thread
  stay. The bubble shows "Photo removed", the state #general already has
  (`messages.rs:386`, `_message.html:8`).
- Group or DM photo: the stored bytes are deleted from R2, then the four
  photo columns are cleared. `has_photo` stays true. The existing
  `messages-own-photo` constraint allows this, so no migration.
- #general photo: the Documents file is deleted too. It is the same file. The
  confirm says so.
- The photo address returns "not found" from then on.
- The chat list and Home line say "Photo removed" instead of "Photo"
  (`last_line`, `chat.rs:486`).

Where the control lives:

- In the message's action sheet, as "Delete photo", in red, at the bottom.
  This is the sheet a tap on the bubble opens on a phone and a click opens
  on desktop (`_actions.html`).
- In the full-screen viewer, next to Download. On a phone this is where you
  notice the wrong photo, and a photo with no caption has little bubble left
  to tap.
- Both show only to people allowed to delete it.
- Both open one confirm: "Delete this photo? It's removed for everyone in
  #finance. This can't be undone." For #general it adds "It's also deleted
  from Documents." Buttons: Cancel and Delete.

Other open pages:

- A new chat socket event swaps the message to "Photo removed" on every open
  page following that conversation, in the feed and on the thread page.
- If someone has that photo open in the viewer, it closes or shows "Photo
  removed". It must not keep showing it.
- A page that missed the event gets it right on the next load.

Phone first: 390px and desktop, light and dark. The designer owns the look.

## Not in this version

- Deleting or editing text messages, captions or thread replies.
- Deleting the whole message. Removing it would orphan threads and reactions.
- Undo, a bin, or a grace period. The bytes are gone at once, which is the
  point.
- Telling the sender when an admin removes their photo, or showing who
  removed it.
- An audit log of removals.
- Long-press menus. The tap-to-open sheet already exists.
- Removing a photo through the Claude connector. No connector tool reads chat
  today (`src/mcp/mod.rs:6`).

## Done when

- In #finance, Tom sends the wrong photo. He taps the bubble, taps Delete
  photo, confirms. His bubble keeps "Van fuel £62" with "Photo removed".
- Maya has #finance open on her phone. Her copy changes to "Photo removed"
  without a reload. If she had the photo open full screen, it no longer
  shows.
- The photo's old address gives "not found" to everyone. The bytes are gone
  from R2 (checked by key in a test).
- Leo sends a photo to Ana in a DM, opens it full screen and deletes it from
  the viewer. Ana sees "Photo removed".
- Maya, an admin, removes a photo Tom posted in #general. It is gone from
  Documents, Chat photos, too.
- Leo, a member, sees no Delete photo on Tom's photo. Posting to the delete
  address directly is refused.
- A pending member, someone from another organisation, and someone not in the
  group all get "not found" or the waiting page, as for the photo route.
- A thread under a removed photo still opens with all its replies.

## Risks and open questions

- Privacy and access. This is a new write route on private files. It must
  load the conversation with `find_for_member` and the message with
  `find_in_conversation`, then check sender or owner/admin. It needs tenancy
  tests like `tests/requests/tenancy.rs`.
- Hard to undo. Deletion is permanent by design. The confirm is the only
  guard.
- Order of steps. If the R2 delete fails, the photo must stay and the person
  sees an error they can retry. If the row update fails after the bytes are
  gone, the next try must still finish. The plan picks the order; Documents
  deletes bytes first (`documents.rs:326`).
- Assumption: R2 bucket versioning and lifecycle copies are off, so a delete
  really removes the bytes. Confirm in the Cloudflare dashboard.
- Assumption: notification emails and the bell never carry the photo itself,
  only the caption. Confirm in `controllers/notifications.rs`.
- Assumption: owners and admins may remove others' photos without telling
  them. Raj, is that fine for a team of four? Recommend yes.
- A #general file may have been moved out of Chat photos in Documents. It is
  still the same file and is still deleted. Recommend yes; the confirm warns.
