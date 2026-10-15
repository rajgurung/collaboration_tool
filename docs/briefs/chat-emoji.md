# Chat emoji, attached reactions and task notes

Status: Approved · Owner: Raj · 15 October 2026

## Summary

Make reactions and the thread line look part of the message, the way
WhatsApp pins reactions to the bubble. Bring the same six reactions and
@everyone to task notes. Add a built-in emoji picker to the chat composer,
the thread composer and the note box.

## Who and why

Raj asked, after reactions and threads shipped (PRs #35, #36):

> "could you make the chat's emoji stick on the actual chat.. like how whats
> app does.. also the thread replies info .. all looks bit too segmented..
> and doesn't look seamless... and also on the ticket.. we should be able to
> tag @everyone as well.. also please add the emojis in the comment.. also
> please ensure that there is an emoji picker we can use during the chat and
> in the ticket comments.."

The team is four people, mostly on phones. Four problems:

1. Chat looks broken up. One message stacks four blocks: the bubble, a time
   and receipts row, a row of bordered reaction pills, then a separate
   "3 replies · 14:05 >" line (`chat/_message.html`, `_reactions.html`,
   `_thread_link.html`). On a phone the feed reads as a list of parts.
2. Notes on a task cannot be reacted to. The only way to say "seen" is to
   write another note, which notifies the assignees again.
3. @everyone works in chat (PR #26) but not in task notes. Notes only
   understand @name (`notifications::note_added`). Today you must tag each
   person.
4. There is no emoji picker. The last brief left it out because phones have
   one. Raj now asks for it, and desktop users have no easy way in.

## What people can do (first version)

Chat looks seamless:

- Each emoji gets its own small chip with its own count ("👍 4 · 🎉 3 ·
  ❤️ 1"), in a row that overlaps the bottom edge of the bubble. Your own
  reaction is amber. Tapping a chip shows who reacted with it.
- The thread line sits tight under the bubble with no gap or border, so it
  reads as part of the message. It keeps the reply count, last time and
  avatars.
- Adding a reaction is unchanged: tap the message, pick from the six.
- The designer owns the exact look. It must work at 390px, in light and dark.

Reactions on task notes:

- Any member who can open the task can react to a note with the same six
  emoji, and tap again to remove. It looks the same as chat.
- No notification. No live update; the counts refresh when the sheet does.

@everyone on task notes:

- Typing @everyone in a note notifies every approved member of the
  organisation except the writer, once each (question 2).
- The note box's @ autocomplete offers "everyone", as chat does.
- Someone mentioned by name and by @everyone gets one notice.

Emoji picker:

- A smiley button next to the chat composer, the thread composer and the
  note box. It opens a panel; tapping an emoji inserts it at the cursor. The
  panel stays open for more.
- A curated set of about 200 common emoji in a few categories, with a
  "Recent" row kept on the device. No search.
- Built in. No third-party script, font or image service. Emoji draw with
  the device's own font.
- On a phone it opens as a sheet above the keyboard area. It must not hide
  the text being typed.

## Not in this version

- Reacting with any emoji. Reactions stay at the six (question 3).
- Emoji search, skin tones, custom emoji, GIFs or stickers.
- Shortcodes like `:smile:`.
- Live updates on the task sheet.
- Threads on task notes, and editing or deleting notes or messages.
- @everyone for the Claude connector's notes (see risks, question 4).
- @here, @channel or role groups like @admins.

## Done when

- On Raj's phone, a message with 👍 from two people shows one pill "👍 2"
  pinned to the bubble's corner. A message with replies shows its thread line
  right under it. Nothing looks like a separate card.
- Kreepa adds a note "Booked the van ✅" on a task. Shusma taps ❤️ on it.
  Kreepa opens the task later and sees "❤️ 1". Nobody gets a notice for it.
- Samir writes "@everyone stock count Friday" in a note. Raj, Shusma and
  Kreepa each get one bell notice that opens the task. Samir gets none.
- Shusma taps the smiley in #general, taps 🎉 twice and 🙏 once, and sends.
  The message reads "🎉🎉🙏". Next time, those emoji are in Recent.
- The same works in a thread and in the note box, on a phone and on desktop.
- A pending member and someone from another organisation cannot react to a
  note, even with the right IDs.

## Risks and open questions

Risks:

- Tenancy. A note reaction names a note. The server must check the note
  belongs to a task in the same organisation. Needs a tenancy test, like
  `tests/requests/tenancy.rs`.
- Notice noise. @everyone on a note reaches the whole organisation. Fine for
  four people. In a larger team it could be overused. Chat already accepts
  this.
- The Claude connector adds notes too (`src/mcp/mod.rs` calls `note_added`).
  If it inherits @everyone, an AI tool could notify the whole team. Assumption:
  Raj does not want that. Recommend the connector skips @everyone for now.
- Migration. Note reactions need storage. The current `reactions` table is
  tied to messages. A new `note_reactions` table is simplest and rolls back
  cleanly; rolling back loses only note reactions.
- Phone layout. The overlapping pill must not cover the time, the receipts or
  the next message. The picker must not push the composer off screen when the
  keyboard is up.
- Emoji rendering varies by phone. Some newer emoji show as boxes on older
  devices. Keep the curated set to widely supported emoji.
- The textarea `maxlength` counts most emoji as 2 (noted in the last brief).
  A message full of emoji may stop early. Harmless; not fixed here.

Questions for Raj:

1. "add the emojis in the comment": did you mean reactions on task notes?
   Recommend: yes, the same six as chat, same look. The picker covers emoji
   inside the note text.
2. Who does @everyone on a task note reach? Recommend: every approved member
   of the organisation. It matches the word and the chat meaning. The
   alternative is the task's assignees and earlier commenters, but they
   already get a "commented" notice, so @everyone would add little.
3. Should the picker also let you react with any emoji? Recommend: not yet.
   The six keep reactions quick and readable. Add "+ more" later if people
   ask.
4. Should the Claude connector be able to use @everyone in notes? Recommend:
   no for now. Only people should be able to notify the whole team.

## Decisions (15 October 2026)

Raj approved the brief and the mockup (`docs/designs/chat-emoji.html`) and
went with every recommendation:

1. Reactions on task notes: yes, the same six, same look.
2. @everyone on a note reaches every approved member of the organisation.
3. Reactions stay at the six; no "react with any emoji".
4. The Claude connector cannot use @everyone in notes.
5. Group read-receipt faces leave the message; ticks show the state, and who
   read it moves to the message sheet.
6. Every message shows its time, inside the bubble.
7. The picker has search by emoji name.

Raj also asked for one chip per emoji with its own count, instead of one
combined pill. The mockup shows this.

Chip taps (15 October 2026): tapping a chip adds your reaction with that
emoji, or removes it if it is already yours, in one tap, like Slack. Who
reacted shows on hover on desktop and in the message sheet (tap the bubble)
on phones. This replaces "tapping a chip shows who reacted". It applies to
chat and task notes.

One PR (15 October 2026): Raj chose a single PR for the chat restyle, note
reactions, note @everyone and the picker, over the product manager's
suggestion to split it.

Who read it (15 October 2026): Raj reversed decision 5. Reader faces stay on
the last of your messages in a run, as today. Your messages get one row under
the bubble: reaction chips on the left, who read it on the right. Mockup
section 8.
