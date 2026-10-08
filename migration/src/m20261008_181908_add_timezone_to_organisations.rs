use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// The time zone an organisation reads times in, as an IANA name. Times stay
/// in UTC in the database; this only changes how they're shown.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE organisations ADD COLUMN timezone VARCHAR NOT NULL DEFAULT 'Europe/London'",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE organisations DROP COLUMN timezone")
            .await?;
        Ok(())
    }
}
