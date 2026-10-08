//! Tokens that let Claude act as one person in their organisation: OAuth
//! access tokens (with a refresh token) and personal access tokens. Only
//! SHA-256 hashes are stored; the plain value is shown once.
use std::fmt::Write as _;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use loco_rs::prelude::*;
use rand::RngCore;
use sea_orm::sea_query::{ExprTrait, Func};
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub use super::_entities::access_tokens::{ActiveModel, Column, Entity, Model};
use super::{field_error, memberships, oauth_clients, oauth_codes};

pub type AccessTokens = Entity;

pub mod kind {
    pub const PERSONAL: &str = "personal";
    pub const OAUTH: &str = "oauth";
}

/// The one scope: read and change projects, tasks, assignees and task notes.
pub const SCOPE: &str = "tasks";
pub const PERSONAL_PREFIX: &str = "collab_pat_";
const ACCESS_PREFIX: &str = "collab_at_";
const REFRESH_PREFIX: &str = "collab_rt_";
pub const ACCESS_SECONDS: i64 = 60 * 60;
const REFRESH_DAYS: i64 = 30;
pub const MAX_PERSONAL: u64 = 10;
/// Revoked or expired rows are kept this long, then deleted.
const KEEP_DAYS: i64 = 30;
/// `last_used_at` is written at most this often per token.
const TOUCH_SECONDS: i64 = 60;

/// A new secret: `prefix` then 32 random bytes as base64url.
#[must_use]
pub fn generate(prefix: &str) -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes))
}

/// The stored form of a secret: SHA-256 as hex. Secrets are high-entropy, so
/// looking a row up by this hash needs no salt or constant-time compare.
#[must_use]
pub fn hash(secret: &str) -> String {
    Sha256::digest(secret.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// Whether a space-separated scope list includes ours.
#[must_use]
pub fn has_scope(scope: &str) -> bool {
    scope.split_whitespace().any(|s| s == SCOPE)
}

/// The personal token form.
#[derive(Debug, Deserialize, Validate)]
pub struct PersonalParams {
    #[validate(length(min = 1, max = 60, message = "Name the token (up to 60 characters)."))]
    pub name: String,
}

/// Tokens handed to an OAuth client.
pub struct Issued {
    pub access_token: String,
    pub refresh_token: String,
    pub scope: String,
}

/// A token request with `grant_type=authorization_code`.
pub struct CodeExchange<'a> {
    pub code: &'a str,
    pub code_verifier: &'a str,
    pub redirect_uri: &'a str,
    pub resource: &'a str,
    pub client: &'a oauth_clients::Model,
}

/// One connection on the settings page: a personal token, or an OAuth grant
/// however many times its refresh token has rotated.
pub struct Connection {
    pub grant_id: Uuid,
    pub name: String,
    pub kind: String,
    pub created_at: DateTimeWithTimeZone,
    pub last_used_at: Option<DateTimeWithTimeZone>,
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
    /// The token row for a bearer token, if it is live, for `resource`, and
    /// carries our scope.
    ///
    /// # Errors
    /// On database errors.
    pub async fn find_usable<C: ConnectionTrait>(
        db: &C,
        token: &str,
        resource: &str,
    ) -> ModelResult<Option<Self>> {
        let now = chrono::Utc::now();
        Ok(Entity::find()
            .filter(Column::TokenHash.eq(hash(token)))
            .one(db)
            .await?
            .filter(|t| {
                t.revoked_at.is_none()
                    && t.expires_at.is_none_or(|at| at > now)
                    && t.resource == resource
                    && has_scope(&t.scope)
            }))
    }

    /// Makes a personal access token. Returns the row and the plain token,
    /// which is never stored.
    ///
    /// # Errors
    /// Validation errors (including the 10-token limit), or database errors.
    pub async fn create_personal(
        db: &DatabaseConnection,
        org_id: i64,
        user_id: i64,
        params: &PersonalParams,
        resource: &str,
    ) -> ModelResult<(Self, String)> {
        let name = params.name.trim().to_string();
        ValidatorTrait::validate(&PersonalParams { name: name.clone() })?;
        let txn = db.begin().await?;
        let active = Entity::find()
            .filter(Column::OrganisationId.eq(org_id))
            .filter(Column::UserId.eq(user_id))
            .filter(Column::Kind.eq(kind::PERSONAL))
            .filter(Column::RevokedAt.is_null())
            .count(&txn)
            .await?;
        if active >= MAX_PERSONAL {
            return Err(field_error(
                "name",
                "You can have up to 10 personal tokens. Revoke one first.",
            ));
        }
        let token = generate(PERSONAL_PREFIX);
        let row = ActiveModel {
            kind: ActiveValue::Set(kind::PERSONAL.to_string()),
            name: ActiveValue::Set(name),
            token_hash: ActiveValue::Set(hash(&token)),
            grant_id: ActiveValue::Set(Uuid::new_v4()),
            organisation_id: ActiveValue::Set(org_id),
            user_id: ActiveValue::Set(user_id),
            resource: ActiveValue::Set(resource.to_string()),
            scope: ActiveValue::Set(SCOPE.to_string()),
            ..Default::default()
        }
        .insert(&txn)
        .await?;
        txn.commit().await?;
        Ok((row, token))
    }

    /// Swaps an authorization code for tokens. The code is spent first, so it
    /// works once whatever happens next. `None` when the code is unknown,
    /// used or expired, or does not match the client, redirect URI, resource
    /// or PKCE verifier it was issued for.
    ///
    /// Spending the code and saving the tokens share a transaction: a second
    /// request for the same code waits on the row lock until the tokens
    /// exist, so revoking the grant on reuse always catches them.
    ///
    /// # Errors
    /// On database errors.
    pub async fn exchange_code(
        db: &DatabaseConnection,
        exchange: &CodeExchange<'_>,
    ) -> ModelResult<Option<Issued>> {
        let txn = db.begin().await?;
        let Some(code) = oauth_codes::Model::redeem(&txn, exchange.code).await? else {
            txn.commit().await?;
            return Ok(None);
        };
        let matches = code.oauth_client_id == exchange.client.id
            && code.redirect_uri == exchange.redirect_uri
            && code.resource == exchange.resource
            && code.verifier_matches(exchange.code_verifier)
            && memberships::Model::is_active_member(&txn, code.organisation_id, code.user_id)
                .await?;
        if !matches {
            txn.commit().await?;
            return Ok(None);
        }
        let issued = issue(
            &txn,
            &Grant {
                grant_id: code.grant_id,
                name: &exchange.client.client_name,
                oauth_client_id: exchange.client.id,
                org_id: code.organisation_id,
                user_id: code.user_id,
                resource: &code.resource,
                scope: &code.scope,
            },
        )
        .await?;
        txn.commit().await?;
        Ok(Some(issued))
    }

    /// Swaps a refresh token for new tokens, in one statement, so it works
    /// once. `None` when the token is unknown, expired, for another client or
    /// resource, or the person is no longer an active member. Using a token
    /// that was already swapped revokes the whole grant.
    ///
    /// # Errors
    /// On database errors.
    pub async fn refresh(
        db: &DatabaseConnection,
        refresh_token: &str,
        client: &oauth_clients::Model,
        resource: &str,
    ) -> ModelResult<Option<Issued>> {
        let hashed = hash(refresh_token);
        let now = chrono::Utc::now();
        let txn = db.begin().await?;
        let mut rotated = Entity::update_many()
            .col_expr(Column::RevokedAt, Expr::value(now))
            .col_expr(Column::UpdatedAt, Expr::value(now))
            .filter(Column::RefreshHash.eq(&hashed))
            .filter(Column::RevokedAt.is_null())
            .filter(Column::RefreshExpiresAt.gt(now))
            .filter(Column::OauthClientId.eq(client.id))
            .filter(Column::Resource.eq(resource))
            .exec_with_returning(&txn)
            .await?;
        let Some(old) = rotated.pop() else {
            txn.rollback().await?;
            let reused = Entity::find()
                .filter(Column::RefreshHash.eq(&hashed))
                .filter(Column::RevokedAt.is_not_null())
                .one(db)
                .await?;
            if let Some(row) = reused {
                tracing::warn!(grant = %row.grant_id, "refresh token reused; revoking grant");
                Self::revoke_grant(db, row.grant_id).await?;
            }
            return Ok(None);
        };
        if !memberships::Model::is_active_member(&txn, old.organisation_id, old.user_id).await? {
            txn.commit().await?;
            Self::revoke_grant(db, old.grant_id).await?;
            return Ok(None);
        }
        let issued = issue(
            &txn,
            &Grant {
                grant_id: old.grant_id,
                name: &old.name,
                oauth_client_id: client.id,
                org_id: old.organisation_id,
                user_id: old.user_id,
                resource: &old.resource,
                scope: &old.scope,
            },
        )
        .await?;
        txn.commit().await?;
        Ok(Some(issued))
    }

    /// Revokes every token in a grant and spends its unused codes.
    ///
    /// # Errors
    /// On database errors.
    pub async fn revoke_grant<C: ConnectionTrait>(db: &C, grant_id: Uuid) -> ModelResult<()> {
        let now = chrono::Utc::now();
        Entity::update_many()
            .col_expr(Column::RevokedAt, Expr::value(now))
            .col_expr(Column::UpdatedAt, Expr::value(now))
            .filter(Column::GrantId.eq(grant_id))
            .filter(Column::RevokedAt.is_null())
            .exec(db)
            .await?;
        oauth_codes::Entity::update_many()
            .col_expr(oauth_codes::Column::UsedAt, Expr::value(now))
            .filter(oauth_codes::Column::GrantId.eq(grant_id))
            .filter(oauth_codes::Column::UsedAt.is_null())
            .exec(db)
            .await?;
        Ok(())
    }

    /// Revokes one of this person's own grants.
    ///
    /// # Errors
    /// `EntityNotFound` when the grant is not theirs, or database errors.
    pub async fn revoke_mine<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
        grant_id: Uuid,
    ) -> ModelResult<()> {
        Entity::find()
            .filter(Column::OrganisationId.eq(org_id))
            .filter(Column::UserId.eq(user_id))
            .filter(Column::GrantId.eq(grant_id))
            .one(db)
            .await?
            .ok_or(ModelError::EntityNotFound)?;
        Self::revoke_grant(db, grant_id).await
    }

    /// This person's live connections, newest first.
    ///
    /// # Errors
    /// On database errors.
    pub async fn connections<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
    ) -> ModelResult<Vec<Connection>> {
        let now = chrono::Utc::now();
        let rows = Entity::find()
            .filter(Column::OrganisationId.eq(org_id))
            .filter(Column::UserId.eq(user_id))
            .order_by_asc(Column::CreatedAt)
            .order_by_asc(Column::Id)
            .all(db)
            .await?;
        let live = |t: &Self| {
            t.revoked_at.is_none()
                && (t.kind == kind::PERSONAL || t.refresh_expires_at.is_some_and(|at| at > now))
        };
        let mut connections: Vec<Connection> = Vec::new();
        for row in rows.iter().filter(|t| live(t)) {
            if connections.iter().any(|c| c.grant_id == row.grant_id) {
                continue;
            }
            let grant = rows.iter().filter(|t| t.grant_id == row.grant_id);
            connections.push(Connection {
                grant_id: row.grant_id,
                name: row.name.clone(),
                kind: row.kind.clone(),
                created_at: grant
                    .clone()
                    .map(|t| t.created_at)
                    .min()
                    .unwrap_or(row.created_at),
                last_used_at: grant.filter_map(|t| t.last_used_at).max(),
            });
        }
        connections.reverse();
        Ok(connections)
    }

    /// Records use, at most once a minute, so busy tokens do not write on
    /// every call.
    ///
    /// # Errors
    /// On database errors.
    pub async fn touch<C: ConnectionTrait>(&self, db: &C) -> ModelResult<()> {
        let now = chrono::Utc::now();
        if self
            .last_used_at
            .is_some_and(|at| (now - at.with_timezone(&chrono::Utc)).num_seconds() < TOUCH_SECONDS)
        {
            return Ok(());
        }
        Entity::update_many()
            .col_expr(Column::LastUsedAt, Expr::value(now))
            .filter(Column::Id.eq(self.id))
            .exec(db)
            .await?;
        Ok(())
    }

    /// Deletes tokens revoked or expired more than 30 days ago, and old codes.
    ///
    /// # Errors
    /// On database errors.
    pub async fn prune<C: ConnectionTrait>(db: &C) -> ModelResult<()> {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(KEEP_DAYS);
        let expiry = Func::coalesce([
            Expr::col(Column::RefreshExpiresAt),
            Expr::col(Column::ExpiresAt),
        ]);
        Entity::delete_many()
            .filter(
                Condition::any()
                    .add(Column::RevokedAt.lt(cutoff))
                    .add(Expr::expr(expiry).lt(cutoff)),
            )
            .exec(db)
            .await?;
        oauth_codes::Model::prune(db).await
    }
}

struct Grant<'a> {
    grant_id: Uuid,
    name: &'a str,
    oauth_client_id: i64,
    org_id: i64,
    user_id: i64,
    resource: &'a str,
    scope: &'a str,
}

async fn issue<C: ConnectionTrait>(db: &C, grant: &Grant<'_>) -> ModelResult<Issued> {
    let access_token = generate(ACCESS_PREFIX);
    let refresh_token = generate(REFRESH_PREFIX);
    let now = chrono::Utc::now();
    ActiveModel {
        kind: ActiveValue::Set(kind::OAUTH.to_string()),
        name: ActiveValue::Set(grant.name.to_string()),
        token_hash: ActiveValue::Set(hash(&access_token)),
        refresh_hash: ActiveValue::Set(Some(hash(&refresh_token))),
        grant_id: ActiveValue::Set(grant.grant_id),
        oauth_client_id: ActiveValue::Set(Some(grant.oauth_client_id)),
        organisation_id: ActiveValue::Set(grant.org_id),
        user_id: ActiveValue::Set(grant.user_id),
        resource: ActiveValue::Set(grant.resource.to_string()),
        scope: ActiveValue::Set(grant.scope.to_string()),
        expires_at: ActiveValue::Set(Some(
            (now + chrono::Duration::seconds(ACCESS_SECONDS)).into(),
        )),
        refresh_expires_at: ActiveValue::Set(Some(
            (now + chrono::Duration::days(REFRESH_DAYS)).into(),
        )),
        ..Default::default()
    }
    .insert(db)
    .await?;
    Ok(Issued {
        access_token,
        refresh_token,
        scope: grant.scope.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::{generate, has_scope, hash, PERSONAL_PREFIX};

    #[test]
    fn tokens_are_prefixed_random_and_long() {
        let a = generate(PERSONAL_PREFIX);
        let b = generate(PERSONAL_PREFIX);
        assert!(a.starts_with("collab_pat_"));
        assert_eq!(a.len(), "collab_pat_".len() + 43);
        assert_ne!(a, b);
    }

    #[test]
    fn hashes_are_sha256_hex() {
        assert_eq!(
            hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn scope_lists_are_split_on_spaces() {
        assert!(has_scope("tasks"));
        assert!(has_scope("openid tasks"));
        assert!(!has_scope("tasksx"));
        assert!(!has_scope(""));
    }
}
