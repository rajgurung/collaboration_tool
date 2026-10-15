use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

/// Reactions on task notes, the same six as chat. One row per person, note
/// and emoji. The unique index stops the same person adding the same emoji
/// twice, and leads with `task_note_id` so it also serves "all reactions for
/// these notes". The allowed emoji live in the model.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "note_reactions",
            &[("id", ColType::PkAuto), ("emoji", ColType::String)],
            &[("organisation", ""), ("task_note", ""), ("user", "")],
        )
        .await?;
        m.get_connection()
            .execute_unprepared(
                r#"
CREATE UNIQUE INDEX "idx-note-reactions-note-user-emoji" ON note_reactions (task_note_id, user_id, emoji);
CREATE INDEX "idx-note-reactions-org" ON note_reactions (organisation_id);
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "note_reactions").await
    }
}
