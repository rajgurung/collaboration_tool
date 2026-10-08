//! Authorization codes: made when someone presses Allow, swapped once for
//! tokens. Stored as a hash, live 10 minutes.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use loco_rs::prelude::*;
use sha2::{Digest, Sha256};

pub use super::_entities::oauth_codes::{ActiveModel, Column, Entity, Model};
use super::{access_tokens, oauth_clients};

pub type OauthCodes = Entity;

const LIFETIME_MINUTES: i64 = 10;
/// Old codes (used or not) are deleted after this.
const KEEP_HOURS: i64 = 24;

/// What the person agreed to on the consent page.
pub struct Consent<'a> {
    pub client: &'a oauth_clients::Model,
    pub org_id: i64,
    pub user_id: i64,
    pub redirect_uri: &'a str,
    pub code_challenge: &'a str,
    pub resource: &'a str,
    pub scope: &'a str,
}

/// A PKCE challenge is the base64url SHA-256 of a verifier: always 43 characters.
#[must_use]
pub fn is_challenge(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// RFC 7636: 43 to 128 unreserved characters.
fn is_verifier(value: &str) -> bool {
    (43..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
}

/// The S256 challenge for a verifier.
#[must_use]
pub fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
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
    /// Starts a new grant and returns its code. The plain code is only ever
    /// in the return value.
    ///
    /// # Errors
    /// On database errors.
    pub async fn issue<C: ConnectionTrait>(
        db: &C,
        consent: &Consent<'_>,
    ) -> ModelResult<(Self, String)> {
        let code = access_tokens::generate("collab_code_");
        let row = ActiveModel {
            code_hash: ActiveValue::Set(access_tokens::hash(&code)),
            grant_id: ActiveValue::Set(Uuid::new_v4()),
            oauth_client_id: ActiveValue::Set(consent.client.id),
            organisation_id: ActiveValue::Set(consent.org_id),
            user_id: ActiveValue::Set(consent.user_id),
            redirect_uri: ActiveValue::Set(consent.redirect_uri.to_string()),
            code_challenge: ActiveValue::Set(consent.code_challenge.to_string()),
            resource: ActiveValue::Set(consent.resource.to_string()),
            scope: ActiveValue::Set(consent.scope.to_string()),
            expires_at: ActiveValue::Set(
                (chrono::Utc::now() + chrono::Duration::minutes(LIFETIME_MINUTES)).into(),
            ),
            ..Default::default()
        }
        .insert(db)
        .await?;
        Ok((row, code))
    }

    /// Marks the code used, in one statement, and returns it. `None` when the
    /// code is unknown, expired or already used. A second use of a code
    /// revokes everything issued from it.
    ///
    /// # Errors
    /// On database errors.
    pub async fn redeem<C: ConnectionTrait>(db: &C, code: &str) -> ModelResult<Option<Self>> {
        let hash = access_tokens::hash(code);
        let now = chrono::Utc::now();
        let mut used = Entity::update_many()
            .col_expr(Column::UsedAt, Expr::value(now))
            .filter(Column::CodeHash.eq(&hash))
            .filter(Column::UsedAt.is_null())
            .filter(Column::ExpiresAt.gt(now))
            .exec_with_returning(db)
            .await?;
        if let Some(row) = used.pop() {
            return Ok(Some(row));
        }
        let reused = Entity::find()
            .filter(Column::CodeHash.eq(&hash))
            .filter(Column::UsedAt.is_not_null())
            .one(db)
            .await?;
        if let Some(row) = reused {
            tracing::warn!(grant = %row.grant_id, "authorization code reused; revoking grant");
            access_tokens::Model::revoke_grant(db, row.grant_id).await?;
        }
        Ok(None)
    }

    /// Whether `verifier` is the PKCE verifier this code was issued for.
    #[must_use]
    pub fn verifier_matches(&self, verifier: &str) -> bool {
        is_verifier(verifier) && challenge_for(verifier) == self.code_challenge
    }

    /// Deletes codes older than a day.
    ///
    /// # Errors
    /// On database errors.
    pub async fn prune<C: ConnectionTrait>(db: &C) -> ModelResult<()> {
        let cutoff = chrono::Utc::now() - chrono::Duration::hours(KEEP_HOURS);
        Entity::delete_many()
            .filter(Column::CreatedAt.lt(cutoff))
            .exec(db)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{challenge_for, is_challenge, is_verifier};

    #[test]
    fn s256_matches_the_rfc_example() {
        // RFC 7636, appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = challenge_for(verifier);
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert!(is_challenge(&challenge));
    }

    #[test]
    fn verifiers_must_be_43_to_128_unreserved_characters() {
        assert!(!is_verifier("short"));
        assert!(!is_verifier(&"a".repeat(129)));
        assert!(!is_verifier(&format!("{}!", "a".repeat(42))));
        assert!(is_verifier(&"a".repeat(43)));
    }
}
