use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

/// One row per person, message and emoji. The unique index stops the same
/// person adding the same emoji twice, and leads with `message_id` so it also
/// serves "all reactions for these messages". The six allowed emoji live in
/// the model, so the set can change without a migration.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "reactions",
            &[("id", ColType::PkAuto), ("emoji", ColType::String)],
            &[("organisation", ""), ("message", ""), ("user", "")],
        )
        .await?;
        m.get_connection()
            .execute_unprepared(
                r#"
CREATE UNIQUE INDEX "idx-reactions-message-user-emoji" ON reactions (message_id, user_id, emoji);
CREATE INDEX "idx-reactions-org" ON reactions (organisation_id);
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "reactions").await
    }
}
