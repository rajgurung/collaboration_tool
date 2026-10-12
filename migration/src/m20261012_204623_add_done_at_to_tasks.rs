use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

/// When a task was finished, so the board can show the latest done tasks and
/// the Done view can group them by week and month. Tasks already done get
/// their last update time. Rolling back drops the column.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        add_column(m, "tasks", "done_at", ColType::TimestampWithTimeZoneNull).await?;
        m.get_connection()
            .execute_unprepared("UPDATE tasks SET done_at = updated_at WHERE status = 'done';")
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        remove_column(m, "tasks", "done_at").await?;
        Ok(())
    }
}
