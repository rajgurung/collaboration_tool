# Deploying to Railway

The app ships as one Docker image (`Dockerfile`). Railway builds it from the repo using `railway.toml`.

## One-time setup

1. Create a Railway project and add a **PostgreSQL** service.
2. Add a service from this GitHub repo. Railway picks up `railway.toml` and builds the `Dockerfile`.
3. In the app service, set the variables below.
4. Deploy. Migrations run automatically when the app starts.
5. Create your super admin once. Either:

   - Open a shell in the running container (`railway ssh`, or the shell in the Railway dashboard) and run:

     ```sh
     SUPER_ADMIN_PASSWORD='choose-a-strong-one' /usr/app/collab-cli task super_admin
     ```

   - Or run it from this repo on your machine against the database's public URL:

     ```sh
     LOCO_ENV=production DATABASE_URL='<Postgres DATABASE_PUBLIC_URL>' \
       JWT_SECRET=x APP_URL=x MAIL_FROM=x MAILER_HOST=x MAILER_USER=x MAILER_PASSWORD=x \
       SUPER_ADMIN_PASSWORD='choose-a-strong-one' cargo loco task super_admin
     ```

   The exact Railway CLI commands are unverified; check `railway --help`. The task is safe to run again: it never changes an existing password.

## Variables

| Variable | Required | Example / notes |
|---|---|---|
| `DATABASE_URL` | yes | Reference the Postgres service: `${{Postgres.DATABASE_URL}}` |
| `JWT_SECRET` | yes | Must be valid base64. Generate with `openssl rand -base64 48`. A non-base64 value makes every sign-in fail. |
| `APP_URL` | yes | Public URL, no trailing slash, e.g. `https://collab.up.railway.app` |
| `MAIL_FROM` | yes | `Collaboration Tool <no-reply@yourdomain.com>`. Must be allowed by your mail provider. |
| `MAILER_HOST` | yes | SMTP host, e.g. `smtp.resend.com` |
| `MAILER_PORT` | no | Defaults to `587` (STARTTLS) |
| `MAILER_USER` | yes | SMTP user |
| `MAILER_PASSWORD` | yes | SMTP password or API key |
| `RESEND_API_KEY` | production | Sending-only Resend key. When set, email goes through Resend's HTTPS API instead of SMTP. |
| `SECRETS_KEK` | yes | The key-encryption key (KEK). It encrypts each organisation's own key, which in turn encrypts that organisation's login passwords. Base64 of 32 random bytes: `openssl rand -base64 32`. The app refuses to start without it, with a value that is not 32 bytes, or with the development key. Keep a copy offline, outside Railway: losing it loses every stored password. Never just replace it: follow [Rotating SECRETS_KEK](#rotating-secrets_kek). |
| `SECRETS_KEK_VERSION` | no | The version number of `SECRETS_KEK`, stored with each organisation key. Defaults to `1`. Raise it by one at each rotation. |
| `SECRETS_KEK_PREVIOUS` | rotation only | The old KEK while a rotation runs. Same rules as `SECRETS_KEK`, and it must differ from it. Remove it once no organisation key uses it. |
| `SECRETS_KEK_PREVIOUS_VERSION` | with `SECRETS_KEK_PREVIOUS` | The old KEK's version. Required when `SECRETS_KEK_PREVIOUS` is set, and must differ from `SECRETS_KEK_VERSION`. |
| `PORT` | set by Railway | The app binds `0.0.0.0:$PORT` |
| `SUPER_ADMIN_EMAIL` | no | Defaults to `gurungraj26@gmail.com` |
| `SUPER_ADMIN_USERNAME` | no | Defaults to `raj` |
| `SUPER_ADMIN_ORGANISATION` | no | Defaults to `Himalayan Ritual` |
| `SUPER_ADMIN_PASSWORD` | first run only | Only read by the `super_admin` task |
| `LOG_LEVEL` | no | Defaults to `info` |
| `DB_MAX_CONNECTIONS` | no | Defaults to `10` |

The app refuses to start if a required variable is missing.

## Rotating SECRETS_KEK

Each organisation's key is stored encrypted by the KEK. Rotating swaps the KEK
and re-encrypts (rewraps) those organisation keys. The logins themselves are
not touched. The steps below assume today's key is version 1.

1. Generate the new key with `openssl rand -base64 32`. Keep an offline copy,
   outside Railway, next to the copy of the old one.
2. Set all four variables together, in one change, so there is one deploy:
   - `SECRETS_KEK_PREVIOUS` = the old key (today's `SECRETS_KEK`)
   - `SECRETS_KEK_PREVIOUS_VERSION` = `1`
   - `SECRETS_KEK` = the new key
   - `SECRETS_KEK_VERSION` = `2`
3. Deploy. At boot, after migrations, the app rewraps every organisation key
   still on version 1 in one transaction, then logs:
   `rewrapped N organisation keys from v1 to v2; 0 left on v1`.
   If it fails, nothing changes, the log says so, and logins still open
   through the old key. Fix the cause and redeploy.
4. Confirm nothing is left on the old key. Either read that log line, or run
   `/usr/app/collab-cli task rewrap_org_keys dry_run:true` in the service
   (`cargo loco task rewrap_org_keys dry_run:true` locally). It prints counts
   only, for example `0 organisation keys on v1 (previous)`. Without
   `dry_run:true` the task rewraps first, the same way boot does.
5. Once it says 0, remove `SECRETS_KEK_PREVIOUS` and
   `SECRETS_KEK_PREVIOUS_VERSION`, and keep `SECRETS_KEK_VERSION` = `2`.
   Deploy. Keep the old offline copy until you have checked a saved login
   still opens.

The next rotation is the same with versions 2 and 3.

While a deploy overlaps the old instance, the old instance cannot open
organisation keys already moved to the new version. Logins added on the old
instance in that window get a key on the old version; the dry run in step 4
shows them, and a plain run of the task moves them.

**The limit.** This protects against a leaked KEK on its own. If the database
was also read, anyone with the old KEK and that copy can open every login
saved in it. Then each organisation key, and every login under it, must be re-encrypted
with new keys. This rotation does not do that.

## Custom domain (Cloudflare DNS)

Live at https://collab.zen-scale.io (Railway project `collaboration-tool`, service `web`). The original host, collab.rajgurung.me, now 308-redirects there via Cloudflare. Email still sends from `no-reply@rajgurung.me`, the domain verified in Resend.

1. `railway domain collab.zen-scale.io --service web` prints the CNAME target.
2. In Cloudflare add `CNAME collab -> <target>` as **DNS only** (grey cloud).
3. Railway also needs a TXT record `_railway-verify.<subdomain>` that the CLI does not print.
   Get it from the dashboard (service → Settings → Networking) or Railway's API
   (`customDomain { status { verificationDnsHost verificationToken } }`).
4. Wait for the certificate (a few minutes), then set `APP_URL` to the new address.
   After that, form posts from the old `*.up.railway.app` address are rejected.

Wrangler cannot manage DNS records; use the Cloudflare API, dashboard or the `cf` CLI.

## Logs and app addresses

App pages live at `/o/{slug}/...`. The org-path layer strips the prefix
before routing, so Loco's request logs show the stripped path (`/tasks/58`).
The slug is on a span of its own, `org{slug=acme-studio}`, around each such
request; dashboards or alerts keyed on paths need that field. Every response
sets `Referrer-Policy: same-origin`, and every response but the homepage
`X-Robots-Tag: noindex`. `/robots.txt` disallows `/o/`, `/oauth/`, `/admin`
and `/mcp`, and leaves `/join/` open so crawlers can see its `noindex`.

## Notes

- **One instance.** Live chat keeps WebSocket subscribers in memory, so run a single replica. Removal, sign out everywhere, password reset and the account lock also close live chat sockets and notification streams through in-memory events (`src/data/live.rs`). With a second replica, a removed person's open socket on the other instance would stay connected until it reconnects, so scaling out first needs a shared event bus.
- **Health check.** Railway checks `/_health`.
- **Email.** Railway blocks outbound SMTP (ports 25/465/587) below the Pro plan; sends fail with `Network is unreachable`. Production therefore uses Resend's HTTPS API (`RESEND_API_KEY`), sending from `no-reply@rajgurung.me` (domain verified in Resend; DKIM/SPF records in Cloudflare). The `MAILER_*` variables must still exist for the app to boot, but are unused while `RESEND_API_KEY` is set.
- **Cookies.** Production marks the session cookie `Secure`, so the site must be served over HTTPS (Railway domains are).
- **Form posts** are rejected if their `Origin` is not `APP_URL`. If you add a custom domain, update `APP_URL`.
- **Client IP.** Sign-in limits count failures per IP, read by Loco's `remote_ip` middleware from `X-Real-IP`, which Railway's edge sets ("for identifying client's remote IP"). Railway's docs say nothing about `X-Forwarded-For`, so it is not used. After a deploy that touches this, check it: sign in once with a wrong password while sending `X-Real-IP: 203.0.113.9` and `X-Forwarded-For: 203.0.113.9`, then look at the newest `auth_attempts` row. Its `ip` must be your real address. If it is 203.0.113.9, the header is not overwritten and the per-IP limit can be dodged. Putting Cloudflare's proxy in front would change this.

## Build and run the image locally

```sh
docker build -t collab:local .
docker run --rm -p 8080:8080 \
  -e PORT=8080 \
  -e DATABASE_URL=postgres://loco:loco@host.docker.internal:5433/collab_development \
  -e JWT_SECRET="$(openssl rand -base64 48)" -e APP_URL=http://localhost:8080 \
  -e MAIL_FROM='Collaboration Tool <no-reply@localhost>' \
  -e MAILER_HOST=host.docker.internal -e MAILER_USER=x -e MAILER_PASSWORD=x \
  collab:local
```
