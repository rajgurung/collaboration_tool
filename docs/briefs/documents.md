# Documents

Status: Approved by Raj, 12 October 2026 · Owner: Raj · 12 October 2026

## Summary

A Documents space where an organisation keeps company files and logins in a
few folders. Members add photos and files from their phone. Logins are stored
encrypted, and the password shows only after you re-enter your app password.
There is no 2FA in this version. Raj chose that knowingly; the risk is below.

## Who and why

The request is task #7, "create a company docs folder", in the "Company
Setup" project of Himalayan Ritual. It is assigned to Shusma (admin) and Raj
(owner), due 15 October 2026. Shusma's notes: "Document folder, login
details", "login details for company house", "and email acc",
"office@himalayanritual.co", "able to add in pic, doc".

The team is three people: RajG007 (owner), Kreepa (admin) and Shusma (admin).
Most use phones. Today the app has no file uploads and no storage. Assumption:
papers and logins sit in personal phones and email.

## Decisions (Raj, 12 October 2026)

1. Files are stored in a private Cloudflare R2 bucket. Google Drive and the
   home NAS were rejected as the main store.
2. Logins are stored and shown in the app. This reverses the earlier "never
   store passwords" position.
3. No 2FA in this version. The security reviewer blocked on this and Raj
   overruled it. The residual risk is recorded under Risks.
4. A file can be deleted by its uploader, an owner or an admin.
5. New organisations start with two folders, "Company" and "Logins".
6. Logins live in folders, next to files. One Documents tab, one Add button
   (Photo, File, Login). The "Logins" folder keeps them grouped by default.
   Why: one place to look on a phone, and no second section to learn. A login
   entry is marked with a key icon so it is never mistaken for a file.

7. Design answers (Raj, after the mockup): folders have an optional short
   note; admins can remove members and other admins; any member can rename
   any file; a locked account unlocks by resetting the password with "Forgot
   password"; the "change these at the source" list shows to whoever removed
   the person and stays on that person's member record for owners.
8. Fix the dark-mode error toast contrast in the same change.

## What people can do (first version)

Files. Any active member can:

1. Open Documents from More on a phone, or the sidebar on desktop.
2. See folders, then the folder's items, newest first.
3. Add a photo (camera or library) or a file. Photos, PDFs and Office files,
   up to 25 MB.
4. Open a file. Photos and PDFs open in the browser; others download.
5. Rename a file, and delete files they uploaded.

Owners and admins also create, rename and delete folders (empty only) and
delete any file. Folders are one level deep. Every file open goes through the
app, which checks membership first.

Logins. An entry has a title, username, URL, notes, a password and optional
recovery codes.

- Who sees the secret: owners, admins, and people named on the entry. Others
  see the entry but not its password or codes.
- Reveal: re-enter your app password. That unlocks reveals for about 5
  minutes. Each secret hides itself after about 30 seconds. A copy button
  copies without showing.
- Refused for: the super admin acting as an organisation (`acting_org`),
  bearer tokens and the Claude connector, and pending or removed members.
- Secrets never appear in notifications, emails, search, lists, logs or MCP
  tools.

Safeguards in this version:

- Envelope encryption. Each organisation has its own data key. That key is
  wrapped by a key-encryption key in a Railway variable, `SECRETS_KEK`. Fields
  use XChaCha20-Poly1305. The associated data binds organisation, entry,
  field and key version, so a value cannot be moved to another entry. Only
  password and recovery codes are encrypted. Title, username, URL and notes
  are plain.
- Reveals are a POST. Responses are no-store and cannot be framed.
- Rate limits on sign-in and on re-entering your password, per user and per
  IP. Too many failures locks the account and emails the owner.
- An append-only audit log of reveal, copy, create, edit, delete and failed
  re-entry. Owners can read it. The owner gets an email on every reveal.
- Remove member (owners and admins). Today there is no way to remove someone.
- Sign out everywhere. A per-user session version is bumped by this button
  and by every password reset, which ends all other sessions.
- When someone is removed, the owner sees a "change these at the source" list
  of every login that person revealed.

## Not in this version

- 2FA or passkeys for the app.
- A backup copy of files on the NAS.
- Sharing logins or files outside the organisation.
- Autofill, password generation, or import from a password manager.
- Folders inside folders, per-folder permissions.
- Attaching files to tasks, notes, meetings or chat.
- Search, tags, version history, previews of Office files.
- A trash or undo for deleted files.
- Any Claude connector access to documents or logins.

## Done when

- Shusma photographs a company certificate on her phone and adds it to
  "Company" in under a minute. Kreepa opens it straight away.
- Shusma adds the Companies House login to "Logins" and names Raj on it.
- Raj taps reveal, enters his app password, sees the password, and it hides
  again within about 30 seconds. He gets a reveal email.
- A member not named on that entry sees its title but no password.
- The super admin acting as Himalayan Ritual, a Claude connector token, a
  pending member and a removed member all get refused.
- The database holds no readable password. Changing the key version or the
  entry ID makes decryption fail.
- Five wrong re-entries lock the account and the owner gets an email.
- The audit log shows each reveal, copy and failed re-entry, and cannot be
  edited.
- Removing Shusma ends her sessions and shows Raj the logins she revealed.
- Other tenants and logged-out visitors get nothing. Files survive a
  redeploy. Task #7 can be marked done.

## Risks and open questions

Risks:

- No 2FA, accepted by Raj. Anyone who controls a member's email inbox can
  reset their app password and then reveal every login that member can see.
  The office@ mailbox makes this worse: it is both stored here and a likely
  reset route.
- The platform operator can always decrypt. Whoever has `SECRETS_KEK` and the
  database can read every secret.
- Losing `SECRETS_KEK` loses every stored password for good.
- Tenancy. One missed organisation check leaks papers or logins. Every route
  needs a test like `tests/requests/tenancy.rs`.
- Hard to undo. Deleted files are gone from R2. R2 is not in Railway's
  Postgres backups.
- Unverified: R2 free tier covers this team; 25 MB uploads pass Loco's
  `limit_payload` and Railway's proxy; iPhone Safari sends JPEG not HEIC; the
  Loco S3 driver needs no extra Cargo feature.

Questions for Raj. Answered 12 October 2026: all four recommendations accepted.

1. Where is the backup of `SECRETS_KEK` kept? Recommend: written down offline
   by you, outside Railway and outside this app.
2. Who can create and edit login entries? Recommend: owners and admins only.
   Named members can reveal but not edit.
3. Should members who are not named see the username and URL? Recommend:
   yes, since they are plain by design. Say no if usernames feel sensitive;
   then unnamed members see only the title.
4. Can an admin remove an owner? Recommend: no. Only an owner can remove an
   owner, and the last owner cannot be removed.
