use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

/// Document folders, one level deep. Every organisation starts with
/// "Company" and "Logins"; existing ones get them here.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "folders",
            &[
                ("id", ColType::PkAuto),
                ("name", ColType::String),
                ("note", ColType::TextNull),
            ],
            &[("organisation", ""), ("users?", "created_by_id")],
        )
        .await?;
        for sql in [
            r#"CREATE UNIQUE INDEX "idx-folders-org-name" ON folders (organisation_id, lower(name));"#,
            r#"CREATE INDEX "idx-folders-created_by" ON folders (created_by_id);"#,
            "INSERT INTO folders (organisation_id, name, note, created_at, updated_at)
             SELECT id, 'Company', NULL, now(), now() FROM organisations;",
            "INSERT INTO folders (organisation_id, name, note, created_at, updated_at)
             SELECT id, 'Logins', 'Shared logins for the team. Never put passwords in notes.', now(), now()
             FROM organisations;",
        ] {
            m.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "folders").await
    }
}
