# Homepage: show Documents

Status: Draft · Owner: Raj · 12 October 2026

## Summary

Documents shipped in PR #28, but the public homepage does not mention it. Add
one Documents feature row after Meetings, a nav link, and a Documents item in
the hero's fake sidebar. Copy covers only what is live: folders, photos and
files, opening in the browser, private to the organisation. Nothing about
logins or passwords.

## Who and why

Raj: "we do not have the 'Attach the documents' part on the homepage as a
showcase." Visitors deciding whether to sign up see four features today
(Tasks, Chat, Roadmap, Meetings) in `assets/views/home/index.html`. Company
papers on a phone is a real reason a small team would pick this tool.

## What changes (first version)

1. New feature row, `id="documents"`, after Meetings. Meetings is flipped, so
   this row is not (copy left, preview right). Same `lp-feature` markup.
2. Nav: add "Documents" between Meetings and Claude.
3. Hero window: add "Documents" to the fake sidebar, after Meetings, so it
   matches the real app. The board itself stays as it is.
4. Hero eyebrow becomes "Roadmaps · Tasks · Meetings · Chat · Documents".
5. Unchanged: strip, Claude band, steps, CTA, page title.

Copy:

- Eyebrow: Documents
- Heading: Company papers, one tap away.
- Body: Take a photo of a paper, or add a PDF, Word, Excel or PowerPoint file.
  It goes in a shared folder, newest first, with the name of who added it.
- Ticks: "Snap it with your phone's camera, or pick from your photos" ·
  "Photos and PDFs open right in the browser" · "Only your organisation can
  open them, checked on every open"

Preview (static HTML, `role="img"` with an `aria-label`, `lp-` classes only):
a folder called "Company" with an Add button and "4 files", then four rows
modelled on `documents/_files.html`. Each row has a type tile, a name, and
"who · when · size". Newest first, using the existing cast:

- Certificate of incorporation.jpg (photo icon) · Sonam · Today 09:12 · 2.1 MB
- Supplier contract.pdf · Raj · Yesterday · 840 KB
- Wholesale price list.xlsx · Maya · 3 Oct · 96 KB
- Spring launch deck.pptx · Priya · 1 Oct · 6.4 MB

Account safeguards (sign out everywhere, remove member, sign-in limits) get no
mention. They matter to members, not to a visitor choosing a tool, and the
strip already says "Each organisation gets its own private workspace".

## Not in this version

- Any mention of logins, passwords, a vault, encryption or "secure storage".
- Attaching files to tasks, chat or meetings. Not built. Raj's word "attach"
  must not become that claim.
- Search, sharing outside the organisation, previews of Office files, a bin.
- Documents in the Claude band. The connector cannot see documents.
- Changes to the strip, steps or page title. A real screenshot.

## Done when

- A visitor on a 390px phone scrolls past Meetings and sees the Documents row
  stacked, with no sideways scroll and file names cut with an ellipsis.
- At 1440px the row sits copy left, preview right, and "Documents" in the nav
  jumps to it below the sticky nav.
- It reads well in light and dark mode.
- Every claim matches the live app: types, 25 MB is not overstated, photos and
  PDFs open in the browser, others download.
- `tests/requests/page.rs` asserts the page has `id="documents"` and does not
  contain "password" or "Logins".
- The PR has desktop, phone and dark screenshots.

## Risks and open questions

- Overclaiming is the main risk. HEIC photos from an iPhone download rather
  than open in the browser (`INLINE` in `src/models/documents.rs`). The tick
  says "Photos and PDFs" loosely. Assumption: Safari sends JPEG, as the
  Documents brief also left unverified.
- Six nav links may crowd the header between the breakpoint where links show
  and about 1100px. Unverified until screenshotted.
- Unverified: the homepage on `main` matches the copy read from the documents
  worktree.

Questions for Raj:

1. Is the eyebrow change wanted? Recommend: yes. It is the first line a
   visitor reads.
2. Mention the 25 MB limit on the page? Recommend: no. It is a detail for
   inside the app.
3. When logins ship (PR 2), revisit with a separate brief. Recommend: yes, and
   only after the security reviewer signs off on the wording.
