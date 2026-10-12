# Documents space: build plan

Status: Approved by Raj, 12 October 2026 (revision 2, after staff, product and security review) · Owner: Raj

Builds the approved brief `docs/briefs/documents.md` and the approved mockup
(https://claude.ai/artifact/DvmwqWKXgAxqd3uNJEwc4f, local copy
`/private/tmp/claude-501/-Users-rajg-Codes-collaboration-tool/83f0d096-c371-4303-b30b-b78218969c3d/scratchpad/documents-mockup.html`).

## Summary

Two PRs, so files ship before task #7's due date (15 October):

- **PR 1, branch `documents`:** account safeguards and files.
- **PR 2, branch `documents-logins` (after PR 1 merges):** logins, encryption,
  reveal, login activity, and the "change these at the source" list.

## Decisions from review (Raj, 12 October 2026)

1. A super admin in "act as" mode (`CurrentMember.acting`) is refused on every
   Documents route (files, folders, activity, logins) and on remove member.
   They use their own membership instead.
2. Failed sign-ins give a 15-minute lock that clears itself. "Locked until
   password reset" is only for 5 wrong app passwords typed inside a signed-in
   session (re-entry, PR 2); that lock also ends all the user's sessions.
3. A token without the session version claim is rejected: everyone signs in
   once after deploy. Raj posts a heads-up in General before merging.
4. Split into two PRs as above.
5. Photos show a type icon, not a thumbnail, in this version. HEIC is
   download-only.

---

# PR 1: account safeguards and files

## 1. Client IP

Enable Loco's `remote_ip` middleware in `config/*.yaml`
(`.claude/skills/loco/recipes/middleware.md`) with one trusted source. Check
Railway's docs for whether it overwrites `X-Forwarded-For`/`X-Real-IP` and
whether anything proxies in front of it; default to `RightmostXForwardedFor`.
Every limit and audit field reads only the `RemoteIP` extractor.

## 2. Session version

- Migration: `users.session_version int not null default 0`.
- `users::Model::generate_jwt` puts `"sv"` (and, for PR 2, a random `"sid"`)
  into the claims map.
- `CurrentUser` rejects a JWT whose `sv` is missing or differs (redirect to
  `/login`). Everyone signs in once after deploy (decision 3).
- Bump with one SQL `SET session_version = session_version + 1` (no lost
  updates). One model function `users::bump_session_version(db, user_id)` that
  also publishes a "user signed out" event (below).
- Bumped by: sign out everywhere, completed password reset, removal.
- `POST /account/sign-out-everywhere`: bump, then issue a fresh cookie for this
  device. Row under More, Account (mockup 1a).
- Password reset keeps today's redirect to `/login?reset=1`.
- **Live connections:** add a "user signed out" event to `ChatHub` and
  `NotifyHub` (`src/data/`). Chat WebSocket loops (`src/controllers/chat_ws.rs`)
  and the notifications SSE stream (`src/controllers/notifications.rs`) close
  when the event names their user. Test both.

## 3. Sign-in limits

- Table `auth_attempts (id, email_hash, user_id null, ip, kind, created_at)`,
  `kind` in `login | forgot` (PR 2 adds `reauth`). Indexes on
  `(email_hash, created_at)` and `(ip, created_at)`. Old rows deleted lazily on
  insert (older than 24 hours).
- Counted by normalised email (`users::normalize_email`, then SHA-256), so
  unknown emails count too.
- 5 failed sign-ins for an email in 15 minutes: further attempts for that email
  are refused for 15 minutes. 20 failures per IP in 15 minutes: refused from
  that IP. A successful sign-in or completed reset clears that email's rows.
- The refusal message is the same for real and unknown emails, and the dummy
  hash still runs, so timing does not reveal which emails exist.
- `/forgot`: at most 3 per email and 10 per IP per hour, same message either way.
- No email to owners for temporary sign-in locks (the lock clears itself).

## 4. Password changed email

On a completed password reset, email the user ("Your password was changed").
New mailer `security`. Templates escape every variable (see 7). PR 2 extends
this to owners when the user can reveal logins.

## 5. Remove member

- `memberships.status` gains `removed`; add `removed_at`, `removed_by_id`
  (nullable, `ON DELETE SET NULL`). Down migration sets `removed` rows to
  `rejected` first.
- `POST /members/{id}/remove`, owners and admins, refused when `acting`.
  Rules, checked in a transaction that locks the organisation's owner rows:
  not yourself; only an owner removes an owner; never the last owner.
- Removal, in one model function `memberships::Model::remove`:
  - sets status; bumps the user's session version (ends sessions and live
    streams);
  - revokes their access tokens (`access_tokens::revoke_all_for(org_id, user_id)`);
  - deletes their `conversation_members` rows, so chat sends stop;
  - marks their unread notifications as emailed, so no mention email goes out
    after removal; `mailers::mentions::send_due` also skips non-active members.
- A user belongs to one organisation (`memberships.user_id` is unique). State in
  the PR: a removed person cannot join any organisation with that email.
- Members page: hide `removed` from the member list; add a "Removed" section
  for owners (name, removed by, date). PR 2 links each to its rotate list.
- A removed person who signs in sees a new `tenancy/removed.html` ("You were
  removed from {org}"), not `declined.html`.
- UI: Remove button and confirmation sheet (mockup 6c, 6d). Copy in 6d is true
  only because of the live-stream handling above.

## 6. Files

1. **Storage.** Enable `loco-rs` feature `storage_aws_s3`. In
   `App::after_context`, keep the hub inserts and set storage with
   `ctx.into_builder().storage(...)` (`AppContext` is non-exhaustive):
   - production: `aws::with_credentials_and_endpoint(bucket, "auto", endpoint, Credential { .. })`;
   - test: `mem::new()`;
   - development: `local::new_with_prefix("tmp/storage")` after
     `create_dir_all` (gitignore `tmp/storage`).
   R2 settings come from `settings:` in `production.yaml` via `get_env` with no
   default (`R2_BUCKET`, `R2_ENDPOINT`, `R2_ACCESS_KEY_ID`,
   `R2_SECRET_ACCESS_KEY`), so a missing one fails at boot. Verify that.
2. **Data.**
   - `folders (id, organisation_id, name, note null, created_by_id null, created_at, updated_at)`,
     unique `(organisation_id, lower(name))`.
   - `documents (id, organisation_id, folder_id, name, content_type, size_bytes, storage_key, uploaded_by_id null, created_at, updated_at)`.
   - `folder_id` is `ON DELETE RESTRICT`; user FKs `ON DELETE SET NULL`.
   - `storage_key` = `orgs/{org_id}/docs/{uuid}`; never from the file name.
   - Starter folders "Company" and "Logins" (Logins note: "Shared logins for the
     team. Never put passwords in notes.") at organisation creation, and by the
     migration for existing organisations.
   - Add the new tables to `App::truncate`, children first.
3. **Routes.** All through `CurrentMember`; all refuse `acting` (decision 1).
   - `GET /documents`, `GET /documents/folders/{id}`.
   - Owners and admins: `POST /documents/folders`, `.../{id}` (rename, note),
     `.../{id}/delete` (only when it has no files; PR 2 adds logins to the check).
   - `POST /documents/folders/{id}/files`: one file per request. Route
     `DefaultBodyLimit::max` 26 MB; enforce 25 MB while reading chunks; test
     that the route limit overrides the global `limit_payload`.
     - Type comes from a server-side sniff (`infer` crate) against an allowlist:
       JPEG, PNG, WebP, HEIC/HEIF, PDF, and Office files (sniffed as ZIP or OLE,
       exact type from extension). Anything else, including SVG, HTML, XML and
       text, is refused. `content_type` is stored from the allowlist only.
     - Upload with `storage.upload(&Bytes)` (one PUT). Write the object first;
       if the row insert fails, delete the object.
   - `GET /documents/files/{id}`: membership check, then
     `ctx.storage.download_stream(...)`, `Content-Length` from `size_bytes`.
     JPEG, PNG, WebP and PDF `inline`; everything else `attachment`. Headers:
     `X-Content-Type-Options: nosniff`, `Cache-Control: private, no-store`,
     `Cross-Origin-Resource-Policy: same-origin`,
     `Content-Security-Policy: default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; sandbox`
     (drop `sandbox` for PDFs only if Chrome or Safari stop rendering them; note
     which), and an RFC 6266 filename. No file name in the URL. Never presign.
   - `POST /documents/files/{id}/rename`: any active member. Changes the stem
     only; the extension comes from the stored type. Strip control characters and
     bidi overrides; max 200 characters.
   - `POST /documents/files/{id}/delete`: uploader, owner or admin. Delete the
     object, then the row.
4. **UI** per mockup sections 1, 2, 3 (except 3e) and 7: More row and sidebar
   link, folder list with counts, folder view newest first with type icons
   (decision 5), Add sheet (Photo, File), upload progress (handle
   `htmx:xhr:progress` only until the upload completes,
   `hx-encoding="multipart/form-data"`), size and type checked in `app.ts`
   before sending, inline row errors, delete confirm with "no bin" copy, empty
   states, desktop two-pane. `hx-history="false"` on Documents pages.
   Delete shows only for the uploader, owners and admins.
5. **Headers:** `X-Frame-Options: DENY` and `frame-ancestors 'none'` on all
   `/documents/*` responses.

## 7. Fixes in the same PR

- Dark-mode `.toast-error` contrast (designer).
- Escape every variable in mail `html.t` templates, new and existing
  (`src/mailers/mentions/unread/html.t` renders chat excerpts unescaped). Test
  that `<a>` in a message excerpt is escaped.

## PR 1 tests

- Tenancy for every new route in `tests/requests/tenancy.rs`: other org,
  logged out, pending, removed, acting super admin.
- Session version: old cookie refused after sign-out everywhere, reset and
  removal; token without `sv` refused; chat WebSocket and SSE closed on bump.
- Limits: 5 bad sign-ins refuse the 6th, unknown email gets the same message,
  lock clears after the window, success clears the count, per-IP limit using
  different `X-Forwarded-For`, `/forgot` limit.
- Remove rules: self, last owner, admin removing owner, acting refused;
  removed person sees `removed.html`, is gone from Members, receives no
  mention email, cannot post to chat.
- Files: upload; refuse oversize, SVG, HTML and a renamed `.exe`; stream with
  every header; rename keeps the extension; delete by uploader and admin;
  member cannot delete someone else's file; folder delete only when empty.
- Mail escaping.

## PR 1 migrations

All additive with down migrations: users.session_version; auth_attempts;
memberships removed columns; folders and documents with starter folders.

## Before merging PR 1 (Raj)

- Create the R2 bucket. Public access off, no r2.dev subdomain, no custom
  domain, no CORS rules. API token with Object Read & Write on that bucket only.
- Set `R2_BUCKET`, `R2_ENDPOINT`, `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY` in
  Railway.
- Post in General that everyone will need to sign in again once.

---

# PR 2: logins

Built after PR 1 merges, on `documents-logins` from main.

1. **Crypto.** `chacha20poly1305` 0.11 (XChaCha20-Poly1305) and `secrecy` 0.10.
   - `SECRETS_KEK`: base64 of 32 bytes, required in production; boot fails if
     it is not 32 bytes or equals the dev/test key. Held as `SecretBox<[u8; 32]>`.
   - `org_keys (organisation_id pk, wrapped_dek, nonce, kek_version, dek_version, created_at)`.
     Created with the organisation, and by the migration for existing ones;
     anywhere else, `INSERT ... ON CONFLICT DO NOTHING` then re-select. Wrapped
     DEK associated data: `org:{id}|kek:{v}`.
   - Field associated data: `org:{org}|login:{login}|field:{name}|dek:{v}`.
     Column `logins.dek_version`.
   - One module `src/models/secrets/crypto.rs` (`seal`, `open`). Plaintext is
     `SecretString`; no `Debug` or `Serialize` over secrets; forms holding a
     password use `SecretString`.
2. **Data.** `logins (id, organisation_id, folder_id RESTRICT, title, username, url, notes, password_ct, password_nonce, recovery_ct null, recovery_nonce null, dek_version, created_by_id null, created_at, updated_at)`;
   `login_access (login_id, user_id)`; `login_events (id, organisation_id, login_id null ON DELETE SET NULL, login_title, user_id null, username, action, ip, user_agent, created_at)`
   with a Postgres trigger that raises on UPDATE or DELETE (down migration drops
   it). Actions: reveal, copy, create, edit, delete, unlock, reauth_failed,
   lock, member_removed, sign_out_everywhere.
3. **Rules.** `logins::Model::can_reveal(&CurrentMember)` requires a session
   (new `CurrentMember.via_session` flag; bearer sets false), not `acting`, and
   owner, admin or named in `login_access`. Create and edit: owners and admins.
   `login_access` ids must be active members of the org. URL: `http`/`https`
   only, rendered with `rel="noopener noreferrer"`. Notes hint: "Visible to
   every member. Not encrypted."
4. **Unlock and reveal.**
   - `POST /documents/logins/unlock`: refuses `acting`; limit `reauth`; on
     success set `users.secrets_unlocked_until` (+5 min) and
     `secrets_unlocked_sid` = this token's `sid`. Both must match to reveal.
     Cleared on any session version bump.
   - 5 wrong re-entries: `users.locked_at` (unlock only by reset), session
     version bump, one email to owners per lock.
   - `POST /documents/logins/{id}/reveal` and `.../copy`: write the event row
     first and fail closed; then return the fragment with `no-store`; queue the
     reveal email to owners after (never contains the secret or a link to it).
   - Client: 30-second auto-hide, clear the node on hide and on `pagehide`. Copy
     uses `ClipboardItem` with the fetch promise for iOS Safari; test on an iPhone.
5. **Owner views.** `GET /documents/activity` (owners; refuses `acting`).
   `/members/{id}/rotate`: logins the removed person created, edited, revealed
   or copied, plus any they could reveal; mailbox entries first. Visible to the
   person who removed them and to owners, from the Removed section.
6. **Password changed email** also goes to owners when the user can reveal any login.
7. **UI** per mockup 3e, 4, 5, 6a, 6b, 6e, 6f.
8. **Tests:** crypto round trip; changing `dek_version` or the login id fails
   decryption; DEK race; DB holds no plaintext; reveal needs unlock from the same
   session; unlock expires; unnamed member, acting, bearer refused; event row
   written before plaintext; trigger blocks UPDATE/DELETE; reauth lock bumps
   sessions; `javascript:` URL refused; foreign user id refused; MCP tool list
   has no login or document tools.

## Checks (both PRs)

`bin/build-assets`, `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`,
`cargo test` against `DATABASE_URL=postgres://loco:loco@localhost:5433/collab_test_documents`.
Screenshots at 1440px and 390px, light and dark, with realistic data.

## Unverified

- Railway's forwarded-header behaviour (pick the `remote_ip` source from its docs).
- That 26 MB bodies pass Railway's proxy, and that the route limit overrides
  `limit_payload` for multipart.
- That browsers render PDFs under the CSP `sandbox` directive.
- iPhone Safari: HEIC conversion on upload, clipboard after fetch.
- That `get_env` with no default fails config load at boot.
