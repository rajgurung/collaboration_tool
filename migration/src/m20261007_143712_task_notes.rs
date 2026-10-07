use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "task_notes",
            &[("id", ColType::PkAuto), ("body", ColType::Text)],
            &[("organisation", ""), ("task", ""), ("users?", "author_id")],
        )
        .await?;
        m.get_connection()
            .execute_unprepared(
                r#"
CREATE INDEX "idx-task_notes-org" ON task_notes (organisation_id);
CREATE INDEX "idx-task_notes-task-created" ON task_notes (task_id, created_at);
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "task_notes").await
    }
}
