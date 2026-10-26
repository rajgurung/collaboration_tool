//! Document folders, one level deep. Owners and admins see every folder and
//! create, rename and delete them. Members see only the built-in Shared
//! folder, and add to it ([`Model::open_to`]).
use std::collections::HashMap;

use loco_rs::prelude::*;
use sea_orm::{
    sea_query::{ExprTrait, Func},
    DbBackend, FromQueryResult, SqlErr, Statement, TransactionTrait,
};
use serde::Deserialize;

pub use super::_entities::folders::{ActiveModel, Column, Entity, Model};
use super::{documents, field_error, logins, visible_text};

pub type Folders = Entity;

/// The name of the starter folder for logins.
pub const LOGINS: &str = "Logins";

/// The folder photos posted in chat channels go to.
pub const CHAT_PHOTOS: &str = "Chat photos";

/// The name of the built-in folder every member can see and add to.
pub const SHARED: &str = "Shared";

/// The Shared folder's note. The migration that added `kind` uses the same.
pub const SHARED_NOTE: &str =
    "Photos and files for the whole team. Everyone can open them and add to them.";

/// Values of `folders.kind`. Ordinary folders have none.
pub mod kind {
    pub const SHARED: &str = "shared";
}

/// The folders every organisation starts with: name, note and kind.
pub const STARTERS: [(&str, Option<&str>, Option<&str>); 3] = [
    ("Company", None, None),
    (
        LOGINS,
        Some("Shared logins for the team. Never put passwords in notes."),
        None,
    ),
    (SHARED, Some(SHARED_NOTE), Some(kind::SHARED)),
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
    /// The "Logins" folder holds logins only, never files, so photos of
    /// passwords have no obvious home.
    #[must_use]
    pub fn is_logins(&self) -> bool {
        self.name.eq_ignore_ascii_case(LOGINS)
    }

    /// The built-in folder every member can see and add to.
    #[must_use]
    pub fn is_shared(&self) -> bool {
        self.kind.as_deref() == Some(kind::SHARED)
    }

    /// The one Documents gate: whether someone may see this folder and the
    /// files in it. Owners and admins see every folder; members only Shared.
    #[must_use]
    pub fn open_to(&self, can_manage: bool) -> bool {
        can_manage || self.is_shared()
    }

    /// Shared and Logins keep their names and can't be deleted.
    #[must_use]
    pub fn is_built_in(&self) -> bool {
        self.is_shared() || self.is_logins()
    }

    /// Adds the starter folders to a new organisation.
    ///
    /// # Errors
    /// On database errors.
    pub async fn create_starters<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        created_by_id: i64,
    ) -> ModelResult<()> {
        for (name, note, kind) in STARTERS {
            ActiveModel {
                name: ActiveValue::Set(name.to_string()),
                note: ActiveValue::Set(note.map(str::to_string)),
                kind: ActiveValue::Set(kind.map(str::to_string)),
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

    /// The folder chat photos are stored in, made on first use. Two first
    /// photos at once both end up in the one folder: the loser of the unique
    /// name index finds the winner's.
    ///
    /// # Errors
    /// On database errors.
    pub async fn chat_photos<C: ConnectionTrait>(db: &C, org_id: i64) -> ModelResult<Self> {
        let find = || {
            Entity::find()
                .in_tenant(org_id)
                .filter(
                    Expr::expr(Func::lower(Expr::col(Column::Name))).eq(CHAT_PHOTOS.to_lowercase()),
                )
                .one(db)
        };
        if let Some(folder) = find().await? {
            return Ok(folder);
        }
        let created = ActiveModel {
            name: ActiveValue::Set(CHAT_PHOTOS.to_string()),
            note: ActiveValue::Set(Some("Photos posted in chat channels.".to_string())),
            ..Default::default()
        }
        .set_tenant(org_id)?
        .insert(db)
        .await;
        match created {
            Ok(folder) => Ok(folder),
            Err(err) if matches!(err.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) => {
                find().await?.ok_or(ModelError::EntityNotFound)
            }
            Err(err) => Err(err.into()),
        }
    }

    /// The organisation's Shared folder. The migration and
    /// [`Self::create_starters`] make it. If it is somehow missing, it is
    /// made here, under a lock on the organisation so two requests make one.
    ///
    /// # Errors
    /// On database errors, including a name clash with an ordinary folder.
    pub async fn shared(db: &DatabaseConnection, org_id: i64) -> ModelResult<Self> {
        let find = |db| {
            Entity::find()
                .in_tenant(org_id)
                .filter(Column::Kind.eq(kind::SHARED))
                .one(db)
        };
        if let Some(folder) = find(db).await? {
            return Ok(folder);
        }
        let txn = db.begin().await?;
        txn.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
            [format!("shared-folder:{org_id}").into()],
        ))
        .await?;
        let folder = match Entity::find()
            .in_tenant(org_id)
            .filter(Column::Kind.eq(kind::SHARED))
            .one(&txn)
            .await?
        {
            Some(folder) => folder,
            None => {
                tracing::warn!(org_id, "Shared folder was missing; making it");
                ActiveModel {
                    name: ActiveValue::Set(SHARED.to_string()),
                    note: ActiveValue::Set(Some(SHARED_NOTE.to_string())),
                    kind: ActiveValue::Set(Some(kind::SHARED.to_string())),
                    ..Default::default()
                }
                .set_tenant(org_id)?
                .insert(&txn)
                .await?
            }
        };
        txn.commit().await?;
        Ok(folder)
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
        if self.is_built_in() && params.name != self.name {
            return Err(field_error("name", "Built-in folders keep their name."));
        }
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
    /// `Message` for a built-in folder or one that still has files or
    /// logins, or database errors.
    pub async fn delete_if_empty<C: ConnectionTrait>(self, db: &C) -> ModelResult<()> {
        if self.is_built_in() {
            return Err(ModelError::Message(format!(
                "The {} folder is built in, so it can't be deleted.",
                self.name
            )));
        }
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
