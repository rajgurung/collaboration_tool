use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// A task can be a small chore with no project. Rolling back fails while any
/// such task exists; move or delete them first.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE tasks ALTER COLUMN project_id DROP NOT NULL")
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE tasks ALTER COLUMN project_id SET NOT NULL")
            .await?;
        Ok(())
    }
}
