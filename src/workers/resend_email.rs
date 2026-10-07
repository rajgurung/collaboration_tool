//! Sends one email through Resend's HTTPS API. Used in production because
//! Railway blocks outbound SMTP below the Pro plan.
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};

use crate::data::settings::Settings;

const ENDPOINT: &str = "https://api.resend.com/emails";

pub struct Worker {
    pub ctx: AppContext,
}

/// A fully rendered email.
#[derive(Deserialize, Debug, Serialize, Clone, PartialEq, Eq)]
pub struct WorkerArgs {
    pub from: String,
    pub to: String,
    pub subject: String,
    pub text: String,
    pub html: String,
}

#[async_trait]
impl BackgroundWorker<WorkerArgs> for Worker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }

    fn class_name() -> String {
        "ResendEmail".to_string()
    }

    async fn perform(&self, args: WorkerArgs) -> Result<()> {
        let key = Settings::from_context(&self.ctx)?.resend_api_key;
        if key.is_empty() {
            return Err(Error::string("RESEND_API_KEY is not set"));
        }
        let response = reqwest::Client::new()
            .post(ENDPOINT)
            .bearer_auth(key)
            .json(&serde_json::json!({
                "from": args.from,
                "to": [args.to],
                "subject": args.subject,
                "text": args.text,
                "html": args.html,
            }))
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await
            .map_err(|e| Error::string(&format!("resend request failed: {e}")))?;
        let status = response.status();
        if status.is_success() {
            tracing::info!(subject = args.subject, "email sent through Resend");
            Ok(())
        } else {
            let body = response.text().await.unwrap_or_default();
            tracing::error!(%status, body, "Resend rejected the email");
            Err(Error::string(&format!("resend returned {status}")))
        }
    }
}
