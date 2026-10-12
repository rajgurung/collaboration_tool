//! The login activity log: who saw, copied, added, changed or deleted a
//! login, and failed or locked re-entries of the app password. Append-only:
//! a database trigger refuses updates and deletes. Rows keep the login's
//! title and the person's username, so they read the same after either is
//! deleted.
use loco_rs::prelude::*;

pub use super::_entities::login_events::{ActiveModel, Column, Entity, Model};

pub type LoginEvents = Entity;

pub mod action {
    pub const REVEAL: &str = "reveal";
    pub const COPY: &str = "copy";
    pub const CREATE: &str = "create";
    pub const EDIT: &str = "edit";
    pub const DELETE: &str = "delete";
    pub const UNLOCK: &str = "unlock";
    pub const REAUTH_FAILED: &str = "reauth_failed";
    pub const LOCK: &str = "lock";
    pub const MEMBER_REMOVED: &str = "member_removed";
    pub const SIGN_OUT_EVERYWHERE: &str = "sign_out_everywhere";
}

/// Longest user agent kept.
const USER_AGENT_MAX: usize = 300;

/// Where a request came from.
#[derive(Debug, Clone, Default)]
pub struct Client {
    pub ip: String,
    pub user_agent: String,
}

/// One event to write.
pub struct Entry<'a> {
    pub org_id: i64,
    /// The login it is about, with its title at the time.
    pub login: Option<(i64, &'a str)>,
    /// Who did it, and the username they had in the organisation.
    pub user: (i64, &'a str),
    pub action: &'static str,
    pub detail: Option<String>,
}

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {}

impl Model {
    /// Writes an event.
    ///
    /// # Errors
    /// On database errors. Callers that go on to show a secret must stop.
    pub async fn record<C: ConnectionTrait>(
        db: &C,
        client: &Client,
        entry: Entry<'_>,
    ) -> ModelResult<Self> {
        Ok(ActiveModel {
            organisation_id: ActiveValue::Set(entry.org_id),
            login_id: ActiveValue::Set(entry.login.map(|(id, _)| id)),
            login_title: ActiveValue::Set(entry.login.map(|(_, title)| title.to_string())),
            user_id: ActiveValue::Set(Some(entry.user.0)),
            username: ActiveValue::Set(entry.user.1.to_string()),
            action: ActiveValue::Set(entry.action.to_string()),
            ip: ActiveValue::Set(client.ip.clone()),
            user_agent: ActiveValue::Set(client.user_agent.chars().take(USER_AGENT_MAX).collect()),
            detail: ActiveValue::Set(entry.detail),
            ..Default::default()
        }
        .insert(db)
        .await?)
    }

    /// The organisation's events, newest first.
    ///
    /// # Errors
    /// On database errors.
    pub async fn for_org<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        limit: u64,
    ) -> ModelResult<Vec<Self>> {
        Ok(Entity::find()
            .filter(Column::OrganisationId.eq(org_id))
            .order_by_desc(Column::CreatedAt)
            .order_by_desc(Column::Id)
            .limit(limit)
            .all(db)
            .await?)
    }

    /// One person's events of the given kinds, newest first.
    ///
    /// # Errors
    /// On database errors.
    pub async fn by_user<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
        actions: &[&str],
    ) -> ModelResult<Vec<Self>> {
        Ok(Entity::find()
            .filter(Column::OrganisationId.eq(org_id))
            .filter(Column::UserId.eq(user_id))
            .filter(Column::Action.is_in(actions.iter().copied()))
            .order_by_desc(Column::CreatedAt)
            .order_by_desc(Column::Id)
            .all(db)
            .await?)
    }
}
