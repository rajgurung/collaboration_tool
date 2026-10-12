//! Document folders, one level deep. Every member can see every folder and
//! add to it; owners and admins create, rename and delete them.
use std::collections::HashMap;

use loco_rs::prelude::*;
use sea_orm::{
    sea_query::{ExprTrait, Func},
    FromQueryResult, SqlErr,
};
use serde::Deserialize;

pub use super::_entities::folders::{ActiveModel, Column, Entity, Model};
use super::{documents, field_error, logins, visible_text};

pub type Folders = Entity;

/// The name of the starter folder for logins.
pub const LOGINS: &str = "Logins";

/// The folders every organisation starts with, and their notes.
pub const STARTERS: [(&str, Option<&str>); 2] = [
    ("Company", None),
    (
        LOGINS,
        Some("Shared logins for the team. Never put passwords in notes."),
    ),
];

#[derive(Debug, Deserialize, Validate)]
pub struct FolderParams {
    #[validate(length(
        min = 1,
        max = 60,
        message = "Give the folder a name (up to 60 characters)."
    ))]
    pub name: String,
    #[serde(default)]
    #[validate(length(max = 500, message = "Keep the note under 500 characters."))]
    pub note: String,
}

impl FolderParams {
    fn cleaned(&self) -> Self {
        Self {
            name: visible_text(&self.name, false).trim().to_string(),
            note: visible_text(&self.note, true).trim().to_string(),
        }
    }
}

/// Item count and newest item per folder, from one grouped query per table.
#[derive(Debug, FromQueryResult)]
struct Stats {
    folder_id: i64,
    items: i64,
    latest: Option<DateTimeWithTimeZone>,
}

/// A folder with what is in it, for the folder list.
#[derive(Debug)]
pub struct Summary {
    pub folder: Model,
    pub files: u64,
    pub logins: u64,
    pub updated_at: Option<DateTimeWithTimeZone>,
}

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        if !insert && self.updated_at.is_unchanged() {
            let mut this = self;
            this.updated_at = sea_orm::ActiveValue::Set(chrono::Utc::now().into());
            Ok(this)
        } else {
            Ok(self)
        }
    }
}

fn taken() -> ModelError {
    field_error("name", "There's already a folder with that name.")
}

/// A unique index violation on the name becomes the same field error the
/// check before it gives, for two people saving the same name at once.
fn unique_name(err: DbErr) -> ModelError {
    if matches!(err.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) {
        taken()
    } else {
        err.into()
    }
}

impl Model {
    /// Adds the starter folders to a new organisation.
    ///
    /// # Errors
    /// On database errors.
    pub async fn create_starters<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        created_by_id: i64,
    ) -> ModelResult<()> {
        for (name, note) in STARTERS {
            ActiveModel {
                name: ActiveValue::Set(name.to_string()),
                note: ActiveValue::Set(note.map(str::to_string)),
                created_by_id: ActiveValue::Set(Some(created_by_id)),
                ..Default::default()
            }
            .set_tenant(org_id)?
            .insert(db)
            .await?;
        }
        Ok(())
    }

    /// Every folder in the organisation by name, with its file count and
    /// latest upload.
    ///
    /// # Errors
    /// On database errors.
    pub async fn list_for_org<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
    ) -> ModelResult<Vec<Summary>> {
        let folders = Entity::find()
            .in_tenant(org_id)
            .order_by_asc(Expr::expr(Func::lower(Expr::col(Column::Name))))
            .order_by_asc(Column::Id)
            .all(db)
            .await?;
        let files: HashMap<i64, Stats> = documents::Entity::find()
            .in_tenant(org_id)
            .select_only()
            .column(documents::Column::FolderId)
            .column_as(
                Expr::expr(Func::count(Expr::col(documents::Column::Id))),
                "items",
            )
            .column_as(
                Expr::expr(Func::max(Expr::col(documents::Column::CreatedAt))),
                "latest",
            )
            .group_by(documents::Column::FolderId)
            .into_model::<Stats>()
            .all(db)
            .await?
            .into_iter()
            .map(|s| (s.folder_id, s))
            .collect();
        let logins: HashMap<i64, Stats> = logins::Entity::find()
            .in_tenant(org_id)
            .select_only()
            .column(logins::Column::FolderId)
            .column_as(
                Expr::expr(Func::count(Expr::col(logins::Column::Id))),
                "items",
            )
            .column_as(
                Expr::expr(Func::max(Expr::col(logins::Column::UpdatedAt))),
                "latest",
            )
            .group_by(logins::Column::FolderId)
            .into_model::<Stats>()
            .all(db)
            .await?
            .into_iter()
            .map(|s| (s.folder_id, s))
            .collect();
        let count =
            |stats: Option<&Stats>| stats.map_or(0, |s| u64::try_from(s.items).unwrap_or(0));
        Ok(folders
            .into_iter()
            .map(|folder| {
                let (f, l) = (files.get(&folder.id), logins.get(&folder.id));
                Summary {
                    files: count(f),
                    logins: count(l),
                    updated_at: std::cmp::max(f.and_then(|s| s.latest), l.and_then(|s| s.latest)),
                    folder,
                }
            })
            .collect())
    }

    /// # Errors
    /// `EntityNotFound` when the folder is not in this organisation.
    pub async fn find_in_org<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        id: i64,
    ) -> ModelResult<Self> {
        Entity::find_by_id(id)
            .in_tenant(org_id)
            .one(db)
            .await?
            .ok_or(ModelError::EntityNotFound)
    }

    async fn name_taken<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        name: &str,
        except: Option<i64>,
    ) -> ModelResult<bool> {
        let mut query = Entity::find()
            .in_tenant(org_id)
            .filter(Expr::expr(Func::lower(Expr::col(Column::Name))).eq(name.to_lowercase()));
        if let Some(id) = except {
            query = query.filter(Column::Id.ne(id));
        }
        Ok(query.one(db).await?.is_some())
    }

    /// # Errors
    /// Field errors for a bad or taken name, or database errors.
    pub async fn create<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        created_by_id: i64,
        params: &FolderParams,
    ) -> ModelResult<Self> {
        let params = params.cleaned();
        ValidatorTrait::validate(&params)?;
        if Self::name_taken(db, org_id, &params.name, None).await? {
            return Err(taken());
        }
        ActiveModel {
            name: ActiveValue::Set(params.name),
            note: ActiveValue::Set(Some(params.note).filter(|n| !n.is_empty())),
            created_by_id: ActiveValue::Set(Some(created_by_id)),
            ..Default::default()
        }
        .set_tenant(org_id)?
        .insert(db)
        .await
        .map_err(unique_name)
    }

    /// Renames the folder and changes its note.
    ///
    /// # Errors
    /// Field errors for a bad or taken name, or database errors.
    pub async fn edit<C: ConnectionTrait>(
        self,
        db: &C,
        params: &FolderParams,
    ) -> ModelResult<Self> {
        let params = params.cleaned();
        ValidatorTrait::validate(&params)?;
        if Self::name_taken(db, self.organisation_id, &params.name, Some(self.id)).await? {
            return Err(taken());
        }
        let mut folder = self.into_active_model();
        folder.name = ActiveValue::Set(params.name);
        folder.note = ActiveValue::Set(Some(params.note).filter(|n| !n.is_empty()));
        folder.update(db).await.map_err(unique_name)
    }

    /// Deletes an empty folder. The database refuses a folder with files or
    /// logins even if one arrives after this check.
    ///
    /// # Errors
    /// `Message` when the folder still has files or logins, or database errors.
    pub async fn delete_if_empty<C: ConnectionTrait>(self, db: &C) -> ModelResult<()> {
        if documents::Model::count_in_folder(db, self.organisation_id, self.id).await? > 0
            || logins::Model::count_in_folder(db, self.organisation_id, self.id).await? > 0
        {
            return Err(ModelError::msg(
                "Only empty folders can be deleted. Move or delete what is in it first.",
            ));
        }
        self.into_active_model().delete(db).await?;
        Ok(())
    }
}

impl loco_rs::prelude::TenantEntity for Entity {
    type TenantId = i64;

    fn tenant_column() -> Column {
        Column::OrganisationId
    }
}
