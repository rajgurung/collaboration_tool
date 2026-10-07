# Auth decisions (Task 4)

- Session: Loco JWT (7 days) in an `auth_token` cookie: `HttpOnly`, `SameSite=Lax`,
  `Path=/`, `Secure` in production (`settings.secure_cookies`). Loco reads it via
  `auth.jwt.location: { from: Cookie, name: auth_token }`.
- `CurrentUser` extractor (`src/extractors/current_user.rs`) redirects anonymous
  requests to `/login` (303, or `HX-Redirect` for HTMX requests).
- CSRF: `SameSite=Lax` keeps the cookie off cross-site POSTs. As a second layer,
  `OriginCheckInitializer` rejects any non-GET request whose `Origin` header is not
  `settings.app_url` (403). We chose this over form tokens: no per-form plumbing, and
  every current browser sends `Origin` on POST. Consequence in development: use
  `http://localhost:5150`, not `127.0.0.1`, or posts are rejected.
- Passwords: at least 8 characters (Argon2 via `loco_rs::hash`).
- Usernames: letter first, then letters or numbers, 3 to 30 characters.
- Emails are trimmed and lowercased on save and lookup.
- Login gives the same message and timing for unknown email and wrong password.
- Forgot password always shows the same confirmation. Reset links expire after 60
  minutes and are cleared after one use. Tokens are stored as plain UUIDv4 (from the
  Loco starter); hashing them at rest is a possible later hardening.
- Removed from the starter: JSON auth API, magic links, welcome/verification email.
  Email verification model methods remain in case we turn verification on.
- Not done yet: login rate limiting.
- Local mail: Mailpit in Docker (`collab-mailpit`), SMTP on 1025, inbox at
  http://localhost:8025.
