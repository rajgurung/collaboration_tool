use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Named Tasks setups (whose tasks, lanes, board or list, search), private to
/// each person. One can be their default.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "saved_views",
            &[
                ("id", ColType::PkAuto),
                ("name", ColType::String),
                ("scope", ColType::String),
                ("lanes", ColType::String),
                ("layout", ColType::String),
                ("q", ColType::String),
                ("is_default", ColType::Boolean),
            ],
            &[("organisation", ""), ("user", "")],
        )
        .await?;
        m.get_connection()
            .execute_unprepared(
                r#"CREATE INDEX "idx-saved_views-org-user" ON saved_views (organisation_id, user_id);"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "saved_views").await
    }
}
