//! Each organisation's data key (DEK) for login secrets, stored wrapped by
//! the KEK. See [`super::secrets::crypto`].
use loco_rs::prelude::*;

pub use super::_entities::org_keys::{ActiveModel, Column, Entity, Model};
use super::secrets::crypto::{self, Kek, Key};

pub type OrgKeys = Entity;

/// The DEK version new keys get.
pub const DEK_VERSION: i32 = 1;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {}

impl Model {
    /// The organisation's data key and its version. The first caller makes
    /// it; if two make one at once, both use the one that was saved first.
    ///
    /// # Errors
    /// On database errors, or a key that does not unwrap with this KEK.
    pub async fn dek_for<C: ConnectionTrait>(
        db: &C,
        kek: &Kek,
        org_id: i64,
    ) -> ModelResult<(Key, i32)> {
        let row = match Entity::find_by_id(org_id).one(db).await? {
            Some(row) => row,
            None => {
                let sealed = kek.wrap(org_id, &crypto::random_key())?;
                Entity::insert(ActiveModel {
                    organisation_id: ActiveValue::Set(org_id),
                    wrapped_dek: ActiveValue::Set(sealed.ciphertext),
                    nonce: ActiveValue::Set(sealed.nonce),
                    kek_version: ActiveValue::Set(kek.version),
                    dek_version: ActiveValue::Set(DEK_VERSION),
                    ..Default::default()
                })
                .on_conflict_do_nothing()
                .exec(db)
                .await?;
                Entity::find_by_id(org_id)
                    .one(db)
                    .await?
                    .ok_or(ModelError::EntityNotFound)?
            }
        };
        let dek = kek.unwrap(org_id, row.kek_version, &row.wrapped_dek, &row.nonce)?;
        Ok((dek, row.dek_version))
    }
}

/// What [`Model::rewrap`] did, in counts. It never holds key material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rewrapped {
    pub from: i32,
    pub to: i32,
    pub rewrapped: u64,
    /// Rows still on `from` afterwards. Zero means the previous KEK can go.
    pub left: u64,
}

impl Model {
    /// How many DEKs are wrapped by each KEK version, lowest version first.
    ///
    /// # Errors
    /// On database errors.
    pub async fn count_by_kek_version<C: ConnectionTrait>(db: &C) -> ModelResult<Vec<(i32, u64)>> {
        let counts: Vec<(i32, i64)> = Entity::find()
            .select_only()
            .column(Column::KekVersion)
            .column_as(Column::OrganisationId.count(), "count")
            .group_by(Column::KekVersion)
            .order_by_asc(Column::KekVersion)
            .into_tuple()
            .all(db)
            .await?;
        Ok(counts
            .into_iter()
            .map(|(version, count)| (version, u64::try_from(count).unwrap_or_default()))
            .collect())
    }

    /// Moves every DEK wrapped by the previous KEK onto the current one,
    /// all or nothing: any failure rolls the whole batch back. `None` when
    /// no previous KEK is set. The DEKs themselves, and so every login's
    /// ciphertext, stay the same.
    ///
    /// Safe to run from two instances at once: the rows are locked, and the
    /// second run finds them already on the current version.
    ///
    /// # Errors
    /// On database errors, or a DEK the previous KEK cannot unwrap.
    pub async fn rewrap(db: &DatabaseConnection, kek: &Kek) -> ModelResult<Option<Rewrapped>> {
        let Some(from) = kek.previous_version() else {
            return Ok(None);
        };
        let txn = db.begin().await?;
        let rows = Entity::find()
            .filter(Column::KekVersion.eq(from))
            .lock_exclusive()
            .all(&txn)
            .await?;
        for row in &rows {
            let dek = kek.unwrap(row.organisation_id, from, &row.wrapped_dek, &row.nonce)?;
            let sealed = kek.wrap(row.organisation_id, &dek)?;
            let updated = Entity::update_many()
                .col_expr(Column::WrappedDek, Expr::value(sealed.ciphertext))
                .col_expr(Column::Nonce, Expr::value(sealed.nonce))
                .col_expr(Column::KekVersion, Expr::value(kek.version))
                .col_expr(
                    Column::UpdatedAt,
                    Expr::value(DateTimeWithTimeZone::from(chrono::Utc::now())),
                )
                .filter(Column::OrganisationId.eq(row.organisation_id))
                .filter(Column::KekVersion.eq(from))
                .exec(&txn)
                .await?;
            if updated.rows_affected != 1 {
                return Err(ModelError::msg(
                    "an organisation key changed during the rewrap",
                ));
            }
        }
        txn.commit().await?;
        let left = Self::count_by_kek_version(db)
            .await?
            .into_iter()
            .find(|(version, _)| *version == from)
            .map_or(0, |(_, count)| count);
        Ok(Some(Rewrapped {
            from,
            to: kek.version,
            rewrapped: rows.len() as u64,
            left,
        }))
    }
}
