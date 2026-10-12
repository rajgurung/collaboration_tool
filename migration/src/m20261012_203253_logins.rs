use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

/// Shared logins. Title, username, URL and notes are plain text; the password
/// and recovery codes are stored only as XChaCha20-Poly1305 ciphertext under
/// the organisation's data key (`dek_version`). Like documents, a folder
/// cannot be deleted while it holds logins (NO ACTION on folder_id).
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "logins",
            &[
                ("id", ColType::PkAuto),
                ("title", ColType::String),
                ("username", ColType::String),
                ("url", ColType::String),
                ("notes", ColType::Text),
                ("password_ct", ColType::Blob),
                ("password_nonce", ColType::Blob),
                ("recovery_ct", ColType::BlobNull),
                ("recovery_nonce", ColType::BlobNull),
                ("dek_version", ColType::Integer),
            ],
            &[
                ("organisation", ""),
                ("folder", ""),
                ("users?", "created_by_id"),
            ],
        )
        .await?;
        for sql in [
            r#"ALTER TABLE logins DROP CONSTRAINT "fk-logins-folder_id-to-folders";"#,
            r#"ALTER TABLE logins ADD CONSTRAINT "fk-logins-folder_id-to-folders" FOREIGN KEY (folder_id) REFERENCES folders (id) ON DELETE NO ACTION ON UPDATE CASCADE;"#,
            r#"CREATE INDEX "idx-logins-folder-created" ON logins (folder_id, created_at);"#,
            r#"CREATE INDEX "idx-logins-org" ON logins (organisation_id);"#,
            r#"CREATE INDEX "idx-logins-created_by" ON logins (created_by_id);"#,
        ] {
            m.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "logins").await
    }
}
