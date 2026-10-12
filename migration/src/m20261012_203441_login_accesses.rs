use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

/// Members named on a login, who may see its password as well as owners and
/// admins.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(m, "login_accesses", &[], &[("login", ""), ("user", "")]).await?;
        for sql in [
            r#"ALTER TABLE login_accesses ADD PRIMARY KEY (login_id, user_id);"#,
            r#"CREATE INDEX "idx-login_accesses-user" ON login_accesses (user_id);"#,
        ] {
            m.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "login_accesses").await
    }
}
