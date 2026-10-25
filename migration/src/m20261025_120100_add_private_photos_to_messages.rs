use sea_orm_migration::prelude::*;

/// A group or DM photo is stored on its message, outside Documents, so only
/// the conversation's members can open it. The four columns are all set or
/// all empty; a message never has both its own file and a Documents file;
/// and the stored type is always a photo, so a bad row can never be served
/// as HTML or SVG. #general photos keep using `document_id`.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                r#"
SET LOCAL lock_timeout = '5s';
ALTER TABLE messages
  ADD COLUMN photo_key text NULL,
  ADD COLUMN photo_type text NULL,
  ADD COLUMN photo_size bigint NULL,
  ADD COLUMN photo_name text NULL,
  ADD CONSTRAINT "messages-own-photo" CHECK (
    (photo_key IS NULL AND photo_type IS NULL AND photo_size IS NULL AND photo_name IS NULL)
    OR (photo_key IS NOT NULL AND photo_type IS NOT NULL
        AND photo_size IS NOT NULL AND photo_name IS NOT NULL
        AND photo_size > 0
        AND photo_type IN ('image/jpeg', 'image/png', 'image/webp')
        AND has_photo AND document_id IS NULL)
  );
"#,
            )
            .await?;
        Ok(())
    }

    /// Drops the columns and the constraint. That loses the only record of
    /// where group and DM photo bytes are stored, so export the keys first:
    /// `SELECT id, organisation_id, conversation_id, photo_key FROM messages
    /// WHERE photo_key IS NOT NULL`. The bytes all sit under `orgs/*/chat/`.
    /// The old code then shows those messages as "Photo removed".
    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                r#"
ALTER TABLE messages
  DROP CONSTRAINT "messages-own-photo",
  DROP COLUMN photo_key,
  DROP COLUMN photo_type,
  DROP COLUMN photo_size,
  DROP COLUMN photo_name;
"#,
            )
            .await?;
        Ok(())
    }
}
