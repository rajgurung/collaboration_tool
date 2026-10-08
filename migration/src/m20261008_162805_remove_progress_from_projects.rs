use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Project progress now comes from the share of its tasks that are done, so
/// the hand-typed percentage goes. Rolling back brings the column back at 0.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        remove_column(m, "projects", "progress").await
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE projects ADD COLUMN progress BIGINT NOT NULL DEFAULT 0",
            )
            .await?;
        Ok(())
    }
}
