use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "conversations",
            &[
                ("id", ColType::PkAuto),
                ("kind", ColType::String),
                ("name", ColType::StringNull),
                ("dm_key", ColType::StringNull),
            ],
            &[("organisation", ""), ("users?", "created_by_id")],
        )
        .await?;
        m.get_connection()
            .execute_unprepared(
                r#"
CREATE INDEX "idx-conversations-org" ON conversations (organisation_id);
CREATE UNIQUE INDEX "idx-conversations-org-dm_key" ON conversations (organisation_id, dm_key);
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "conversations").await
    }
}
