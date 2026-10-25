# Delete a chat message

Status: Draft · Owner: Raj · 25 October 2026

> **Changed 25 October 2026: hard delete.** Raj chose to remove the message
> row instead of leaving a "Message deleted" placeholder. A deleted message
> disappears from every feed and thread, with its reactions and the bell
> notices about it. Threads: the sender can delete their own message only
> if it has no replies; owners and admins can delete any message, and a
> deleted parent takes its replies with it. Lines below that said otherwise
> are updated; the plan (`docs/plans/chat-message-delete.md`) has the detail.

## Summary

Let the sender delete a message they sent, text and all. Owners and admins in
the conversation can delete one too. The text, any photo and the reactions
go for everyone, for good. The message disappears from the chat.

## Who and why

Raj, after photo delete (PR #59): "i believe we should also have the ability
to delete the text-message as well, since we can now delete the images."

The photo delete brief listed this as not done: "Deleting or editing text
messages, captions or thread replies" (`docs/briefs/chat-photo-delete.md:87`).

What people do today:

- Nobody can remove a message. A wrong text stays in the chat, the chat list,
  Home and any bell notice for good.
- Only the photo part of a message can go. Its caption stays.

Example: in Juniper Tea Co, Tom pastes the supplier portal password into
#general instead of his DM with Ana. Today it stays there, and if he typed
@ana it also sits in Ana's bell and, after 15 minutes unread, her email.

## What people can do (first version)

Who can delete a message:

- The person who sent it.
- Owners and admins, in conversations they are a member of. Same rule as
  photos (`may_delete_photo`, `src/models/messages.rs:55`). They don't notify
  the sender.
- Nobody else. A member can't delete someone else's message. A pending
  member and an acting super admin are refused, as on the photo route
  (`require_own_membership`, `src/controllers/chat.rs:877`).
- This covers top-level messages and thread replies alike.
- A message with replies: only owners and admins can delete it, and its
  replies go with it. The sender's sheet says "This has replies. Ask an
  owner or admin to delete it." instead of the row.

What a delete does:

- The message row is deleted from the database, not hidden. Reactions on it
  go.
- Any photo goes too, the same way photo delete does it: bytes first, then
  the row. A #general photo's Documents file is deleted as well.
- The bubble disappears from the feed or thread.
- Threads: deleting a parent deletes its replies. A deleted reply disappears
  from the thread and the reply count drops.
- Unread counts drop, since the message no longer exists.
- Chat list and Home: if the last message was deleted, the line shows the
  message before it (`last_line`, `chat.rs:502`).
- Bell and email: mention and reply notices for that message (and its
  replies) are deleted with it. If an email has not gone yet, it is not sent.
- No undo. The confirm is the only guard.

Where the control lives:

- The message sheet gets one red row at the bottom: "Delete message". It
  replaces today's "Delete photo" row (`assets/views/chat/_actions.html:20`).
  One delete row per sheet, so nobody has to guess which one they want.
- "Delete photo" stays in the full-screen viewer only. That is where you look
  at a photo, and it keeps the caption. So the sheet deletes the message and
  the viewer deletes the photo.
- The thread page's sheet gets the same row, for the parent and each reply.
- One confirm: "Delete this message? It's removed for everyone in #finance.
  This can't be undone." With a photo it adds "The photo is deleted too." For
  #general it adds "It's also deleted from Documents." In a DM: "It's removed
  for you and {name}." For a parent: "Its 3 replies are deleted too."

Other open pages:

- A new chat socket event removes the bubble from every open feed and thread
  page for that conversation, like `PhotoRemoved` (`src/data/chat_hub.rs:54`).
  A thread page whose parent was deleted says "This thread was deleted".
- An open viewer on that photo shows its removed state. A sheet left open on
  another screen finds the message gone and says so.
- The chat list and Home don't update live, as today. They are right on the
  next load.

Phone first: 390px and desktop, light and dark. The designer owns the look.

## Not in this version

- Editing a message. It is not cheap: it needs an "edited" mark, rules for
  new and removed @mentions, notice text and a live update. Worth its own
  brief.
- Undo, a bin or a grace period.
- Telling the sender, or showing who deleted it.
- Deleting several messages at once, or a whole conversation.
- Recalling an email that has already gone out.
- Long-press menus. The tap-to-open sheet already exists.
- The Claude connector. No tool reads chat today (`src/mcp/mod.rs:6`).

## Done when

- Tom pastes a password in #general and deletes it from the sheet. His bubble
  disappears. The row is gone from the database.
- Maya has #general open on her phone. Her copy changes without a reload.
- Tom had written "@ana" in it. The notice is gone from Ana's bell. No
  mention email arrives later.
- Leo deletes a #finance message that has a photo and caption. The photo
  address gives "not found" and the bytes are gone.
- Maya, an admin, deletes a parent message with three replies. The parent
  and all three replies are gone. Leo deletes his own reply in another
  thread; it disappears and the count drops. Tom can't delete his own
  message once it has replies.
- Leo, a member, sees no "Delete message" on Tom's message. Posting to the
  delete address directly is refused.
- A pending member, someone from another organisation and someone not in the
  group get "not found" or the waiting page.
- The chat list and Home show the message before it when it was the last
  one. Ana's unread count drops by one if she hadn't read it.
- In the viewer, Tom can still delete only the photo and keep the caption.

## Risks and open questions

- Privacy and access. A new write route that destroys data. Same gate as
  photo delete: `find_for_member`, then `find_in_conversation`, then sender or
  owner/admin. Needs tenancy tests like `tests/requests/tenancy.rs`.
- Migrations. Notifications need a link to their message (a nullable
  `message_id`, deleted with the message). Today a notice holds only text
  and a page link (`src/models/_entities/notifications.rs`). An added
  column, so it can be rolled back, but deleted messages never come back.
- Older notices, made before this ships, have no `message_id`. Their quotes
  stay in the bell. Accept that, or match on link and text. Recommend accept.
  Their unsent emails are stopped at deploy.
- Emails already sent, and Railway database backups, still hold the text.
  Assumption: backups age out on their own. Confirm the retention in Railway.
- Assumption: no other place copies message text. Checked: bell, mention
  email, chat list, Home and the guide's "said hello" step (`guide.rs:33`,
  which no longer counts a deleted hello). No chat search exists.

Questions for Raj:

1. (Decided: hard delete, see the note at the top.) Placeholder always, or
   remove it entirely when nothing hangs off it?
   Recommend a "Message deleted" placeholder always. One rule, honest, no race
   with a reply arriving mid-delete, and it matches "Photo removed". The
   other way is tidier but needs two paths.
2. Scrub bell notices and stop pending emails in this version? Recommend yes.
   A leaked password is the main reason to delete, and today the mention
   email would still carry it 15 minutes later.
3. Sheet shows "Delete message" only, and "Delete photo" moves to the viewer
   only? Recommend yes, to avoid two red rows.
