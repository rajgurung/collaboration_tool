use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

/// Removing a member: `memberships.status` can now be `removed`, with when and
/// by whom. Rolling back turns removed members into declined ones, which keeps
/// them locked out.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        add_column(
            m,
            "memberships",
            "removed_at",
            ColType::TimestampWithTimeZoneNull,
        )
        .await?;
        m.get_connection()
            .execute_unprepared(
                r#"ALTER TABLE memberships ADD COLUMN removed_by_id bigint
  CONSTRAINT "fk-memberships-removed_by_id-to-users" REFERENCES users (id) ON DELETE SET NULL ON UPDATE CASCADE;"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "UPDATE memberships SET status = 'rejected' WHERE status = 'removed';",
            )
            .await?;
        remove_column(m, "memberships", "removed_by_id").await?;
        remove_column(m, "memberships", "removed_at").await?;
        Ok(())
    }
}
