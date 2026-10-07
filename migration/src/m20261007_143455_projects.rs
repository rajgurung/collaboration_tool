use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "projects",
            &[
                ("id", ColType::PkAuto),
                ("name", ColType::String),
                ("lane", ColType::String),
                ("status", ColType::String),
                ("progress", ColType::BigInteger),
                ("accent", ColType::String),
                ("summary", ColType::Text),
                ("sort_order", ColType::BigInteger),
            ],
            &[("organisation", ""), ("users?", "owner_id")],
        )
        .await?;
        m.get_connection()
            .execute_unprepared(
                r#"
CREATE INDEX "idx-projects-org-sort" ON projects (organisation_id, sort_order);
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "projects").await
    }
}
