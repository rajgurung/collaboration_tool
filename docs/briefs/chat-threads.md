# Chat threads and reactions

Status: Approved (Raj took all five recommendations, 13 October 2026) · Owner: Raj · 13 October 2026

## Summary

Reply to any chat message in a thread, so a side conversation stops burying
the channel. Under the message, the channel shows "3 replies · 14:05". Add
quick emoji reactions to any message from a fixed set of six. Typing emoji
from the phone keyboard already works, so this version adds no emoji picker.

## Who and why

Raj asked: "Is it possible to create a thread like in Slack, where you could
chat in a thread and also want to put emojis in the messages?"

The team is four people: Raj, Shusma, Kreepa and Samir. Most of them use
phones. Chat today has channels, groups and DMs, @mentions including
@everyone, the bell, and live updates over a websocket.

What the code does today:

- Every message is one flat list per conversation (`messages` has no parent).
  A reply to an older message lands at the bottom, away from what it answers.
- The only way to acknowledge a message is to send another one ("ok", "thanks").
  That adds unread badges and noise for everyone.
- Emoji typed from the keyboard. Nothing blocks them. The body is a Postgres
  `TEXT` column, `messages::create` only trims and checks 1 to 1,000
  characters, and Tera escapes it as text. The Enter-to-send handler skips
  keys while the keyboard is still composing. No test sends an emoji, and I
  have not tried it on a phone. Unverified, but expected to work.
- There is no way to edit or delete a chat message. The only way a message
  disappears is when a user account is deleted with the `user:delete` task:
  their messages go with it (`ON DELETE CASCADE`). A removed member's messages
  stay and show as "Former member".

Assumption: the team wants threads mainly in the channels, where several
topics run at once. Raj can confirm.

## What people can do (first version)

Threads. Any member of a conversation can:

1. Reply in a thread to any top-level message in a channel, group or DM.
   One level only. You cannot start a thread on a reply.
2. See under the parent message in the main feed: the reply count, the last
   reply time and small avatars of who replied ("3 replies · 14:05"). Tap it
   to open the thread.
3. Read the thread: the parent message at the top, replies below, oldest
   first, with its own composer. @mentions and @everyone work as in the
   channel.
4. See new replies and the count arrive live, in the thread and in the
   channel.

Replies do not appear in the main feed. There is no "also send to channel"
option in this version (question 2).

On a phone:

- Tap a message to show its actions: Reply in thread and the reaction row.
  The designer picks tap or long-press; long-press can clash with text
  selection on iPhone.
- A thread opens full screen, like a conversation does today. Back returns to
  the channel at the same spot.
- A thread has its own link, so a bell notice opens the thread directly.

On desktop the thread also replaces the conversation pane. A side panel can
come later.

Who gets notified about a reply (a new "replied" notice in the bell):

- The parent message's author.
- Everyone who has already replied in that thread.
- Anyone @mentioned in the reply who can see the conversation. They get the
  "mentioned you" notice instead, once. This follows how `note_added` and
  `message_sent` avoid double notices today.
- Never the person who wrote the reply.

Thread replies do not add to the chat unread badge (question 4). The bell
covers them.

Reactions. Any member of a conversation can:

1. Add a reaction to any message or reply from a fixed set: 👍 ❤️ 😂 🎉 👀 ✅.
2. Tap the same reaction again to remove it.
3. See each reaction with its count under the message. Tap and hold, or hover
   on desktop, to see who reacted.

Reactions update live and send no notification.

If the parent message is deleted: today that only happens when its author's
account is deleted with `user:delete`. The thread's replies are deleted with
it, the same as every other message of that person today. When message delete
is added later, it should leave a "Message deleted" placeholder and keep the
thread.

Who can see what. Only members of a conversation can see its threads and
reactions, the same check as messages today (`find_for_member`). Someone
waiting for approval cannot open chat at all; the `CurrentMember` extractor
already requires an approved member. A removed member loses access to every
thread with the conversation. The Claude connector has no chat tools and gets
none here.

## Not in this version

- An emoji picker in the composer. The phone keyboard already has one.
- Reactions beyond the six, or custom emoji.
- "Also send to channel".
- Editing or deleting messages, in threads or anywhere.
- Following, muting or leaving a thread. A "Threads" list of everything
  you are in.
- Threads inside threads.
- A desktop side panel for threads.
- Read receipts inside threads.
- Emoji shortcodes like `:smile:`.

## Done when

- Shusma posts "Supplier sent the new price list" in #general. Kreepa taps it,
  chooses Reply in thread, and answers. #general shows "1 reply · 10:42" under
  Shusma's message, and no new message at the bottom.
- Shusma gets a bell notice "Kreepa replied to your message in #general".
  Tapping it opens the thread on her phone.
- Samir replies too. Shusma and Kreepa are each told once. Samir is not told
  about his own reply.
- Raj has #general open on his phone. The count changes to "2 replies" without
  a refresh.
- Raj taps 👍 on Shusma's message. Everyone in #general sees "👍 1" appear.
  Raj taps it again and it goes away.
- A message typed with keyboard emoji, such as "Done ✅🎉", saves and shows
  correctly on iPhone, Android and desktop.
- Someone from another organisation, a pending member and a removed member
  cannot read a thread, reply, or react, even with the right IDs.
- The channel's last-message preview in the chat list never shows a thread
  reply.

## Risks and open questions

Risks:

- Tenancy and IDs. A reply names a parent message, and a reaction names a
  message. The server must check the message is in that same conversation and
  organisation, not just that the IDs exist. Every new route needs a test
  like `tests/requests/tenancy.rs`.
- Feed queries. `recent`, `latest_per_conversation` and `read_in_span` read
  every message in a conversation. Each must skip replies, or replies leak
  into the main feed, the list preview and the read receipts.
- Live updates. The hub sends whole message HTML to sockets in a
  conversation. Replies and reactions need new events, and a reconnect must
  reload threads too.
- Migration. A nullable `parent_id` on `messages` and a new `reactions`
  table. Both can be rolled back. Rolling back after use loses all threading
  and reactions, but not the replies' text.
- Tap targets on a phone. Six reactions plus Reply must fit a 390px screen
  with 44px targets.
- Unverified: emoji typed on a real phone save and render on the live site.
  The composer's `maxlength` counts emoji as 2, while the server counts
  characters; harmless, but a full message of emoji may stop early.

Questions for Raj:

1. What did you mean by emojis in messages? Recommend: reactions from a fixed
   set of six. The keyboard already covers emoji in the text, so a picker adds
   little on a phone.
2. Should a reply have "also send to channel"? Recommend: no, not yet. With
   four people the reply count and the bell are enough. Add it if replies get
   missed.
3. Who is told about a reply? Recommend: the parent's author, earlier
   repliers and anyone mentioned. Not the whole channel.
4. Do thread replies count in the chat unread badge? Recommend: no. The bell
   tells the people involved. Counting them would bring back the noise
   threads remove.
5. Threads in groups and DMs as well as channels? Recommend: yes. It is the
   same work and nobody has to remember where threads are allowed.
