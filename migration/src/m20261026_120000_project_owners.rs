use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Copies each project's single owner into `project_owners`. Only when the
/// owner is a member of the project's organisation, which the app always
/// required; the composite keys below would refuse anything else.
pub const COPY_OWNERS: &str = r"
INSERT INTO project_owners (organisation_id, project_id, user_id, created_at, updated_at)
SELECT p.organisation_id, p.id, p.owner_id, NOW(), NOW()
  FROM projects p
 WHERE p.owner_id IS NOT NULL
   AND EXISTS (SELECT 1 FROM memberships m
                WHERE m.organisation_id = p.organisation_id AND m.user_id = p.owner_id)
ON CONFLICT (project_id, user_id) DO NOTHING;
";

/// For `down`: each project's first-added owner goes back into `owner_id`,
/// and a project with no owners gets NULL.
pub const FIRST_OWNER_BACK: &str = r"
UPDATE projects SET owner_id = (
  SELECT user_id FROM project_owners o WHERE o.project_id = projects.id ORDER BY o.id LIMIT 1
);
";

/// A project has zero or more owners. `projects.owner_id` stays, written as
/// a mirror of the first owner and never read, until a later migration
/// drops it.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "project_owners",
            &[("id", ColType::PkAuto)],
            &[("organisation", ""), ("project", ""), ("user", "")],
        )
        .await?;
        let db = m.get_connection();
        db.execute_unprepared(
            r#"
CREATE UNIQUE INDEX "idx-project_owners-project-user" ON project_owners (project_id, user_id);
CREATE INDEX "idx-project_owners-org-user" ON project_owners (organisation_id, user_id);

-- Org consistency in the database, not only in code: the project and the
-- person must both belong to the row's organisation.
CREATE UNIQUE INDEX "idx-projects-id-org" ON projects (id, organisation_id);
ALTER TABLE project_owners
  ADD CONSTRAINT "fk-project_owners-project-same-org"
  FOREIGN KEY (project_id, organisation_id) REFERENCES projects (id, organisation_id)
  ON DELETE CASCADE ON UPDATE CASCADE,
  ADD CONSTRAINT "fk-project_owners-member-same-org"
  FOREIGN KEY (organisation_id, user_id) REFERENCES memberships (organisation_id, user_id)
  ON DELETE CASCADE ON UPDATE CASCADE;
"#,
        )
        .await?;
        db.execute_unprepared(COPY_OWNERS).await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(FIRST_OWNER_BACK)
            .await?;
        drop_table(m, "project_owners").await?;
        m.get_connection()
            .execute_unprepared(r#"DROP INDEX IF EXISTS "idx-projects-id-org";"#)
            .await?;
        Ok(())
    }
}
