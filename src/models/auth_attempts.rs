//! Failed sign-ins and password reset requests, counted for rate limits.
//! Emails are kept only as a hash of the normalised address, so unknown
//! emails are counted the same as real ones.
use chrono::{DateTime, Duration, Utc};
use loco_rs::prelude::*;

pub use super::_entities::auth_attempts::{ActiveModel, Column, Entity, Model};
use super::{access_tokens, users};

pub type AuthAttempts = Entity;

pub mod kind {
    pub const LOGIN: &str = "login";
    pub const FORGOT: &str = "forgot";
}

/// Failed sign-ins for one email that lock it.
pub const LOGIN_EMAIL_LIMIT: usize = 5;
/// Failed sign-ins from one IP that lock it.
pub const LOGIN_IP_LIMIT: usize = 20;
/// The window those failures must fall in, and how long the lock lasts.
pub const LOCK_MINUTES: i64 = 15;
/// Reset emails per email address, and per IP, in an hour.
pub const FORGOT_EMAIL_LIMIT: u64 = 3;
pub const FORGOT_IP_LIMIT: u64 = 10;
/// Rows older than this are deleted when new ones are written.
const KEEP_HOURS: i64 = 24;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {}

/// The stored form of an email: SHA-256 of the normalised address.
#[must_use]
pub fn email_hash(email: &str) -> String {
    access_tokens::hash(&users::normalize_email(email))
}

/// Whether `limit` earlier attempts (newest first) fell within the lock window
/// and the newest is still inside it. Refused attempts are deleted, so a lock
/// always ends [`LOCK_MINUTES`] after the failure that started it.
fn locked(newest_first: &[DateTime<Utc>], limit: usize, now: DateTime<Utc>) -> bool {
    let window = Duration::minutes(LOCK_MINUTES);
    match (newest_first.first(), newest_first.get(limit - 1)) {
        (Some(newest), Some(oldest)) => *newest - *oldest <= window && now - *newest < window,
        _ => false,
    }
}

impl Model {
    /// Records a sign-in attempt before the password is checked, so attempts
    /// sent at the same moment all see each other. Returns false when this
    /// email or IP is locked; the refused attempt is then deleted, so
    /// refusals never stretch a lock.
    ///
    /// # Errors
    /// On database errors.
    pub async fn begin_login<C: ConnectionTrait>(
        db: &C,
        email: &str,
        user_id: Option<i64>,
        ip: &str,
    ) -> ModelResult<bool> {
        let row = Self::record(db, kind::LOGIN, email, user_id, ip).await?;
        let now = Utc::now();
        let by_email = Self::earlier_logins(
            db,
            Column::EmailHash,
            &row.email_hash,
            row.id,
            LOGIN_EMAIL_LIMIT,
        )
        .await?;
        let by_ip = Self::earlier_logins(db, Column::Ip, ip, row.id, LOGIN_IP_LIMIT).await?;
        if locked(&by_email, LOGIN_EMAIL_LIMIT, now) || locked(&by_ip, LOGIN_IP_LIMIT, now) {
            Entity::delete_by_id(row.id).exec(db).await?;
            return Ok(false);
        }
        Ok(true)
    }

    /// The newest `limit` sign-in attempts matching `value`, other than
    /// `except`, newest first. Attempts still being checked count as failures.
    async fn earlier_logins<C: ConnectionTrait>(
        db: &C,
        column: Column,
        value: &str,
        except: i64,
        limit: usize,
    ) -> ModelResult<Vec<DateTime<Utc>>> {
        let since = Utc::now() - Duration::minutes(LOCK_MINUTES * 2);
        Ok(Entity::find()
            .filter(column.eq(value))
            .filter(Column::Kind.eq(kind::LOGIN))
            .filter(Column::Id.ne(except))
            .filter(Column::CreatedAt.gt(since))
            .order_by_desc(Column::CreatedAt)
            .limit(limit as u64)
            .all(db)
            .await?
            .into_iter()
            .map(|row| row.created_at.with_timezone(&Utc))
            .collect())
    }

    /// Records a reset request, then counts it with the others in the last
    /// hour. Returns whether an email may go out; a refused request is deleted.
    ///
    /// # Errors
    /// On database errors.
    pub async fn begin_forgot<C: ConnectionTrait>(
        db: &C,
        email: &str,
        user_id: Option<i64>,
        ip: &str,
    ) -> ModelResult<bool> {
        let row = Self::record(db, kind::FORGOT, email, user_id, ip).await?;
        let since = Utc::now() - Duration::hours(1);
        let count = |column: Column, value: String| {
            Entity::find()
                .filter(column.eq(value))
                .filter(Column::Kind.eq(kind::FORGOT))
                .filter(Column::CreatedAt.gt(since))
                .count(db)
        };
        let allowed = count(Column::EmailHash, row.email_hash.clone()).await? <= FORGOT_EMAIL_LIMIT
            && count(Column::Ip, ip.to_string()).await? <= FORGOT_IP_LIMIT;
        if !allowed {
            Entity::delete_by_id(row.id).exec(db).await?;
        }
        Ok(allowed)
    }

    /// Stores an attempt and deletes rows older than a day.
    async fn record<C: ConnectionTrait>(
        db: &C,
        kind: &str,
        email: &str,
        user_id: Option<i64>,
        ip: &str,
    ) -> ModelResult<Self> {
        Entity::delete_many()
            .filter(Column::CreatedAt.lt(Utc::now() - Duration::hours(KEEP_HOURS)))
            .exec(db)
            .await?;
        Ok(ActiveModel {
            email_hash: ActiveValue::Set(email_hash(email)),
            user_id: ActiveValue::Set(user_id),
            ip: ActiveValue::Set(ip.to_string()),
            kind: ActiveValue::Set(kind.to_string()),
            ..Default::default()
        }
        .insert(db)
        .await?)
    }

    /// Forgets an email's sign-in attempts, after a successful sign-in or a
    /// completed password reset. Reset requests keep counting.
    ///
    /// # Errors
    /// On database errors.
    pub async fn clear_logins<C: ConnectionTrait>(db: &C, email: &str) -> ModelResult<()> {
        Entity::delete_many()
            .filter(Column::EmailHash.eq(email_hash(email)))
            .filter(Column::Kind.eq(kind::LOGIN))
            .exec(db)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_failures_in_fifteen_minutes_lock_for_fifteen_minutes() {
        let now = Utc::now();
        let ago = |m: i64| now - Duration::minutes(m);
        let five = [ago(0), ago(1), ago(2), ago(3), ago(14)];
        assert!(locked(&five, 5, now));
        assert!(!locked(&five[..4], 5, now), "four is fine");
        let spread = [ago(0), ago(1), ago(2), ago(3), ago(16)];
        assert!(!locked(&spread, 5, now), "not within fifteen minutes");
        let later = now + Duration::minutes(15);
        assert!(!locked(&five, 5, later), "the lock clears itself");
        assert!(locked(&five, 5, later - Duration::seconds(1)));
    }
}
