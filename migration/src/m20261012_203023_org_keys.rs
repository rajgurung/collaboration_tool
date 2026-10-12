use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

/// One data key per organisation for login secrets, stored wrapped by the
/// `SECRETS_KEK`. Rows are added the first time an organisation needs one,
/// because migrations do not have the KEK.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "org_keys",
            &[
                ("wrapped_dek", ColType::Blob),
                ("nonce", ColType::Blob),
                ("kek_version", ColType::Integer),
                ("dek_version", ColType::Integer),
            ],
            &[("organisation", "")],
        )
        .await?;
        m.get_connection()
            .execute_unprepared(r#"ALTER TABLE org_keys ADD PRIMARY KEY (organisation_id);"#)
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "org_keys").await
    }
}
