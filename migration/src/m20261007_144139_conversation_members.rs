use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "conversation_members",
            &[("id", ColType::PkAuto)],
            &[("organisation", ""), ("conversation", ""), ("user", "")],
        )
        .await?;
        m.get_connection()
            .execute_unprepared(
                r#"
CREATE UNIQUE INDEX "idx-conversation_members-conv-user" ON conversation_members (conversation_id, user_id);
CREATE INDEX "idx-conversation_members-org-user" ON conversation_members (organisation_id, user_id);
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "conversation_members").await
    }
}
