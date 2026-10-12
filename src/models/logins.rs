//! Shared logins, kept in folders next to files. Title, username, website
//! and notes are plain text that every member can read. The password and
//! recovery codes are encrypted with the organisation's data key; they are
//! decrypted only to show or copy them, for a session that re-entered the
//! app password, and every time is logged.
use loco_rs::prelude::*;
use sea_orm::{DbBackend, Statement};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

pub use super::_entities::logins::{ActiveModel, Column, Entity, Model};
use super::{
    field_error, folders, login_accesses,
    login_events::{self, action, Client, Entry},
    memberships::{self, role},
    org_keys,
    secrets::crypto::{self, Kek, Key},
    visible_text,
};
use crate::extractors::current_member::CurrentMember;

pub type Logins = Entity;

/// The encrypted fields, as named in their associated data.
pub mod field {
    pub const PASSWORD: &str = "password";
    pub const RECOVERY: &str = "recovery";
}

const PASSWORD_MAX: usize = 500;
const RECOVERY_MAX: usize = 4000;

fn empty_secret() -> SecretString {
    SecretString::from("")
}

/// The add and edit form. The password and recovery codes are
/// `SecretString`, so they never print; there is no `Debug` on purpose.
#[derive(Deserialize)]
pub struct LoginParams {
    pub title: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub notes: String,
    /// Required when adding. Left empty when editing, it stays as it is.
    #[serde(default = "empty_secret")]
    pub password: SecretString,
    /// Optional. Left empty when editing, the saved codes stay.
    #[serde(default = "empty_secret")]
    pub recovery: SecretString,
    /// Editing only: delete the saved recovery codes.
    #[serde(default)]
    pub remove_recovery: Option<String>,
    /// Members named on the login, by user id.
    #[serde(default)]
    pub access: Vec<i64>,
    /// Editing only: the folder to move it to.
    #[serde(default)]
    pub folder_id: Option<i64>,
}

/// The plain fields after cleaning, with the secrets' lengths.
#[derive(Debug, Validate)]
struct Checked {
    #[validate(length(
        min = 1,
        max = 100,
        message = "Give the login a name (up to 100 characters)."
    ))]
    title: String,
    #[validate(length(max = 200, message = "Keep the username under 200 characters."))]
    username: String,
    #[validate(length(max = 500, message = "Keep the website under 500 characters."))]
    url: String,
    #[validate(length(max = 2000, message = "Keep the notes under 2,000 characters."))]
    notes: String,
    #[validate(range(max = PASSWORD_MAX, message = "Keep the password under 500 characters."))]
    password: usize,
    #[validate(range(max = RECOVERY_MAX, message = "Keep the recovery codes under 4,000 characters."))]
    recovery: usize,
}

/// A website as typed, made into a full `http` or `https` URL. "example.com"
/// becomes "https://example.com/". `None` for any other kind of link, such
/// as `javascript:`, or text that is not a web address.
#[must_use]
pub fn clean_url(input: &str) -> Option<String> {
    let typed = input.trim();
    if typed.is_empty() {
        return Some(String::new());
    }
    // "name:..." with no "//" is a scheme (javascript:, mailto:, data:),
    // unless it is a host and port such as "localhost:8080".
    let scheme = typed.split_once(':').is_some_and(|(name, rest)| {
        name.chars()
            .all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
            && !rest.starts_with(|c: char| c.is_ascii_digit())
    });
    let full = if typed.contains("://") || scheme {
        typed.to_string()
    } else {
        format!("https://{typed}")
    };
    let url = url::Url::parse(&full).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.host_str().is_some_and(|h| !h.is_empty()))
        .then(|| url.to_string())
}

/// A URL shown without its scheme or trailing slash.
#[must_use]
pub fn display_url(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string()
}

impl LoginParams {
    fn checked(&self, adding: bool) -> ModelResult<(Checked, String)> {
        let checked = Checked {
            title: visible_text(&self.title, false).trim().to_string(),
            username: visible_text(&self.username, false).trim().to_string(),
            url: self.url.trim().to_string(),
            notes: visible_text(&self.notes, true).trim().to_string(),
            password: self.password.expose_secret().chars().count(),
            recovery: self.recovery.expose_secret().chars().count(),
        };
        ValidatorTrait::validate(&checked)?;
        let url = clean_url(&checked.url).ok_or_else(|| {
            field_error(
                "url",
                "Use a web address starting with http:// or https://, or leave it empty.",
            )
        })?;
        if adding && self.password.expose_secret().is_empty() {
            return Err(field_error("password", "Enter the password."));
        }
        Ok((checked, url))
    }

    /// The named members, each once, after checking every one is an active
    /// member of the organisation.
    async fn access_ids<C: ConnectionTrait>(&self, db: &C, org_id: i64) -> ModelResult<Vec<i64>> {
        let mut ids = self.access.clone();
        ids.sort_unstable();
        ids.dedup();
        let found = memberships::Model::team_among(db, org_id, ids.iter().copied()).await?;
        if found.len() != ids.len() {
            return Err(field_error(
                "access",
                "Choose people who are members of your organisation.",
            ));
        }
        Ok(ids)
    }
}

/// Ciphertext and nonce for one field.
type Sealed = (Vec<u8>, Vec<u8>);

fn seal(
    dek: &Key,
    org_id: i64,
    login_id: i64,
    name: &str,
    dek_version: i32,
    secret: &SecretString,
) -> ModelResult<Sealed> {
    let aad = crypto::field_aad(org_id, login_id, name, dek_version);
    let sealed = crypto::seal(dek, secret.expose_secret().as_bytes(), &aad)?;
    Ok((sealed.ciphertext, sealed.nonce))
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

impl Model {
    /// The logins in a folder, newest first.
    ///
    /// # Errors
    /// On database errors.
    pub async fn for_folder<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        folder_id: i64,
    ) -> ModelResult<Vec<Self>> {
        Ok(Entity::find()
            .in_tenant(org_id)
            .filter(Column::FolderId.eq(folder_id))
            .order_by_desc(Column::CreatedAt)
            .order_by_desc(Column::Id)
            .all(db)
            .await?)
    }

    /// Every login in the organisation, by title.
    ///
    /// # Errors
    /// On database errors.
    pub async fn for_org<C: ConnectionTrait>(db: &C, org_id: i64) -> ModelResult<Vec<Self>> {
        Ok(Entity::find()
            .in_tenant(org_id)
            .order_by_asc(Column::Title)
            .all(db)
            .await?)
    }

    /// # Errors
    /// On database errors.
    pub async fn count_in_folder<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        folder_id: i64,
    ) -> ModelResult<u64> {
        Ok(Entity::find()
            .in_tenant(org_id)
            .filter(Column::FolderId.eq(folder_id))
            .count(db)
            .await?)
    }

    /// # Errors
    /// `EntityNotFound` when the login is not in this organisation.
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

    /// Adds a login to a folder, encrypting its secrets, and logs it. The id
    /// is taken first, because the encryption names it.
    ///
    /// # Errors
    /// Field errors for bad input, or database errors.
    pub async fn create(
        ctx: &AppContext,
        folder: &folders::Model,
        member: &CurrentMember,
        client: &Client,
        params: &LoginParams,
    ) -> ModelResult<Self> {
        let org_id = folder.organisation_id;
        let (checked, url) = params.checked(true)?;
        let kek = Kek::get(ctx)?;
        let txn = ctx.db.begin().await?;
        let access = params.access_ids(&txn, org_id).await?;
        let (dek, dek_version) = org_keys::Model::dek_for(&txn, &kek, org_id).await?;
        let id = Self::next_id(&txn).await?;
        let (password_ct, password_nonce) = seal(
            &dek,
            org_id,
            id,
            field::PASSWORD,
            dek_version,
            &params.password,
        )?;
        let recovery = if params.recovery.expose_secret().trim().is_empty() {
            None
        } else {
            Some(seal(
                &dek,
                org_id,
                id,
                field::RECOVERY,
                dek_version,
                &params.recovery,
            )?)
        };
        let (recovery_ct, recovery_nonce) = recovery.unzip();
        let login = ActiveModel {
            id: ActiveValue::Set(id),
            folder_id: ActiveValue::Set(folder.id),
            title: ActiveValue::Set(checked.title),
            username: ActiveValue::Set(checked.username),
            url: ActiveValue::Set(url),
            notes: ActiveValue::Set(checked.notes),
            password_ct: ActiveValue::Set(password_ct),
            password_nonce: ActiveValue::Set(password_nonce),
            recovery_ct: ActiveValue::Set(recovery_ct),
            recovery_nonce: ActiveValue::Set(recovery_nonce),
            dek_version: ActiveValue::Set(dek_version),
            created_by_id: ActiveValue::Set(Some(member.user.id)),
            ..Default::default()
        }
        .set_tenant(org_id)?
        .insert(&txn)
        .await?;
        login_accesses::Model::replace(&txn, login.id, &access).await?;
        login
            .log(&txn, member, client, action::CREATE, None)
            .await?;
        txn.commit().await?;
        Ok(login)
    }

    /// Changes a login. Secrets left empty stay as they are; new ones are
    /// encrypted under the organisation's data key.
    ///
    /// # Errors
    /// Field errors for bad input, `EntityNotFound` for a folder outside the
    /// organisation, or database errors.
    pub async fn edit(
        self,
        ctx: &AppContext,
        member: &CurrentMember,
        client: &Client,
        params: &LoginParams,
    ) -> ModelResult<Self> {
        let org_id = self.organisation_id;
        let (checked, url) = params.checked(false)?;
        let kek = Kek::get(ctx)?;
        let txn = ctx.db.begin().await?;
        let access = params.access_ids(&txn, org_id).await?;
        let folder_id = match params.folder_id {
            Some(id) if id != self.folder_id => {
                folders::Model::find_in_org(&txn, org_id, id).await?.id
            }
            _ => self.folder_id,
        };
        let new_password = !params.password.expose_secret().is_empty();
        let new_recovery = !params.recovery.expose_secret().trim().is_empty();
        let mut login = self.clone().into_active_model();
        if new_password || new_recovery {
            let (dek, dek_version) = org_keys::Model::dek_for(&txn, &kek, org_id).await?;
            if dek_version != self.dek_version {
                return Err(ModelError::msg("this login's key has changed"));
            }
            for (name, secret, wanted) in [
                (field::PASSWORD, &params.password, new_password),
                (field::RECOVERY, &params.recovery, new_recovery),
            ] {
                if !wanted {
                    continue;
                }
                let (ct, nonce) = seal(&dek, org_id, self.id, name, dek_version, secret)?;
                if name == field::PASSWORD {
                    login.password_ct = ActiveValue::Set(ct);
                    login.password_nonce = ActiveValue::Set(nonce);
                } else {
                    login.recovery_ct = ActiveValue::Set(Some(ct));
                    login.recovery_nonce = ActiveValue::Set(Some(nonce));
                }
            }
        }
        if params.remove_recovery.is_some() && !new_recovery {
            login.recovery_ct = ActiveValue::Set(None);
            login.recovery_nonce = ActiveValue::Set(None);
        }
        login.title = ActiveValue::Set(checked.title);
        login.username = ActiveValue::Set(checked.username);
        login.url = ActiveValue::Set(url);
        login.notes = ActiveValue::Set(checked.notes);
        login.folder_id = ActiveValue::Set(folder_id);
        let login = login.update(&txn).await?;
        login_accesses::Model::replace(&txn, login.id, &access).await?;
        let detail = new_password.then(|| field::PASSWORD.to_string());
        login
            .log(&txn, member, client, action::EDIT, detail)
            .await?;
        txn.commit().await?;
        Ok(login)
    }

    /// Deletes a login for everyone. The log keeps its title.
    ///
    /// # Errors
    /// On database errors.
    pub async fn delete_login(
        self,
        db: &DatabaseConnection,
        member: &CurrentMember,
        client: &Client,
    ) -> ModelResult<()> {
        let txn = db.begin().await?;
        self.log(&txn, member, client, action::DELETE, None).await?;
        self.into_active_model().delete(&txn).await?;
        txn.commit().await?;
        Ok(())
    }

    /// Writes an event about this login.
    ///
    /// # Errors
    /// On database errors.
    pub async fn log<C: ConnectionTrait>(
        &self,
        db: &C,
        member: &CurrentMember,
        client: &Client,
        action: &'static str,
        detail: Option<String>,
    ) -> ModelResult<login_events::Model> {
        login_events::Model::record(
            db,
            client,
            Entry {
                org_id: self.organisation_id,
                login: Some((self.id, &self.title)),
                user: (member.user.id, &member.username),
                action,
                detail,
            },
        )
        .await
    }

    async fn next_id<C: ConnectionTrait>(db: &C) -> ModelResult<i64> {
        let row = db
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT nextval(pg_get_serial_sequence('logins', 'id')) AS id",
            ))
            .await?
            .ok_or_else(|| ModelError::msg("no id for a new login"))?;
        Ok(row.try_get::<i64>("", "id")?)
    }

    /// Whether this person may see the secrets: through their own browser
    /// session (not a bearer token, not a super admin acting inside the
    /// organisation), as an owner, an admin, or a member named on it.
    ///
    /// # Errors
    /// On database errors.
    pub async fn can_reveal<C: ConnectionTrait>(
        &self,
        db: &C,
        member: &CurrentMember,
    ) -> ModelResult<bool> {
        if !member.via_session || member.acting || member.org.id != self.organisation_id {
            return Ok(false);
        }
        if member.can_manage() {
            return Ok(true);
        }
        login_accesses::Model::is_named(db, self.id, member.user.id).await
    }

    /// Decrypts one secret. Callers check [`Self::can_reveal`] and the
    /// unlock, and write the log entry, first.
    ///
    /// # Errors
    /// When there is nothing saved, or it does not decrypt (wrong key, or a
    /// value moved from another login, field or key version).
    pub async fn reveal<C: ConnectionTrait>(
        &self,
        db: &C,
        kek: &Kek,
        name: &str,
    ) -> ModelResult<SecretString> {
        let (ct, nonce) = match name {
            field::PASSWORD => (Some(&self.password_ct), Some(&self.password_nonce)),
            field::RECOVERY => (self.recovery_ct.as_ref(), self.recovery_nonce.as_ref()),
            _ => (None, None),
        };
        let (Some(ct), Some(nonce)) = (ct, nonce) else {
            return Err(ModelError::EntityNotFound);
        };
        let (dek, _) = org_keys::Model::dek_for(db, kek, self.organisation_id).await?;
        let aad = crypto::field_aad(self.organisation_id, self.id, name, self.dek_version);
        crypto::open(&dek, ct, nonce, &aad)
    }

    #[must_use]
    pub fn has_recovery(&self) -> bool {
        self.recovery_ct.is_some()
    }

    /// Whether `user_id`, with `member_role`, can see any login in the
    /// organisation.
    ///
    /// # Errors
    /// On database errors.
    pub async fn any_visible_to<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
        member_role: &str,
    ) -> ModelResult<bool> {
        let ids: Vec<i64> = Entity::find()
            .in_tenant(org_id)
            .all(db)
            .await?
            .into_iter()
            .map(|l| l.id)
            .collect();
        if ids.is_empty() {
            return Ok(false);
        }
        if member_role == role::OWNER || member_role == role::ADMIN {
            return Ok(true);
        }
        Ok(!login_accesses::Model::named_among(db, user_id, ids)
            .await?
            .is_empty())
    }
}

impl loco_rs::prelude::TenantEntity for Entity {
    type TenantId = i64;

    fn tenant_column() -> Column {
        Column::OrganisationId
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websites_are_http_or_https_only() {
        assert_eq!(clean_url("").as_deref(), Some(""));
        assert_eq!(
            clean_url(" example.com/login ").as_deref(),
            Some("https://example.com/login")
        );
        assert_eq!(
            clean_url("http://mail.example.com").as_deref(),
            Some("http://mail.example.com/")
        );
        assert_eq!(
            clean_url("localhost:8080").as_deref(),
            Some("https://localhost:8080/")
        );
        for bad in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            " javascript:alert(document.cookie)",
            "data:text/html,<script>alert(1)</script>",
            "mailto:office@example.com",
            "ftp://example.com",
            "vbscript:msgbox(1)",
            "https://",
            "not a website",
        ] {
            assert_eq!(clean_url(bad), None, "{bad}");
        }
        assert_eq!(
            display_url("https://find-and-update.company-information.service.gov.uk/"),
            "find-and-update.company-information.service.gov.uk"
        );
    }
}
