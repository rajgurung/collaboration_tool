use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "messages",
            &[("id", ColType::PkAuto), ("body", ColType::Text)],
            &[("organisation", ""), ("conversation", ""), ("user", "")],
        )
        .await?;
        m.get_connection()
            .execute_unprepared(
                r#"
CREATE INDEX "idx-messages-conv-created" ON messages (conversation_id, created_at);
CREATE INDEX "idx-messages-org" ON messages (organisation_id);
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "messages").await
    }
}
