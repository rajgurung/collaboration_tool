//! `SeaORM` Entity, written by hand like sea-orm-codegen 2.0 output

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "claude_update_notices")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub grant_id: Uuid,
    pub organisation_id: i64,
    pub user_id: i64,
    pub created_at: DateTimeWithTimeZone,
    pub announced_at: Option<DateTimeWithTimeZone>,
    pub written_at: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
