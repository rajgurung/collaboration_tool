use sea_orm_migration::prelude::*;

/// A user can now hold one membership per organisation instead of one in
/// total. `down` only works while every user has at most one membership:
/// once anyone has two, the unique index fails and nothing changes.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                r#"
SET LOCAL lock_timeout = '5s';
CREATE UNIQUE INDEX "idx-memberships-org-user" ON memberships (organisation_id, user_id);
CREATE INDEX "idx-memberships-user" ON memberships (user_id);
DROP INDEX "idx-memberships-user_id";
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        // The unique index first, so a failure leaves everything as it was.
        m.get_connection()
            .execute_unprepared(
                r#"
SET LOCAL lock_timeout = '5s';
CREATE UNIQUE INDEX "idx-memberships-user_id" ON memberships (user_id);
DROP INDEX "idx-memberships-user";
DROP INDEX "idx-memberships-org-user";
"#,
            )
            .await?;
        Ok(())
    }
}
