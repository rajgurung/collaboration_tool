use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "meeting_attendees",
            &[("id", ColType::PkAuto)],
            &[("organisation", ""), ("meeting", ""), ("user", "")],
        )
        .await?;
        m.get_connection()
            .execute_unprepared(
                r#"
CREATE UNIQUE INDEX "idx-meeting_attendees-meeting-user" ON meeting_attendees (meeting_id, user_id);
CREATE INDEX "idx-meeting_attendees-org" ON meeting_attendees (organisation_id);
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "meeting_attendees").await
    }
}
