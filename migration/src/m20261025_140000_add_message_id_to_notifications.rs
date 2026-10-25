use sea_orm_migration::prelude::*;

/// A chat mention or reply notice points at the message it quotes, and goes
/// when that message is deleted. Task, project and connector notices keep it
/// empty.
///
/// Chat mentions saved before this column existed can't be found by a
/// delete, so none of them is emailed after deploy. Task-note mentions link
/// to `/tasks?open=...` and are left alone.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                r#"
SET LOCAL lock_timeout = '5s';
ALTER TABLE notifications
  ADD COLUMN message_id bigint NULL
  REFERENCES messages (id) ON DELETE CASCADE ON UPDATE CASCADE;
CREATE INDEX "idx-notifications-message"
  ON notifications (message_id) WHERE message_id IS NOT NULL;
UPDATE notifications SET emailed_at = now()
 WHERE kind = 'mention' AND emailed_at IS NULL AND link LIKE '/chat/%';
"#,
            )
            .await?;
        Ok(())
    }

    /// Drops the column and its index. The emails skipped at deploy stay
    /// skipped, and deleted messages and their notices don't come back.
    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                r#"
DROP INDEX "idx-notifications-message";
ALTER TABLE notifications DROP COLUMN message_id;
"#,
            )
            .await?;
        Ok(())
    }
}
