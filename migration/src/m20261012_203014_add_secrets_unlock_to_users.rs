use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        add_column(
            m,
            "users",
            "secrets_unlocked_until",
            ColType::TimestampWithTimeZoneNull,
        )
        .await?;
        add_column(m, "users", "secrets_unlocked_sid", ColType::StringNull).await?;
        add_column(m, "users", "locked_at", ColType::TimestampWithTimeZoneNull).await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        remove_column(m, "users", "secrets_unlocked_until").await?;
        remove_column(m, "users", "secrets_unlocked_sid").await?;
        remove_column(m, "users", "locked_at").await?;
        Ok(())
    }
}
