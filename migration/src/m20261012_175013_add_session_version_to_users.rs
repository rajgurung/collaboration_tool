use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

/// Every session token carries the user's session version. Bumping it signs
/// the user out everywhere. Existing rows start at 0.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        add_column(
            m,
            "users",
            "session_version",
            ColType::IntegerWithDefault(0),
        )
        .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        remove_column(m, "users", "session_version").await?;
        Ok(())
    }
}
