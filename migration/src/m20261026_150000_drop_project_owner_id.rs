use sea_orm_migration::prelude::*;

use crate::m20261026_120000_project_owners::FIRST_OWNER_BACK;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Owners live in `project_owners`; the `owner_id` mirror of the first owner
/// was written but never read. Dropping the column drops its foreign key
/// too. It had no index.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "SET LOCAL lock_timeout = '5s';
ALTER TABLE projects DROP COLUMN owner_id;",
            )
            .await?;
        Ok(())
    }

    /// Restores the column and its foreign key as the projects migration
    /// made them, with each project's first-added owner.
    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        let db = m.get_connection();
        db.execute_unprepared(
            r#"
ALTER TABLE projects ADD COLUMN owner_id bigint NULL;
ALTER TABLE projects
  ADD CONSTRAINT "fk-projects-owner_id-to-users"
  FOREIGN KEY (owner_id) REFERENCES users (id) ON DELETE SET NULL;
"#,
        )
        .await?;
        db.execute_unprepared(FIRST_OWNER_BACK).await?;
        Ok(())
    }
}
