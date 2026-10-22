//! Claude connections made before writes had to name their organisation. Each
//! one is told about the change in its tool results until it makes such a
//! write, for at most [`NOTICE_DAYS`], and its person gets one bell notice.
//! The rows are filled by the migration that made the change; nothing adds
//! new ones.
use loco_rs::prelude::*;

pub use super::_entities::claude_update_notices::{ActiveModel, Column, Entity, Model};
use super::{
    memberships,
    notifications::{self, kind},
};

pub type ClaudeUpdateNotices = Entity;

/// How long after the release the tool results carry the notice.
pub const NOTICE_DAYS: i64 = 14;

/// The bell notice: headline, then the line under it.
pub const BELL: &str =
    "Claude connector updated.\nIf Claude says a write needs an organisation, restart it.";

impl ActiveModelBehavior for ActiveModel {}

impl Model {
    /// Whether this connection's tool results should carry the notice.
    ///
    /// # Errors
    /// On database errors.
    pub async fn due<C: ConnectionTrait>(db: &C, grant_id: Uuid) -> ModelResult<bool> {
        let since = chrono::Utc::now() - chrono::Duration::days(NOTICE_DAYS);
        Ok(Entity::find_by_id(grant_id)
            .filter(Column::WrittenAt.is_null())
            .filter(Column::CreatedAt.gt(since))
            .count(db)
            .await?
            > 0)
    }

    /// Records that this connection has made a write naming its organisation,
    /// so it has the new tools and needs no more telling.
    ///
    /// # Errors
    /// On database errors.
    pub async fn written<C: ConnectionTrait>(db: &C, grant_id: Uuid) -> ModelResult<()> {
        Entity::update_many()
            .col_expr(Column::WrittenAt, Expr::value(chrono::Utc::now()))
            .filter(Column::GrantId.eq(grant_id))
            .filter(Column::WrittenAt.is_null())
            .exec(db)
            .await?;
        Ok(())
    }

    /// Sends the one-time bell notice: one per person and organisation, for
    /// connections not yet announced, to people still active there. Marking
    /// the rows and sending share a transaction, so a second run (a restart,
    /// or a second instance) sends nothing. Returns who was told, as
    /// `(org_id, user_id)`.
    ///
    /// # Errors
    /// On database errors.
    pub async fn announce(db: &DatabaseConnection) -> ModelResult<Vec<(i64, i64)>> {
        let txn = db.begin().await?;
        let rows = Entity::update_many()
            .col_expr(Column::AnnouncedAt, Expr::value(chrono::Utc::now()))
            .filter(Column::AnnouncedAt.is_null())
            .exec_with_returning(&txn)
            .await?;
        let mut told = Vec::new();
        for row in rows {
            let pair = (row.organisation_id, row.user_id);
            if told.contains(&pair)
                || memberships::Model::active_in(&txn, row.organisation_id, row.user_id)
                    .await?
                    .is_none()
            {
                continue;
            }
            notifications::ActiveModel {
                user_id: ActiveValue::Set(row.user_id),
                kind: ActiveValue::Set(kind::CONNECTOR.to_string()),
                body: ActiveValue::Set(BELL.to_string()),
                link: ActiveValue::Set("/settings/claude".to_string()),
                ..Default::default()
            }
            .set_tenant(row.organisation_id)?
            .insert(&txn)
            .await?;
            told.push(pair);
        }
        txn.commit().await?;
        Ok(told)
    }
}
