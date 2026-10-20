use async_trait::async_trait;
use chrono::{offset::Local, Duration};
use loco_rs::{auth::jwt, hash, prelude::*};
use sea_orm::{sea_query::ExprTrait, TransactionSession};
use serde::{Deserialize, Serialize};
use serde_json::Map;
use std::sync::{Arc, LazyLock};
use uuid::Uuid;

pub use super::_entities::users::{self, ActiveModel, Entity, Model};
use crate::data::{
    chat_hub::{ChatEvent, ChatHub},
    live::CloseScope,
    notify_hub::NotifyHub,
};

/// The JWT claim holding the session version.
const SESSION_VERSION_CLAIM: &str = "sv";
/// The JWT claim naming one sign-in, random each time. Unlocking logins is
/// tied to it, so an unlock on one device does not unlock another.
const SESSION_ID_CLAIM: &str = "sid";
/// How long re-entering the app password unlocks logins for.
pub const UNLOCK_MINUTES: i64 = 5;

pub const MAGIC_LINK_LENGTH: i8 = 32;
pub const MAGIC_LINK_EXPIRATION_MIN: i8 = 5;
pub const RESET_TOKEN_EXPIRATION_MIN: i64 = 60;
pub const PASSWORD_MIN_LENGTH: u64 = 8;

/// A letter first, then letters or numbers, 3 to 30 characters in total.
pub static USERNAME_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^[A-Za-z][A-Za-z0-9]{2,29}$").expect("username regex is valid")
});

/// Usernames can't be an @ keyword such as `@everyone`, in any case.
///
/// # Errors
/// A field error when the name is reserved.
pub fn not_reserved(name: &str) -> Result<(), validator::ValidationError> {
    if name
        .trim()
        .eq_ignore_ascii_case(super::notifications::EVERYONE)
    {
        return Err(validator::ValidationError::new("reserved")
            .with_message("That name is reserved.".into()));
    }
    Ok(())
}

/// Emails are compared and stored lowercase.
#[must_use]
pub fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LoginParams {
    pub email: String,
    pub password: String,
}

/// `name` is the username shown across the app.
#[derive(Debug, Deserialize, Serialize, Validate)]
pub struct RegisterParams {
    #[validate(email(message = "Enter a valid email address."))]
    pub email: String,
    #[validate(length(min = PASSWORD_MIN_LENGTH, max = 128, message = "Use at least 8 characters."))]
    pub password: String,
    #[validate(
        regex(path = *USERNAME_RE, message = "Start with a letter and use 3 to 30 letters or numbers."),
        custom(function = "not_reserved")
    )]
    pub name: String,
}

#[derive(Debug, Validate, Deserialize)]
pub struct PasswordParams {
    #[validate(length(min = PASSWORD_MIN_LENGTH, max = 128, message = "Use at least 8 characters."))]
    pub password: String,
}

#[derive(Debug, Validate, Deserialize)]
pub struct Validator {
    #[validate(
        regex(path = *USERNAME_RE, message = "Start with a letter and use 3 to 30 letters or numbers."),
        custom(function = "not_reserved")
    )]
    pub name: String,
    #[validate(email(message = "Enter a valid email address."))]
    pub email: String,
}

impl Validatable for ActiveModel {
    fn validator(&self) -> Box<dyn Validate> {
        Box::new(Validator {
            name: self.name.as_ref().to_owned(),
            email: self.email.as_ref().to_owned(),
        })
    }
}

#[async_trait::async_trait]
impl ActiveModelBehavior for super::_entities::users::ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        if let ActiveValue::Set(email) = &this.email {
            let normalized = normalize_email(email);
            if &normalized != email {
                this.email = ActiveValue::Set(normalized);
            }
        }
        this.validate()?;
        if insert {
            this.pid = ActiveValue::Set(Uuid::new_v4());
            this.api_key = ActiveValue::Set(format!("lo-{}", Uuid::new_v4()));
            Ok(this)
        } else {
            Ok(this)
        }
    }
}

#[async_trait]
impl Authenticable for Model {
    async fn find_by_api_key(db: &DatabaseConnection, api_key: &str) -> ModelResult<Self> {
        Self::find_by_api_key(db, api_key).await
    }

    async fn find_by_claims_key(db: &DatabaseConnection, claims_key: &str) -> ModelResult<Self> {
        Self::find_by_pid(db, claims_key).await
    }
}

impl Model {
    /// finds a user by the provided email
    ///
    /// # Errors
    ///
    /// When could not find user by the given token or DB query error
    pub async fn find_by_email<C: ConnectionTrait>(db: &C, email: &str) -> ModelResult<Self> {
        let user = users::Entity::find()
            .filter(
                model::query::condition()
                    .eq(users::Column::Email, normalize_email(email))
                    .build(),
            )
            .one(db)
            .await?;
        user.ok_or_else(|| ModelError::EntityNotFound)
    }

    /// Every user, newest first. For the platform admin only.
    ///
    /// # Errors
    /// On database errors.
    pub async fn all_newest_first<C: ConnectionTrait>(db: &C) -> ModelResult<Vec<Self>> {
        Ok(users::Entity::find()
            .order_by_desc(users::Column::CreatedAt)
            .all(db)
            .await?)
    }

    /// # Errors
    /// `EntityNotFound` when no user has this id.
    pub async fn find_by_id<C: ConnectionTrait>(db: &C, id: i64) -> ModelResult<Self> {
        users::Entity::find_by_id(id)
            .one(db)
            .await?
            .ok_or(ModelError::EntityNotFound)
    }

    /// finds a user by the provided verification token
    ///
    /// # Errors
    ///
    /// When could not find user by the given token or DB query error
    pub async fn find_by_verification_token(
        db: &DatabaseConnection,
        token: &str,
    ) -> ModelResult<Self> {
        let user = users::Entity::find()
            .filter(
                model::query::condition()
                    .eq(users::Column::EmailVerificationToken, token)
                    .build(),
            )
            .one(db)
            .await?;
        user.ok_or_else(|| ModelError::EntityNotFound)
    }

    /// finds a user by the magic token and verify and token expiration
    ///
    /// # Errors
    ///
    /// When could not find user by the given token or DB query error ot token expired
    pub async fn find_by_magic_token(db: &DatabaseConnection, token: &str) -> ModelResult<Self> {
        let user = users::Entity::find()
            .filter(
                query::condition()
                    .eq(users::Column::MagicLinkToken, token)
                    .build(),
            )
            .one(db)
            .await?;

        let user = user.ok_or_else(|| ModelError::EntityNotFound)?;
        if let Some(expired_at) = user.magic_link_expiration {
            if expired_at >= Local::now() {
                Ok(user)
            } else {
                tracing::debug!(
                    user_pid = user.pid.to_string(),
                    token_expiration = expired_at.to_string(),
                    "magic token expired for the user."
                );
                Err(ModelError::msg("magic token expired"))
            }
        } else {
            tracing::error!(
                user_pid = user.pid.to_string(),
                "magic link expiration time not exists"
            );
            Err(ModelError::msg("expiration token not exists"))
        }
    }

    /// finds a user by the provided reset token
    ///
    /// # Errors
    ///
    /// When could not find user by the given token or DB query error
    pub async fn find_by_reset_token(db: &DatabaseConnection, token: &str) -> ModelResult<Self> {
        let user = users::Entity::find()
            .filter(
                model::query::condition()
                    .eq(users::Column::ResetToken, token)
                    .build(),
            )
            .one(db)
            .await?;

        let user = user.ok_or_else(|| ModelError::EntityNotFound)?;
        if let Some(sent_at) = user.reset_sent_at {
            if sent_at + Duration::minutes(RESET_TOKEN_EXPIRATION_MIN) >= Local::now() {
                Ok(user)
            } else {
                tracing::debug!(
                    user_pid = user.pid.to_string(),
                    "reset token expired for the user."
                );
                Err(ModelError::msg("reset token expired"))
            }
        } else {
            tracing::error!(
                user_pid = user.pid.to_string(),
                "reset token sent time does not exist"
            );
            Err(ModelError::msg("reset token sent time not exists"))
        }
    }

    /// finds a user by the provided pid
    ///
    /// # Errors
    ///
    /// When could not find user  or DB query error
    pub async fn find_by_pid(db: &DatabaseConnection, pid: &str) -> ModelResult<Self> {
        let parse_uuid = Uuid::parse_str(pid).map_err(|e| ModelError::Any(e.into()))?;
        let user = users::Entity::find()
            .filter(
                model::query::condition()
                    .eq(users::Column::Pid, parse_uuid)
                    .build(),
            )
            .one(db)
            .await?;
        user.ok_or_else(|| ModelError::EntityNotFound)
    }

    /// finds a user by the provided api key
    ///
    /// # Errors
    ///
    /// When could not find user by the given token or DB query error
    pub async fn find_by_api_key(db: &DatabaseConnection, api_key: &str) -> ModelResult<Self> {
        let user = users::Entity::find()
            .filter(
                model::query::condition()
                    .eq(users::Column::ApiKey, api_key)
                    .build(),
            )
            .one(db)
            .await?;
        user.ok_or_else(|| ModelError::EntityNotFound)
    }

    /// Verifies whether the provided plain password matches the hashed password
    ///
    /// # Errors
    ///
    /// when could not verify password
    #[must_use]
    pub fn verify_password(&self, password: &str) -> bool {
        hash::verify_password(password, &self.password)
    }

    /// Asynchronously creates a user with a password and saves it to the
    /// database.
    ///
    /// # Errors
    ///
    /// When could not save the user into the DB
    pub async fn create_with_password<C>(db: &C, params: &RegisterParams) -> ModelResult<Self>
    where
        C: ConnectionTrait + TransactionTrait,
    {
        ValidatorTrait::validate(params)?;
        let email = normalize_email(&params.email);
        let txn = db.begin().await?;

        if users::Entity::find()
            .filter(
                model::query::condition()
                    .eq(users::Column::Email, &email)
                    .build(),
            )
            .one(&txn)
            .await?
            .is_some()
        {
            return Err(ModelError::EntityAlreadyExists {});
        }

        let password_hash =
            hash::hash_password(&params.password).map_err(|e| ModelError::Any(e.into()))?;
        let user = users::ActiveModel {
            email: ActiveValue::set(email),
            password: ActiveValue::set(password_hash),
            name: ActiveValue::set(params.name.clone()),
            ..Default::default()
        }
        .insert(&txn)
        .await?;

        txn.commit().await?;

        Ok(user)
    }

    /// Creates a session token for a new sign-in. It carries the session
    /// version, so bumping the version ends every session issued before,
    /// and a random sign-in id.
    ///
    /// # Errors
    ///
    /// when could not convert user claims to jwt token
    pub fn generate_jwt(&self, secret: &str, expiration: u64) -> ModelResult<String> {
        self.generate_jwt_for_session(secret, expiration, &Uuid::new_v4().to_string())
    }

    /// Like [`Self::generate_jwt`], with a given sign-in id.
    ///
    /// # Errors
    ///
    /// when could not convert user claims to jwt token
    pub fn generate_jwt_for_session(
        &self,
        secret: &str,
        expiration: u64,
        session_id: &str,
    ) -> ModelResult<String> {
        let mut claims = Map::new();
        claims.insert(
            SESSION_VERSION_CLAIM.to_string(),
            self.session_version.into(),
        );
        claims.insert(SESSION_ID_CLAIM.to_string(), session_id.into());
        jwt::JWT::new(secret)
            .generate_token(expiration, self.pid.to_string(), claims)
            .map_err(ModelError::from)
    }

    /// Whether a token's claims belong to this user's current sessions. A
    /// token with no version (issued before versions existed) is refused.
    #[must_use]
    pub fn is_current_session(&self, claims: &Map<String, serde_json::Value>) -> bool {
        claims
            .get(SESSION_VERSION_CLAIM)
            .and_then(serde_json::Value::as_i64)
            == Some(i64::from(self.session_version))
    }

    /// The sign-in a token belongs to. Tokens issued before sign-ins had ids
    /// have none.
    #[must_use]
    pub fn session_id(claims: &Map<String, serde_json::Value>) -> Option<String> {
        claims
            .get(SESSION_ID_CLAIM)
            .and_then(serde_json::Value::as_str)
            .filter(|sid| !sid.is_empty())
            .map(str::to_string)
    }

    /// Whether this sign-in re-entered the app password in the last
    /// [`UNLOCK_MINUTES`], inside this organisation. The time, the sign-in
    /// and the organisation must all match.
    #[must_use]
    pub fn secrets_unlocked(&self, session_id: Option<&str>, org_id: i64) -> bool {
        let fresh = self
            .secrets_unlocked_until
            .is_some_and(|until| until > chrono::Utc::now());
        fresh
            && session_id.is_some()
            && self.secrets_unlocked_sid.as_deref() == session_id
            && self.secrets_unlocked_org_id == Some(org_id)
    }

    /// Seconds left on the unlock, for the "Unlocked for 4 more minutes"
    /// line that counts down in the page.
    #[must_use]
    pub fn unlocked_seconds_left(&self) -> i64 {
        self.secrets_unlocked_until.map_or(0, |until| {
            std::cmp::max(
                (until.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_seconds(),
                0,
            )
        })
    }

    /// Unlocks logins for [`UNLOCK_MINUTES`], for one sign-in and one
    /// organisation only. Returns false, and unlocks nothing, unless the user
    /// is an active member there. The membership is read `FOR SHARE`, so a
    /// removal committing at the same time either runs first (and this is
    /// refused) or after (and its `end_unlock_in` clears this).
    ///
    /// # Errors
    /// On database errors.
    pub async fn unlock_secrets(
        db: &DatabaseConnection,
        user_id: i64,
        session_id: &str,
        org_id: i64,
    ) -> ModelResult<bool> {
        let txn = db.begin().await?;
        if super::memberships::Model::active_in_for_share(&txn, org_id, user_id)
            .await?
            .is_none()
        {
            txn.rollback().await?;
            return Ok(false);
        }
        let until = chrono::Utc::now() + Duration::minutes(UNLOCK_MINUTES);
        users::Entity::update_many()
            .col_expr(
                users::Column::SecretsUnlockedUntil,
                Expr::value(Some(until.fixed_offset())),
            )
            .col_expr(
                users::Column::SecretsUnlockedSid,
                Expr::value(Some(session_id.to_string())),
            )
            .col_expr(
                users::Column::SecretsUnlockedOrgId,
                Expr::value(Some(org_id)),
            )
            .filter(users::Column::Id.eq(user_id))
            .exec(&txn)
            .await?;
        txn.commit().await?;
        Ok(true)
    }

    /// Ends any unlock of logins, without ending sessions.
    ///
    /// # Errors
    /// On database errors.
    pub async fn end_unlock<C: ConnectionTrait>(db: &C, user_id: i64) -> ModelResult<()> {
        Self::end_unlock_where(db, user_id, Condition::all()).await
    }

    /// Ends an unlock of logins only if it is for this organisation, so an
    /// unlock in another of the person's organisations keeps running. For a
    /// removal. One `UPDATE`.
    ///
    /// # Errors
    /// On database errors.
    pub async fn end_unlock_in<C: ConnectionTrait>(
        db: &C,
        user_id: i64,
        org_id: i64,
    ) -> ModelResult<()> {
        let only = Condition::all().add(users::Column::SecretsUnlockedOrgId.eq(org_id));
        Self::end_unlock_where(db, user_id, only).await
    }

    /// Ends an unlock of logins only if this sign-in made it, so a switch in
    /// one browser leaves an unlock in another alone. One `UPDATE`.
    ///
    /// # Errors
    /// On database errors.
    pub async fn end_unlock_for_session<C: ConnectionTrait>(
        db: &C,
        user_id: i64,
        session_id: &str,
    ) -> ModelResult<()> {
        let only = Condition::all().add(users::Column::SecretsUnlockedSid.eq(session_id));
        Self::end_unlock_where(db, user_id, only).await
    }

    async fn end_unlock_where<C: ConnectionTrait>(
        db: &C,
        user_id: i64,
        only: Condition,
    ) -> ModelResult<()> {
        users::Entity::update_many()
            .col_expr(
                users::Column::SecretsUnlockedUntil,
                Expr::value(None::<chrono::DateTime<chrono::FixedOffset>>),
            )
            .col_expr(
                users::Column::SecretsUnlockedSid,
                Expr::value(None::<String>),
            )
            .col_expr(
                users::Column::SecretsUnlockedOrgId,
                Expr::value(None::<i64>),
            )
            .filter(users::Column::Id.eq(user_id))
            .filter(only)
            .exec(db)
            .await?;
        Ok(())
    }

    /// Whether the account is locked until its password is reset.
    #[must_use]
    pub fn is_locked(&self) -> bool {
        self.locked_at.is_some()
    }

    /// Locks the account after too many wrong app passwords inside a
    /// session, and signs the person out everywhere. Only a password reset
    /// unlocks it. Their Claude tokens are revoked too, in every
    /// organisation, not only the one where it happened. Who is told is the
    /// caller's job: the owners of the organisation where it happened, and
    /// the person. Returns false if it was already locked, so they are told
    /// once per lock.
    ///
    /// # Errors
    /// On database errors.
    pub async fn lock_account(ctx: &AppContext, user_id: i64) -> ModelResult<bool> {
        let txn = ctx.db.begin().await?;
        let locked = users::Entity::update_many()
            .col_expr(
                users::Column::LockedAt,
                Expr::value(Some(chrono::Utc::now().fixed_offset())),
            )
            .filter(users::Column::Id.eq(user_id))
            .filter(users::Column::LockedAt.is_null())
            .exec(&txn)
            .await?
            .rows_affected
            == 1;
        Self::increment_session_version(&txn, user_id).await?;
        super::access_tokens::Model::revoke_all_for_user(&txn, user_id).await?;
        txn.commit().await?;
        Self::close_live_connections(ctx, user_id);
        Ok(locked)
    }

    /// Signs the user out everywhere: every session token stops working, and
    /// their open chat sockets and notification streams close.
    ///
    /// # Errors
    /// On database errors.
    pub async fn bump_session_version(ctx: &AppContext, user_id: i64) -> ModelResult<()> {
        Self::increment_session_version(&ctx.db, user_id).await?;
        Self::close_live_connections(ctx, user_id);
        Ok(())
    }

    /// The database half of [`Self::bump_session_version`], for callers inside
    /// a transaction. Call [`Self::close_live_connections`] after committing.
    /// One `UPDATE`, so two bumps at once never lose one. It also ends any
    /// unlock of logins.
    ///
    /// # Errors
    /// On database errors.
    pub async fn increment_session_version<C: ConnectionTrait>(
        db: &C,
        user_id: i64,
    ) -> ModelResult<()> {
        users::Entity::update_many()
            .col_expr(
                users::Column::SessionVersion,
                Expr::col(users::Column::SessionVersion).add(1),
            )
            .col_expr(
                users::Column::SecretsUnlockedUntil,
                Expr::value(None::<chrono::DateTime<chrono::FixedOffset>>),
            )
            .col_expr(
                users::Column::SecretsUnlockedSid,
                Expr::value(None::<String>),
            )
            .col_expr(
                users::Column::SecretsUnlockedOrgId,
                Expr::value(None::<i64>),
            )
            .col_expr(users::Column::UpdatedAt, Expr::value(chrono::Utc::now()))
            .filter(users::Column::Id.eq(user_id))
            .exec(db)
            .await?;
        Ok(())
    }

    /// Sets a new password from a reset link, ends every session and revokes
    /// the person's Claude tokens in every organisation, in one transaction,
    /// then closes the user's live connections. Returns the user and how
    /// many tokens were revoked, for the email.
    ///
    /// # Errors
    /// Validation errors for a bad password, or database errors.
    pub async fn reset_password_everywhere(
        self,
        ctx: &AppContext,
        password: &str,
    ) -> ModelResult<(Self, u64)> {
        let txn = ctx.db.begin().await?;
        let user = self
            .into_active_model()
            .reset_password(&txn, password)
            .await?;
        Self::increment_session_version(&txn, user.id).await?;
        let revoked = super::access_tokens::Model::revoke_all_for_user(&txn, user.id).await?;
        txn.commit().await?;
        Self::close_live_connections(ctx, user.id);
        Ok((user, revoked))
    }

    /// Closes every open chat socket and notification stream of the user,
    /// in every organisation: sign out everywhere, a reset and a lock.
    pub fn close_live_connections(ctx: &AppContext, user_id: i64) {
        Self::close(ctx, CloseScope::Account { user_id });
    }

    /// Closes the user's chat sockets and notification streams in one
    /// organisation, on every device: a removal.
    pub fn close_in_org(ctx: &AppContext, org_id: i64, user_id: i64) {
        Self::close(ctx, CloseScope::Org { org_id, user_id });
    }

    /// Closes the chat sockets and notification streams of one browser's
    /// sign-in: a switch or a create. With no sign-in id (a cookie from
    /// before sign-in ids) it closes all of them, as before.
    pub fn close_in_browser(ctx: &AppContext, user_id: i64, session_id: Option<&str>) {
        let scope = match session_id {
            Some(sid) => CloseScope::Browser {
                user_id,
                session_id: Arc::from(sid),
            },
            None => CloseScope::Account { user_id },
        };
        Self::close(ctx, scope);
    }

    fn close(ctx: &AppContext, scope: CloseScope) {
        if let Some(hub) = ctx.shared_store.get::<ChatHub>() {
            hub.publish(ChatEvent::Close(scope.clone()));
        }
        if let Some(hub) = ctx.shared_store.get::<NotifyHub>() {
            hub.close(scope);
        }
    }
}

impl ActiveModel {
    /// Grants platform-wide super admin rights.
    ///
    /// # Errors
    /// On database errors.
    pub async fn make_super_admin<C: ConnectionTrait>(mut self, db: &C) -> ModelResult<Model> {
        self.is_super_admin = ActiveValue::Set(true);
        self.update(db).await.map_err(ModelError::from)
    }

    /// Sets the email verification information for the user and
    /// updates it in the database.
    ///
    /// This method is used to record the timestamp when the email verification
    /// was sent and generate a unique verification token for the user.
    ///
    /// # Errors
    ///
    /// when has DB query error
    pub async fn set_email_verification_sent(
        mut self,
        db: &DatabaseConnection,
    ) -> ModelResult<Model> {
        self.email_verification_sent_at = ActiveValue::set(Some(Local::now().into()));
        self.email_verification_token = ActiveValue::Set(Some(Uuid::new_v4().to_string()));
        self.update(db).await.map_err(ModelError::from)
    }

    /// Sets the information for a reset password request,
    /// generates a unique reset password token, and updates it in the
    /// database.
    ///
    /// This method records the timestamp when the reset password token is sent
    /// and generates a unique token for the user.
    ///
    /// # Arguments
    ///
    /// # Errors
    ///
    /// when has DB query error
    pub async fn set_forgot_password_sent(mut self, db: &DatabaseConnection) -> ModelResult<Model> {
        self.reset_sent_at = ActiveValue::set(Some(Local::now().into()));
        self.reset_token = ActiveValue::Set(Some(Uuid::new_v4().to_string()));
        self.update(db).await.map_err(ModelError::from)
    }

    /// Records the verification time when a user verifies their
    /// email and updates it in the database.
    ///
    /// This method sets the timestamp when the user successfully verifies their
    /// email.
    ///
    /// # Errors
    ///
    /// when has DB query error
    pub async fn verified(mut self, db: &DatabaseConnection) -> ModelResult<Model> {
        self.email_verified_at = ActiveValue::set(Some(Local::now().into()));
        // Invalidate the verification token so it cannot be replayed after use.
        self.email_verification_token = ActiveValue::Set(None);
        self.update(db).await.map_err(ModelError::from)
    }

    /// Resets the current user password with a new password and
    /// updates it in the database.
    ///
    /// This method hashes the provided password and sets it as the new password
    /// for the user.
    ///
    /// # Errors
    ///
    /// when has DB query error or could not hashed the given password
    pub async fn reset_password<C: ConnectionTrait>(
        mut self,
        db: &C,
        password: &str,
    ) -> ModelResult<Model> {
        ValidatorTrait::validate(&PasswordParams {
            password: password.to_string(),
        })?;
        self.password =
            ActiveValue::set(hash::hash_password(password).map_err(|e| ModelError::Any(e.into()))?);
        self.reset_token = ActiveValue::Set(None);
        self.reset_sent_at = ActiveValue::Set(None);
        // A reset is the only way out of a lock.
        self.locked_at = ActiveValue::Set(None);
        self.update(db).await.map_err(ModelError::from)
    }

    /// Creates a magic link token for passwordless authentication.
    ///
    /// Generates a random token with a specified length and sets an expiration time
    /// for the magic link. This method is used to initiate the magic link authentication flow.
    ///
    /// # Errors
    /// - Returns an error if database update fails
    pub async fn create_magic_link(mut self, db: &DatabaseConnection) -> ModelResult<Model> {
        let random_str = hash::random_string(MAGIC_LINK_LENGTH as usize);
        let expired = Local::now() + Duration::minutes(MAGIC_LINK_EXPIRATION_MIN.into());

        self.magic_link_token = ActiveValue::set(Some(random_str));
        self.magic_link_expiration = ActiveValue::set(Some(expired.into()));
        self.update(db).await.map_err(ModelError::from)
    }

    /// Verifies and invalidates the magic link after successful authentication.
    ///
    /// Clears the magic link token and expiration time after the user has
    /// successfully authenticated using the magic link.
    ///
    /// # Errors
    /// - Returns an error if database update fails
    pub async fn clear_magic_link(mut self, db: &DatabaseConnection) -> ModelResult<Model> {
        self.magic_link_token = ActiveValue::set(None);
        self.magic_link_expiration = ActiveValue::set(None);
        self.update(db).await.map_err(ModelError::from)
    }
}
