use loco_rs::prelude::*;
use serde::Deserialize;

/// Typed view of the `settings:` block in `config/<env>.yaml`.
#[derive(Debug, Clone, Deserialize)]
pub struct Settings {
    /// Public URL of the app, e.g. `https://collab.example.com`. No trailing slash.
    pub app_url: String,
    /// Whether the session cookie is marked `Secure`.
    pub secure_cookies: bool,
    /// Sender for outgoing email, e.g. `Collaboration Tool <no-reply@example.com>`.
    pub mail_from: String,
    /// When set, email goes out through Resend's HTTPS API instead of SMTP.
    #[serde(default)]
    pub resend_api_key: String,
    /// Used by `cargo loco task super_admin`.
    pub super_admin: SuperAdminSettings,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SuperAdminSettings {
    pub email: String,
    pub username: String,
    /// The super admin's own organisation, created if they have none.
    pub organisation: String,
    /// Only needed the first time, to create the account. Empty when unset.
    #[serde(default)]
    pub password: String,
}

impl Settings {
    /// # Errors
    /// When `settings:` is missing or does not match this struct.
    pub fn from_context(ctx: &AppContext) -> Result<Self> {
        let mut settings: Self = ctx.config.settings()?;
        settings.app_url = settings.app_url.trim_end_matches('/').to_string();
        Ok(settings)
    }

    /// The MCP endpoint, which is also the OAuth resource Claude's tokens are for.
    #[must_use]
    pub fn mcp_url(&self) -> String {
        format!("{}/mcp", self.app_url)
    }
}
