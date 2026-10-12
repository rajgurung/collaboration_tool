use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

/// Uploaded files. The bytes live in object storage under `storage_key`. A
/// folder cannot be deleted while it still holds files (NO ACTION, checked
/// at the end of the statement, so deleting a whole organisation still
/// cascades through its folders and files).
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "documents",
            &[
                ("id", ColType::PkAuto),
                ("name", ColType::String),
                ("content_type", ColType::String),
                ("size_bytes", ColType::BigInteger),
                ("storage_key", ColType::StringUniq),
            ],
            &[
                ("organisation", ""),
                ("folder", ""),
                ("users?", "uploaded_by_id"),
            ],
        )
        .await?;
        for sql in [
            r#"ALTER TABLE documents DROP CONSTRAINT "fk-documents-folder_id-to-folders";"#,
            r#"ALTER TABLE documents ADD CONSTRAINT "fk-documents-folder_id-to-folders" FOREIGN KEY (folder_id) REFERENCES folders (id) ON DELETE NO ACTION ON UPDATE CASCADE;"#,
            r#"CREATE INDEX "idx-documents-folder-created" ON documents (folder_id, created_at);"#,
            r#"CREATE INDEX "idx-documents-org" ON documents (organisation_id);"#,
            r#"CREATE INDEX "idx-documents-uploaded_by" ON documents (uploaded_by_id);"#,
        ] {
            m.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "documents").await
    }
}
