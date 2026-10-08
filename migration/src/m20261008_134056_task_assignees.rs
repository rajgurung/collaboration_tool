use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Tasks can have several assignees. Each existing owner becomes the task's first assignee.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "task_assignees",
            &[("id", ColType::PkAuto)],
            &[("organisation", ""), ("task", ""), ("user", "")],
        )
        .await?;
        m.get_connection()
            .execute_unprepared(
                r#"
CREATE UNIQUE INDEX "idx-task_assignees-task-user" ON task_assignees (task_id, user_id);
CREATE INDEX "idx-task_assignees-org-user" ON task_assignees (organisation_id, user_id);
INSERT INTO task_assignees (organisation_id, task_id, user_id, created_at, updated_at)
SELECT organisation_id, id, owner_id, NOW(), NOW() FROM tasks WHERE owner_id IS NOT NULL;
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "task_assignees").await
    }
}
