use sea_orm_migration::prelude::*;

/// A project can be marked done: `closed_at` and `closed_by_id` say when and
/// by whom, and its lane is kept for a reopen. `closed_by_id` goes NULL if
/// that user is deleted. A task the close marked done remembers its earlier
/// status in `status_before_close`, so a reopen can put it back. Every
/// existing row gets NULL, so every project stays open. No index: a team has
/// tens of projects, and every query already filters by organisation.
///
/// SQL rather than `add_reference`, which makes a non-null column named after
/// the target table.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                r#"
SET LOCAL lock_timeout = '5s';
ALTER TABLE projects
  ADD COLUMN closed_at timestamptz NULL,
  ADD COLUMN closed_by_id bigint NULL
  REFERENCES users (id) ON DELETE SET NULL ON UPDATE CASCADE;
ALTER TABLE tasks
  ADD COLUMN status_before_close varchar NULL;
"#,
            )
            .await?;
        Ok(())
    }

    /// Drops the three columns. Closed projects show again in their old
    /// lanes at 100%, and their tasks stay done. Who closed them, when, and
    /// the saved statuses are lost, so a reopen can no longer put those tasks
    /// back.
    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE projects DROP COLUMN closed_at, DROP COLUMN closed_by_id;
ALTER TABLE tasks DROP COLUMN status_before_close;",
            )
            .await?;
        Ok(())
    }
}
