use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "meetings",
            &[
                ("id", ColType::PkAuto),
                ("title", ColType::String),
                ("held_on", ColType::Date),
                ("summary", ColType::Text),
                ("decisions", ColType::Text),
            ],
            &[("organisation", ""), ("users?", "created_by_id")],
        )
        .await?;
        m.get_connection()
            .execute_unprepared(
                r#"
CREATE INDEX "idx-meetings-org-held" ON meetings (organisation_id, held_on);
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "meetings").await
    }
}
