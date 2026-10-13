use sea_orm_migration::prelude::*;

/// A thread reply points at its parent message. Existing messages get NULL and
/// stay top-level. Deleting a parent (today only with its author's account)
/// deletes its replies. The partial index serves "replies of these parents"
/// and stays small, since top-level messages are not in it. That a parent is
/// top-level and in the same conversation is checked by the model.
///
/// SQL rather than `add_reference`, which makes a non-null column named after
/// the target table.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                r#"
ALTER TABLE messages
  ADD COLUMN parent_id bigint NULL
  REFERENCES messages (id) ON DELETE CASCADE ON UPDATE CASCADE;
CREATE INDEX "idx-messages-parent-created"
  ON messages (parent_id, created_at) WHERE parent_id IS NOT NULL;
"#,
            )
            .await?;
        Ok(())
    }

    /// Drops the column, its index and its key. Replies become ordinary
    /// messages in the main feed; their text is kept.
    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE messages DROP COLUMN parent_id;")
            .await?;
        Ok(())
    }
}
