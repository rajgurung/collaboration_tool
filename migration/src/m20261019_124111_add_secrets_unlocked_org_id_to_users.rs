use sea_orm_migration::prelude::*;

/// Ties a logins unlock to the organisation it happened in, so it never
/// counts in another one. Unlocks in progress get no org and must be redone.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                r#"
SET LOCAL lock_timeout = '5s';
ALTER TABLE users ADD COLUMN secrets_unlocked_org_id bigint
  CONSTRAINT "fk-users-secrets_unlocked_org_id-to-organisations"
  REFERENCES organisations (id) ON DELETE SET NULL ON UPDATE CASCADE;
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                r"
SET LOCAL lock_timeout = '5s';
ALTER TABLE users DROP COLUMN secrets_unlocked_org_id;
",
            )
            .await?;
        Ok(())
    }
}
