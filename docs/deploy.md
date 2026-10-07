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
| `PORT` | set by Railway | The app binds `0.0.0.0:$PORT` |
| `SUPER_ADMIN_EMAIL` | no | Defaults to `gurungraj26@gmail.com` |
| `SUPER_ADMIN_USERNAME` | no | Defaults to `raj` |
| `SUPER_ADMIN_ORGANISATION` | no | Defaults to `Himalayan Ritual` |
| `SUPER_ADMIN_PASSWORD` | first run only | Only read by the `super_admin` task |
| `LOG_LEVEL` | no | Defaults to `info` |
| `DB_MAX_CONNECTIONS` | no | Defaults to `10` |

The app refuses to start if a required variable is missing.

## Custom domain (Cloudflare DNS)

Live at https://collab.rajgurung.me (Railway project `collaboration-tool`, service `web`).

1. `railway domain collab.rajgurung.me --service web` prints the CNAME target.
2. In Cloudflare add `CNAME collab -> <target>` as **DNS only** (grey cloud).
3. Railway also needs a TXT record `_railway-verify.<subdomain>` that the CLI does not print.
   Get it from the dashboard (service → Settings → Networking) or Railway's API
   (`customDomain { status { verificationDnsHost verificationToken } }`).
4. Wait for the certificate (a few minutes), then set `APP_URL` to the new address.
   After that, form posts from the old `*.up.railway.app` address are rejected.

Wrangler cannot manage DNS records; use the Cloudflare API, dashboard or the `cf` CLI.

## Notes

- **One instance.** Live chat keeps WebSocket subscribers in memory, so run a single replica.
- **Health check.** Railway checks `/_health`.
- **Email.** Outbound SMTP may be blocked on some Railway plans (unverified). If emails do not arrive, check the logs for SMTP errors and use a provider on port 587 or 2587.
- **Cookies.** Production marks the session cookie `Secure`, so the site must be served over HTTPS (Railway domains are).
- **Form posts** are rejected if their `Origin` is not `APP_URL`. If you add a custom domain, update `APP_URL`.

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
