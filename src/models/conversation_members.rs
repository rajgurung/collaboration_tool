use loco_rs::prelude::*;

pub use super::_entities::conversation_members::{ActiveModel, Column, Entity, Model};
use super::{conversations, messages};

pub type ConversationMembers = Entity;

/// A member, when they joined the conversation and when they last read it.
#[derive(Debug, Clone, Copy)]
pub struct ReadMark {
    pub user_id: i64,
    pub joined_at: DateTimeWithTimeZone,
    pub last_read_at: Option<DateTimeWithTimeZone>,
}

/// What one read covered: messages after `from` up to `to`.
#[derive(Debug, Clone, Copy)]
pub struct ReadSpan {
    pub from: DateTimeWithTimeZone,
    pub to: DateTimeWithTimeZone,
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
    /// Adds a user to a conversation in the conversation's organisation.
    ///
    /// # Errors
    /// On database errors, including a duplicate membership.
    pub async fn add<C: ConnectionTrait>(
        db: &C,
        conversation: &conversations::Model,
        user_id: i64,
    ) -> ModelResult<Self> {
        Ok(ActiveModel {
            conversation_id: ActiveValue::Set(conversation.id),
            user_id: ActiveValue::Set(user_id),
            ..Default::default()
        }
        .set_tenant(conversation.organisation_id)?
        .insert(db)
        .await?)
    }
}

/// The new `last_read_at`: now by the database clock, but never earlier than
/// before. `statement_timestamp()` is taken after any lock wait, unlike
/// `CURRENT_TIMESTAMP`, and `GREATEST` ignores a NULL `last_read_at`.
fn read_now() -> Expr {
    Expr::cust("GREATEST(last_read_at, statement_timestamp())")
}

impl Model {
    /// Records that `user_id` has seen everything in the conversation up to now,
    /// by the database clock so it compares cleanly with `messages.created_at`.
    /// Returns the span the read covered, or `None` when they are not a member.
    /// A first read starts at when they joined, like `unread_counts`.
    ///
    /// # Errors
    /// On database errors.
    pub async fn mark_read(
        db: &DatabaseConnection,
        org_id: i64,
        conversation_id: i64,
        user_id: i64,
    ) -> ModelResult<Option<ReadSpan>> {
        let txn = db.begin().await?;
        let Some(before) = Entity::find()
            .in_tenant(org_id)
            .filter(Column::ConversationId.eq(conversation_id))
            .filter(Column::UserId.eq(user_id))
            .lock_exclusive()
            .one(&txn)
            .await?
        else {
            // Not a member: nothing was written, so the transaction just rolls back.
            txn.rollback().await?;
            return Ok(None);
        };
        let after = Entity::update_many()
            .col_expr(Column::LastReadAt, read_now())
            .filter(Column::Id.eq(before.id))
            .exec_with_returning(&txn)
            .await?;
        txn.commit().await?;
        Ok(after
            .into_iter()
            .next()
            .and_then(|m| m.last_read_at)
            .map(|to| ReadSpan {
                from: before.last_read_at.unwrap_or(before.created_at),
                to,
            }))
    }

    /// Records a read with one plain UPDATE and no span, for when the caller
    /// knows nothing new from others was read (the viewer's own message).
    ///
    /// # Errors
    /// On database errors.
    pub async fn touch_read<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        conversation_id: i64,
        user_id: i64,
    ) -> ModelResult<()> {
        Entity::update_many()
            .col_expr(Column::LastReadAt, read_now())
            .filter(Column::ConversationId.eq(conversation_id))
            .filter(Column::UserId.eq(user_id))
            .in_tenant(org_id)
            .exec(db)
            .await?;
        Ok(())
    }

    /// When each member of a conversation joined it and last read it.
    ///
    /// # Errors
    /// On database errors.
    pub async fn read_marks<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        conversation_id: i64,
    ) -> ModelResult<Vec<ReadMark>> {
        Ok(Entity::find()
            .in_tenant(org_id)
            .filter(Column::ConversationId.eq(conversation_id))
            .all(db)
            .await?
            .into_iter()
            .map(|m| ReadMark {
                user_id: m.user_id,
                joined_at: m.created_at,
                last_read_at: m.last_read_at,
            })
            .collect())
    }

    /// Unread message count per conversation for `user_id`. Messages they sent
    /// do not count; before their first visit, counting starts when they joined.
    ///
    /// # Errors
    /// On database errors.
    pub async fn unread_counts<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
    ) -> ModelResult<std::collections::HashMap<i64, u64>> {
        let mut counts = std::collections::HashMap::new();
        let memberships = Entity::find()
            .in_tenant(org_id)
            .filter(Column::UserId.eq(user_id))
            .all(db)
            .await?;
        for m in memberships {
            let since = m.last_read_at.unwrap_or(m.created_at);
            let n = messages::Entity::find()
                .in_tenant(org_id)
                .filter(messages::Column::ConversationId.eq(m.conversation_id))
                .filter(messages::Column::UserId.ne(user_id))
                .filter(messages::Column::CreatedAt.gt(since))
                .count(db)
                .await?;
            if n > 0 {
                counts.insert(m.conversation_id, n);
            }
        }
        Ok(counts)
    }

    /// # Errors
    /// On database errors.
    pub async fn is_member<C: ConnectionTrait>(
        db: &C,
        conversation: &conversations::Model,
        user_id: i64,
    ) -> ModelResult<bool> {
        Ok(Entity::find()
            .in_tenant(conversation.organisation_id)
            .filter(Column::ConversationId.eq(conversation.id))
            .filter(Column::UserId.eq(user_id))
            .one(db)
            .await?
            .is_some())
    }
}

impl loco_rs::prelude::TenantEntity for Entity {
    type TenantId = i64;

    fn tenant_column() -> super::_entities::conversation_members::Column {
        super::_entities::conversation_members::Column::OrganisationId
    }
}
