use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Assignees replace the single owner (see the task_assignees migration).
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        remove_column(m, "tasks", "owner_id").await?;
        Ok(())
    }

    /// Restores the column with each task's first assignee as its owner.
    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        add_column(m, "tasks", "owner_id", ColType::BigIntegerNull).await?;
        m.get_connection()
            .execute_unprepared(
                r"
UPDATE tasks SET owner_id = (
  SELECT user_id FROM task_assignees a WHERE a.task_id = tasks.id ORDER BY a.id LIMIT 1
);
",
            )
            .await?;
        Ok(())
    }
}
