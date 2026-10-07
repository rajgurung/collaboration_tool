use loco_rs::prelude::*;
use serde::Deserialize;

/// Typed view of the `settings:` block in `config/<env>.yaml`.
#[derive(Debug, Clone, Deserialize)]
pub struct Settings {
    /// Public URL of the app, e.g. `https://collab.example.com`. No trailing slash.
    pub app_url: String,
    /// Whether the session cookie is marked `Secure`.
    pub secure_cookies: bool,
}

impl Settings {
    /// # Errors
    /// When `settings:` is missing or does not match this struct.
    pub fn from_context(ctx: &AppContext) -> Result<Self> {
        let mut settings: Self = ctx.config.settings()?;
        settings.app_url = settings.app_url.trim_end_matches('/').to_string();
        Ok(settings)
    }
}
