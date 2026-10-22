use sea_orm_migration::prelude::*;

/// Projects and tasks record the Claude connection (the access token's
/// `grant_id`) that made them, so Claude can undo its own mistakes. No
/// foreign key: token rows are pruned 30 days after they are revoked, and the
/// mark should outlive them. The partial indexes serve "made by this
/// connection lately" (the delete tools and the rate brake).
///
/// `claude_update_notices` has one row per connection that existed before
/// writes had to name their organisation. Those connections get a notice in
/// tool results until they make such a write, for at most 14 days, and one
/// bell notification (`announced_at`).
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                r#"
SET LOCAL lock_timeout = '5s';
CREATE TABLE claude_update_notices (
  grant_id uuid PRIMARY KEY,
  organisation_id bigint NOT NULL,
  user_id bigint NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  announced_at timestamptz NULL,
  written_at timestamptz NULL
);
INSERT INTO claude_update_notices (grant_id, organisation_id, user_id)
SELECT DISTINCT ON (grant_id) grant_id, organisation_id, user_id
  FROM access_tokens
 WHERE revoked_at IS NULL
   AND (kind = 'personal' OR refresh_expires_at > now())
 ORDER BY grant_id, id;
ALTER TABLE projects ADD COLUMN claude_grant_id uuid NULL;
ALTER TABLE tasks ADD COLUMN claude_grant_id uuid NULL;
CREATE INDEX "idx-projects-claude_grant"
  ON projects (claude_grant_id, created_at) WHERE claude_grant_id IS NOT NULL;
CREATE INDEX "idx-tasks-claude_grant"
  ON tasks (claude_grant_id, created_at) WHERE claude_grant_id IS NOT NULL;
"#,
            )
            .await?;
        Ok(())
    }

    /// Drops the columns (their indexes go with them), the notice table, and
    /// the bell notices it sent. Only the "made with Claude" marks are lost.
    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                r"
ALTER TABLE projects DROP COLUMN claude_grant_id;
ALTER TABLE tasks DROP COLUMN claude_grant_id;
DROP TABLE claude_update_notices;
DELETE FROM notifications WHERE kind = 'connector';
",
            )
            .await?;
        Ok(())
    }
}
