use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "memberships",
            &[
                ("id", ColType::PkAuto),
                ("username", ColType::String),
                ("role", ColType::String),
                ("status", ColType::String),
                ("approved_at", ColType::TimestampWithTimeZoneNull),
            ],
            &[
                ("organisation", ""),
                ("user", ""),
                ("users?", "approved_by_id"),
            ],
        )
        .await?;
        m.get_connection()
            .execute_unprepared(
                r#"
CREATE UNIQUE INDEX "idx-memberships-user_id" ON memberships (user_id);
CREATE UNIQUE INDEX "idx-memberships-org-username" ON memberships (organisation_id, lower(username));
CREATE INDEX "idx-memberships-org-status" ON memberships (organisation_id, status);
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "memberships").await
    }
}
