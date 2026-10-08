use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// When each person closed the "Getting started" guide and saw the welcome
/// pop-up. Empty means not yet, so everyone, existing members included, sees both once.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        add_column(
            m,
            "memberships",
            "guide_dismissed_at",
            ColType::TimestampWithTimeZoneNull,
        )
        .await?;
        add_column(
            m,
            "memberships",
            "welcomed_at",
            ColType::TimestampWithTimeZoneNull,
        )
        .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        remove_column(m, "memberships", "guide_dismissed_at").await?;
        remove_column(m, "memberships", "welcomed_at").await?;
        Ok(())
    }
}
