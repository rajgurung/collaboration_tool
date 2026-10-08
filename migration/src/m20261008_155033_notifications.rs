use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Notifications for one person: a mention, an assignment, project activity.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "notifications",
            &[
                ("id", ColType::PkAuto),
                ("kind", ColType::String),
                ("body", ColType::Text),
                ("link", ColType::String),
                ("read_at", ColType::TimestampWithTimeZoneNull),
            ],
            &[("organisation", ""), ("user", ""), ("users?", "actor_id")],
        )
        .await?;
        m.get_connection()
            .execute_unprepared(
                r#"CREATE INDEX "idx-notifications-org-user-created" ON notifications (organisation_id, user_id, created_at);"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "notifications").await
    }
}
