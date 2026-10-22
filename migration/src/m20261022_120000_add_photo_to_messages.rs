use sea_orm_migration::prelude::*;

/// A chat message can carry one photo, stored as a document. `document_id`
/// goes NULL when the file is deleted from Documents; `has_photo` stays true,
/// so the message can say "Photo removed" instead of showing nothing. The
/// partial index keeps a document delete from scanning every message.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                r#"
ALTER TABLE messages
  ADD COLUMN document_id bigint NULL
  REFERENCES documents (id) ON DELETE SET NULL ON UPDATE CASCADE,
  ADD COLUMN has_photo boolean NOT NULL DEFAULT false;
CREATE INDEX "idx-messages-document"
  ON messages (document_id) WHERE document_id IS NOT NULL;
"#,
            )
            .await?;
        Ok(())
    }

    /// Drops both columns, the index and the key. Photo messages keep their
    /// caption; one with no caption is left with an empty body.
    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE messages DROP COLUMN document_id, DROP COLUMN has_photo;",
            )
            .await?;
        Ok(())
    }
}
