pub mod auth;
pub mod mentions;

use loco_rs::prelude::*;

use crate::{
    data::settings::Settings,
    workers::resend_email::{Worker as ResendEmail, WorkerArgs as ResendEmailArgs},
};

/// Loco's SMTP mailer, used when no Resend key is set.
struct Outbox {}
impl Mailer for Outbox {}

/// Sends through Resend's HTTPS API when a key is configured, otherwise
/// through Loco's SMTP mailer (development uses Mailpit, tests use the stub).
pub(crate) async fn deliver(
    ctx: &AppContext,
    settings: &Settings,
    dir: &Dir<'_>,
    args: mailer::Args,
) -> Result<()> {
    if settings.resend_api_key.is_empty() {
        return Outbox::mail_template(ctx, dir, args).await;
    }
    let email = render(dir, &args)?;
    ResendEmail::perform_later(ctx, email).await?;
    Ok(())
}

/// Renders `subject.t`, `text.t` and `html.t` from a mailer template folder.
pub(crate) fn render(dir: &Dir<'_>, args: &mailer::Args) -> Result<ResendEmailArgs> {
    let context =
        tera::Context::from_serialize(&args.locals).map_err(|e| Error::string(&e.to_string()))?;
    let part = |name: &str, autoescape: bool| -> Result<String> {
        let source = dir
            .get_file(name)
            .and_then(|f| f.contents_utf8())
            .ok_or_else(|| Error::string(&format!("missing mailer template {name}")))?;
        tera::Tera::one_off(source, &context, autoescape).map_err(|e| Error::string(&e.to_string()))
    };
    Ok(ResendEmailArgs {
        from: args.from.clone().unwrap_or_default(),
        to: args.to.clone(),
        subject: part("subject.t", false)?.trim().to_string(),
        text: part("text.t", false)?,
        html: part("html.t", true)?,
    })
}
