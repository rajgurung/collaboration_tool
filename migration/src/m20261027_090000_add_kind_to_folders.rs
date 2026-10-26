use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// The note on every organisation's Shared folder.
pub const SHARED_NOTE: &str =
    "Photos and files for the whole team. Everyone can open them and add to them.";

/// Marks a Shared folder left by an earlier down migration as built in
/// again. Moves any other folder called "Shared" out of the way, as "Shared
/// (old)", or "Shared (old 2)" and so on if that is taken, so the unique name
/// index can't fail. Then gives every organisation without one its built-in
/// Shared folder. Safe to run again.
#[must_use]
pub fn backfill_sql() -> String {
    format!(
        r#"
DO $$
DECLARE
  f record;
  candidate text;
  n int;
BEGIN
  -- A Shared left by an earlier down migration is ours: mark it again so
  -- members keep its files. The name index allows one per organisation.
  UPDATE folders d SET kind = 'shared'
  WHERE d.name = 'Shared' AND d.note = '{SHARED_NOTE}' AND d.kind IS NULL
    AND NOT EXISTS (
      SELECT 1 FROM folders s WHERE s.organisation_id = d.organisation_id AND s.kind = 'shared'
    );
  FOR f IN
    SELECT id, organisation_id FROM folders
    WHERE lower(name) = 'shared' AND kind IS NULL ORDER BY id
  LOOP
    n := 1;
    LOOP
      candidate := CASE WHEN n = 1 THEN 'Shared (old)' ELSE 'Shared (old ' || n || ')' END;
      EXIT WHEN NOT EXISTS (
        SELECT 1 FROM folders
        WHERE organisation_id = f.organisation_id AND lower(name) = lower(candidate)
      );
      n := n + 1;
    END LOOP;
    UPDATE folders SET name = candidate, updated_at = now() WHERE id = f.id;
  END LOOP;
END $$;
INSERT INTO folders (organisation_id, name, note, kind, created_at, updated_at)
SELECT o.id, 'Shared', '{SHARED_NOTE}', 'shared', now(), now() FROM organisations o
WHERE NOT EXISTS (
  SELECT 1 FROM folders f WHERE f.organisation_id = o.id AND f.kind = 'shared'
);"#
    )
}

/// Folders get a `kind`. Only the built-in Shared folder has one: members see
/// that folder and no other. At most one per organisation.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        let db = m.get_connection();
        db.execute_unprepared(
            r#"SET LOCAL lock_timeout = '5s';
ALTER TABLE folders ADD COLUMN kind text NULL;
CREATE UNIQUE INDEX "idx-folders-org-shared" ON folders (organisation_id) WHERE kind = 'shared';"#,
        )
        .await?;
        db.execute_unprepared(&backfill_sql()).await?;
        Ok(())
    }

    /// Drops the index and the column. The Shared folders stay, as ordinary
    /// folders called "Shared", and renamed "Shared (old)" folders keep
    /// their new names. Running up again marks those Shared folders again.
    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                r#"SET LOCAL lock_timeout = '5s';
DROP INDEX "idx-folders-org-shared";
ALTER TABLE folders DROP COLUMN kind;"#,
            )
            .await?;
        Ok(())
    }
}
