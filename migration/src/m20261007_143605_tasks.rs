use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "tasks",
            &[
                ("id", ColType::PkAuto),
                ("title", ColType::String),
                ("status", ColType::String),
                ("due_on", ColType::DateNull),
                ("priority", ColType::String),
                ("sort_order", ColType::BigInteger),
            ],
            &[
                ("organisation", ""),
                ("project", ""),
                ("users?", "owner_id"),
            ],
        )
        .await?;
        m.get_connection()
            .execute_unprepared(
                r#"
CREATE INDEX "idx-tasks-org-sort" ON tasks (organisation_id, sort_order);
CREATE INDEX "idx-tasks-project_id" ON tasks (project_id);
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "tasks").await
    }
}
