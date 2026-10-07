use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "organisations",
            &[
                ("id", ColType::PkAuto),
                ("name", ColType::String),
                ("slug", ColType::StringUniq),
            ],
            &[("users?", "created_by_id")],
        )
        .await
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "organisations").await
    }
}
