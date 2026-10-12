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
