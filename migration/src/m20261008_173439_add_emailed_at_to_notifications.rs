use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// When a mention was emailed to someone who hadn't seen it. Notifications
/// that already exist count as handled, so nothing old is emailed on deploy.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        add_column(
            m,
            "notifications",
            "emailed_at",
            ColType::TimestampWithTimeZoneNull,
        )
        .await?;
        m.get_connection()
            .execute_unprepared("UPDATE notifications SET emailed_at = NOW()")
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        remove_column(m, "notifications", "emailed_at").await?;
        Ok(())
    }
}
