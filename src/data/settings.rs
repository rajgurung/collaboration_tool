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
    /// Cloudflare R2, where production keeps uploaded documents. Development
    /// stores them on disk and tests in memory, so only production sets it.
    #[serde(default)]
    pub r2: Option<R2Settings>,
}

#[derive(Clone, Deserialize)]
pub struct R2Settings {
    pub bucket: String,
    /// `https://<account id>.r2.cloudflarestorage.com`
    pub endpoint: String,
    pub access_key_id: String,
    pub secret_access_key: String,
}

/// Never print the keys.
impl std::fmt::Debug for R2Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("R2Settings")
            .field("bucket", &self.bucket)
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
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
