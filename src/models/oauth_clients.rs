//! OAuth clients that registered themselves (RFC 7591), such as claude.ai or
//! Claude Code. Every client is public: no secret, PKCE instead.
use loco_rs::prelude::*;
use serde::Deserialize;

pub use super::_entities::oauth_clients::{ActiveModel, Column, Entity, Model};
use super::access_tokens;

pub type OauthClients = Entity;

pub const MAX_NAME: usize = 100;
pub const MAX_REDIRECT_URIS: usize = 5;
/// A client that never got as far as a code is removed after this. Used
/// clients are kept: claude.ai may cache its client id.
const UNUSED_CLIENT_DAYS: i64 = 7;

/// The only places Claude sends people back to after "Allow". Hosted Claude
/// uses one fixed callback; Claude Code listens on loopback on any port.
pub const HOSTED_CALLBACKS: [(&str, &str); 2] = [
    ("claude.ai", "/api/mcp/auth_callback"),
    ("claude.com", "/api/mcp/auth_callback"),
];
const LOOPBACK_HOSTS: [&str; 2] = ["localhost", "127.0.0.1"];
const LOOPBACK_PATH: &str = "/callback";

/// A redirect URI from the allowlist, parsed. Loopback ones may use any port.
#[derive(Debug, PartialEq, Eq)]
enum Callback {
    Hosted(String),
    Loopback { host: String, port: Option<u16> },
}

/// Parses `raw` and accepts it only if it is on the allowlist. Matching is on
/// the parsed URL, never on string prefixes, and the raw text must already be
/// in its normal form (so `/callback/../x` and similar tricks are refused).
fn callback(raw: &str) -> Option<Callback> {
    let url = url::Url::parse(raw).ok()?;
    if url.as_str() != raw
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let host = url.host_str()?;
    if url.scheme() == "https" && url.port().is_none() {
        return HOSTED_CALLBACKS
            .iter()
            .any(|(h, path)| *h == host && *path == url.path())
            .then(|| Callback::Hosted(host.to_string()));
    }
    if url.scheme() == "http" && LOOPBACK_HOSTS.contains(&host) && url.path() == LOOPBACK_PATH {
        return Some(Callback::Loopback {
            host: host.to_string(),
            port: url.port(),
        });
    }
    None
}

/// Whether `raw` is a redirect URI Collab Tool will ever send a code to.
#[must_use]
pub fn is_allowed_redirect(raw: &str) -> bool {
    callback(raw).is_some()
}

/// The host people are sent back to, for the consent page ("claude.ai").
#[must_use]
pub fn redirect_host(raw: &str) -> String {
    url::Url::parse(raw)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default()
}

/// The registration request. Unknown fields (and the requested auth method)
/// are ignored: every client is registered as public.
#[derive(Debug, Deserialize)]
pub struct RegisterParams {
    #[serde(default)]
    pub client_name: Option<String>,
    #[serde(default)]
    pub redirect_uris: Vec<String>,
}

/// Why a registration was refused, as RFC 7591 error codes.
#[derive(Debug)]
pub enum RegisterError {
    RedirectUri(&'static str),
    Metadata(&'static str),
    Db(ModelError),
}

impl From<ModelError> for RegisterError {
    fn from(err: ModelError) -> Self {
        Self::Db(err)
    }
}

impl From<DbErr> for RegisterError {
    fn from(err: DbErr) -> Self {
        Self::Db(err.into())
    }
}

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        if !insert && self.updated_at.is_unchanged() {
            let mut this = self;
            this.updated_at = sea_orm::ActiveValue::Set(chrono::Utc::now().into());
            Ok(this)
        } else {
            Ok(self)
        }
    }
}

impl Model {
    /// Registers a public client. Also clears out clients that registered
    /// more than a week ago and never got a code.
    ///
    /// # Errors
    /// `RedirectUri` for a missing or non-allowlisted redirect URI, `Metadata`
    /// for a name that is too long, or database errors.
    pub async fn register<C: ConnectionTrait>(
        db: &C,
        params: &RegisterParams,
    ) -> std::result::Result<Self, RegisterError> {
        let name = params
            .client_name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or("Claude");
        if name.chars().count() > MAX_NAME {
            return Err(RegisterError::Metadata(
                "client_name is longer than 100 characters",
            ));
        }
        if params.redirect_uris.is_empty() {
            return Err(RegisterError::RedirectUri("redirect_uris is required"));
        }
        if params.redirect_uris.len() > MAX_REDIRECT_URIS {
            return Err(RegisterError::RedirectUri("at most 5 redirect_uris"));
        }
        if !params.redirect_uris.iter().all(|u| is_allowed_redirect(u)) {
            return Err(RegisterError::RedirectUri(
                "redirect_uris must be Claude's callback or a loopback /callback",
            ));
        }
        Self::prune(db).await?;
        Ok(ActiveModel {
            client_id: ActiveValue::Set(access_tokens::generate("collab_client_")),
            client_name: ActiveValue::Set(name.to_string()),
            redirect_uris: ActiveValue::Set(serde_json::json!(params.redirect_uris)),
            ..Default::default()
        }
        .insert(db)
        .await?)
    }

    /// # Errors
    /// `EntityNotFound` for an unknown client id.
    pub async fn find_by_client_id<C: ConnectionTrait>(
        db: &C,
        client_id: &str,
    ) -> ModelResult<Self> {
        Entity::find()
            .filter(Column::ClientId.eq(client_id))
            .one(db)
            .await?
            .ok_or(ModelError::EntityNotFound)
    }

    /// Whether `raw` is one of this client's redirect URIs. Loopback ones
    /// match on any port (RFC 8252), since Claude Code picks a free port.
    #[must_use]
    pub fn accepts_redirect(&self, raw: &str) -> bool {
        let Some(given) = callback(raw) else {
            return false;
        };
        self.redirect_uris
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r.as_str().and_then(callback))
            .any(|registered| match (&registered, &given) {
                (Callback::Loopback { host: a, .. }, Callback::Loopback { host: b, .. }) => a == b,
                _ => registered == given,
            })
    }

    /// Records that the client got its first code, so it is never pruned.
    ///
    /// # Errors
    /// On database errors.
    pub async fn mark_used<C: ConnectionTrait>(&self, db: &C) -> ModelResult<()> {
        if self.used_at.is_none() {
            Entity::update_many()
                .col_expr(Column::UsedAt, Expr::value(chrono::Utc::now()))
                .filter(Column::Id.eq(self.id))
                .filter(Column::UsedAt.is_null())
                .exec(db)
                .await?;
        }
        Ok(())
    }

    async fn prune<C: ConnectionTrait>(db: &C) -> ModelResult<()> {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(UNUSED_CLIENT_DAYS);
        Entity::delete_many()
            .filter(Column::CreatedAt.lt(cutoff))
            .filter(Column::UsedAt.is_null())
            .exec(db)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::is_allowed_redirect;

    #[test]
    fn allows_claude_and_loopback_callbacks() {
        for ok in [
            "https://claude.ai/api/mcp/auth_callback",
            "https://claude.com/api/mcp/auth_callback",
            "http://localhost:33418/callback",
            "http://127.0.0.1:5000/callback",
            "http://localhost/callback",
        ] {
            assert!(is_allowed_redirect(ok), "{ok} should be allowed");
        }
    }

    #[test]
    fn refuses_lookalikes() {
        for bad in [
            "http://localhost.evil.com/callback",
            "http://localhost:1@evil.com/callback",
            "https://claude.ai.evil.com/api/mcp/auth_callback",
            "http://localhost/callback/../x",
            "http://claude.ai/api/mcp/auth_callback",
            "https://claude.ai:8443/api/mcp/auth_callback",
            "https://claude.ai/api/mcp/auth_callback?x=1",
            "https://claude.ai/api/mcp/auth_callback#x",
            "https://user@claude.ai/api/mcp/auth_callback",
            "https://localhost/callback",
            "http://localhost:3000/callback/",
            "http://LOCALHOST/callback",
            "http://[::1]:3000/callback",
            "http://evil.com/callback",
            "javascript:alert(1)",
            "",
        ] {
            assert!(!is_allowed_redirect(bad), "{bad} should be refused");
        }
    }
}
