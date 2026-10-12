use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

/// Failed sign-ins and password reset requests, for rate limits. Emails are
/// stored only as a SHA-256 of the normalised address, so unknown emails can
/// be counted too. Rows older than a day are deleted as new ones arrive.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "auth_attempts",
            &[
                ("id", ColType::PkAuto),
                ("email_hash", ColType::String),
                ("ip", ColType::String),
                ("kind", ColType::String),
            ],
            &[("user?", "")],
        )
        .await?;
        for sql in [
            r#"CREATE INDEX "idx-auth_attempts-email" ON auth_attempts (email_hash, created_at);"#,
            r#"CREATE INDEX "idx-auth_attempts-ip" ON auth_attempts (ip, created_at);"#,
            r#"CREATE INDEX "idx-auth_attempts-created_at" ON auth_attempts (created_at);"#,
        ] {
            m.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "auth_attempts").await
    }
}
