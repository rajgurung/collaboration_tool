//! Uploaded files. The bytes live in `ctx.storage` under a random key; this
//! table holds the name, type and size. A file's type comes from its own
//! bytes, never from its name, and only photos, PDFs and Office files are
//! accepted.
use std::path::Path;

use bytes::Bytes;
use loco_rs::prelude::*;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};

pub use super::_entities::documents::{ActiveModel, Column, Entity, Model};
use super::folders;

pub type Documents = Entity;

/// The largest file anyone can add: 25 MB.
pub const MAX_BYTES: usize = 25 * 1024 * 1024;
/// Longest file name, extension included.
pub const NAME_MAX: usize = 200;
pub const WRONG_TYPE: &str = "This kind of file can't be added. Use a photo, PDF or Office file.";

/// Every type that can be stored: MIME type, extension, label on its tile.
const TYPES: [(&str, &str, &str); 12] = [
    ("image/jpeg", "jpg", "JPG"),
    ("image/png", "png", "PNG"),
    ("image/webp", "webp", "WEBP"),
    ("image/heic", "heic", "HEIC"),
    ("image/heif", "heif", "HEIF"),
    ("application/pdf", "pdf", "PDF"),
    (
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "docx",
        "DOCX",
    ),
    (
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xlsx",
        "XLSX",
    ),
    (
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "pptx",
        "PPTX",
    ),
    ("application/msword", "doc", "DOC"),
    ("application/vnd.ms-excel", "xls", "XLS"),
    ("application/vnd.ms-powerpoint", "ppt", "PPT"),
];

/// Types the browser shows in place. Everything else downloads.
const INLINE: [&str; 4] = ["image/jpeg", "image/png", "image/webp", "application/pdf"];

/// Invisible characters that can make a name read differently from what it
/// is, such as a right-to-left override that shows "photo[RLO]gpj.exe" as
/// "photoexe.jpg".
const BIDI: [char; 12] = [
    '\u{061C}', '\u{200E}', '\u{200F}', '\u{202A}', '\u{202B}', '\u{202C}', '\u{202D}', '\u{202E}',
    '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
];

fn by_mime(mime: &str) -> Option<&'static (&'static str, &'static str, &'static str)> {
    TYPES.iter().find(|(m, _, _)| *m == mime)
}

fn by_ext(ext: &str) -> Option<&'static (&'static str, &'static str, &'static str)> {
    TYPES.iter().find(|(_, e, _)| *e == ext)
}

fn extension(file_name: &str) -> String {
    file_name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_lowercase())
        .unwrap_or_default()
}

/// The stored type of an upload, from its first bytes. Office files are
/// ZIP or OLE containers underneath, so those are told apart by extension.
/// `None` for anything else, including SVG, HTML, XML and text.
#[must_use]
pub fn sniff(bytes: &[u8], file_name: &str) -> Option<&'static str> {
    let ext = extension(file_name);
    let found = match infer::get(bytes)?.mime_type() {
        mime @ ("image/jpeg" | "image/png" | "image/webp" | "application/pdf") => by_mime(mime),
        "image/heif" => by_ext(if ext == "heif" { "heif" } else { "heic" }),
        "application/zip"
        | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        | "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
            matches!(ext.as_str(), "docx" | "xlsx" | "pptx").then(|| by_ext(&ext))?
        }
        "application/x-ole-storage"
        | "application/msword"
        | "application/vnd.ms-excel"
        | "application/vnd.ms-powerpoint" => {
            matches!(ext.as_str(), "doc" | "xls" | "ppt").then(|| by_ext(&ext))?
        }
        _ => None,
    };
    found.map(|(mime, _, _)| *mime)
}

/// Text safe to show as a file name: no control or direction characters,
/// no slashes, trimmed.
fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() && !BIDI.contains(c))
        .map(|c| if c == '/' || c == '\\' { '-' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

/// `stem.ext`, cut to [`NAME_MAX`] characters. An empty stem becomes "File".
fn full_name(stem: &str, ext: &str) -> String {
    let stem: String = clean(stem).chars().take(NAME_MAX - ext.len() - 1).collect();
    let stem = stem.trim();
    format!("{}.{ext}", if stem.is_empty() { "File" } else { stem })
}

/// "1.8 MB", "214 KB", "900 bytes".
#[must_use]
pub fn human_size(bytes: i64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let size = bytes as f64;
    if size >= 1024.0 * 1024.0 {
        format!("{:.1} MB", size / (1024.0 * 1024.0))
    } else if size >= 1024.0 {
        format!("{:.0} KB", size / 1024.0)
    } else {
        format!("{bytes} bytes")
    }
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

fn storage_error(err: loco_rs::storage::StorageError) -> ModelError {
    ModelError::Any(Box::new(err))
}

impl Model {
    /// The files in a folder, newest first.
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
    /// `EntityNotFound` when the file is not in this organisation.
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

    /// Stores a file in a folder: one write to storage, then the row. If the
    /// row cannot be saved, the stored bytes are deleted again.
    ///
    /// # Errors
    /// `Message` for a file that is empty, too big or of a type that is not
    /// allowed; storage or database errors otherwise.
    pub async fn upload(
        ctx: &AppContext,
        folder: &folders::Model,
        uploaded_by_id: i64,
        file_name: &str,
        bytes: Bytes,
    ) -> ModelResult<Self> {
        if bytes.is_empty() {
            return Err(ModelError::msg("That file is empty."));
        }
        if bytes.len() > MAX_BYTES {
            return Err(ModelError::msg(&too_big()));
        }
        let content_type = sniff(&bytes, file_name).ok_or_else(|| ModelError::msg(WRONG_TYPE))?;
        let ext = by_mime(content_type).map_or("", |(_, ext, _)| ext);
        let stem = file_name
            .rsplit_once('.')
            .map_or(file_name, |(stem, _)| stem);
        let key = format!(
            "orgs/{}/docs/{}",
            folder.organisation_id,
            uuid::Uuid::new_v4()
        );
        ctx.storage
            .upload(Path::new(&key), &bytes)
            .await
            .map_err(storage_error)?;
        let saved = ActiveModel {
            name: ActiveValue::Set(full_name(stem, ext)),
            content_type: ActiveValue::Set(content_type.to_string()),
            size_bytes: ActiveValue::Set(i64::try_from(bytes.len()).unwrap_or(i64::MAX)),
            storage_key: ActiveValue::Set(key.clone()),
            folder_id: ActiveValue::Set(folder.id),
            uploaded_by_id: ActiveValue::Set(Some(uploaded_by_id)),
            ..Default::default()
        }
        .set_tenant(folder.organisation_id)?
        .insert(&ctx.db)
        .await;
        match saved {
            Ok(file) => Ok(file),
            Err(err) => {
                if let Err(cleanup) = ctx.storage.delete(Path::new(&key)).await {
                    tracing::error!(error = %cleanup, key, "could not delete an orphaned upload");
                }
                Err(err.into())
            }
        }
    }

    /// Renames the file. Only the part before the extension changes; the
    /// extension always matches the stored type.
    ///
    /// # Errors
    /// `Message` for an empty name, or database errors.
    pub async fn rename<C: ConnectionTrait>(self, db: &C, name: &str) -> ModelResult<Self> {
        let ext = self.ext();
        let typed = clean(name);
        let stem = typed
            .strip_suffix(&format!(".{ext}"))
            .unwrap_or(&typed)
            .trim()
            .to_string();
        if stem.is_empty() {
            return Err(ModelError::msg("Give the file a name."));
        }
        let mut file = self.into_active_model();
        file.name = ActiveValue::Set(full_name(&stem, ext));
        Ok(file.update(db).await?)
    }

    /// Deletes the stored bytes, then the row. There is no bin.
    ///
    /// # Errors
    /// Storage or database errors. The row stays if the bytes could not be
    /// deleted, so the delete can be tried again.
    pub async fn delete_file(self, ctx: &AppContext) -> ModelResult<()> {
        ctx.storage
            .delete(Path::new(&self.storage_key))
            .await
            .map_err(storage_error)?;
        self.into_active_model().delete(&ctx.db).await?;
        Ok(())
    }

    /// The uploader, owners and admins can delete a file.
    #[must_use]
    pub fn can_delete(&self, user_id: i64, can_manage: bool) -> bool {
        can_manage || self.uploaded_by_id == Some(user_id)
    }

    #[must_use]
    pub fn ext(&self) -> &'static str {
        by_mime(&self.content_type).map_or("", |(_, ext, _)| ext)
    }

    /// The short type shown on the file's tile, such as "PDF".
    #[must_use]
    pub fn label(&self) -> &'static str {
        by_mime(&self.content_type).map_or("FILE", |(_, _, label)| label)
    }

    #[must_use]
    pub fn is_image(&self) -> bool {
        self.content_type.starts_with("image/")
    }

    /// The name without its extension, for the rename field.
    #[must_use]
    pub fn stem(&self) -> &str {
        self.name
            .strip_suffix(&format!(".{}", self.ext()))
            .unwrap_or(&self.name)
    }

    /// Whether the browser may show it in place rather than download it.
    #[must_use]
    pub fn opens_inline(&self) -> bool {
        INLINE.contains(&self.content_type.as_str())
    }

    /// A `Content-Disposition` value (RFC 6266): a plain ASCII name for old
    /// browsers and the real name, percent-encoded, for the rest.
    #[must_use]
    pub fn disposition(&self, download: bool) -> String {
        let kind = if download || !self.opens_inline() {
            "attachment"
        } else {
            "inline"
        };
        let ascii: String = self
            .name
            .chars()
            .map(|c| {
                if c.is_ascii() && !c.is_ascii_control() && c != '"' && c != '\\' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let encoded = utf8_percent_encode(&self.name, NON_ALPHANUMERIC);
        format!(r#"{kind}; filename="{ascii}"; filename*=UTF-8''{encoded}"#)
    }
}

/// The refusal for a file over [`MAX_BYTES`].
#[must_use]
pub fn too_big() -> String {
    "Too big. The limit is 25 MB. Try saving a smaller copy.".to_string()
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

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    const ZIP: &[u8] = b"PK\x03\x04\x14\0\0\0\0\0";

    #[test]
    fn types_come_from_the_bytes() {
        assert_eq!(sniff(PNG, "photo.png"), Some("image/png"));
        assert_eq!(
            sniff(PNG, "photo.pdf"),
            Some("image/png"),
            "the name is ignored"
        );
        assert_eq!(sniff(b"%PDF-1.7\n", "a.pdf"), Some("application/pdf"));
        assert_eq!(
            sniff(ZIP, "Lease.DOCX"),
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
        );
        assert_eq!(sniff(ZIP, "archive.zip"), None, "a plain zip is refused");
        assert_eq!(sniff(b"MZ\x90\0\x03\0\0\0\x04\0\0\0", "setup.pdf"), None);
        assert_eq!(
            sniff(
                br#"<svg xmlns="http://www.w3.org/2000/svg"></svg>"#,
                "a.svg"
            ),
            None
        );
        assert_eq!(sniff(b"<!DOCTYPE html><html></html>", "a.html"), None);
        assert_eq!(sniff(b"just some text", "notes.txt"), None);
    }

    #[test]
    fn names_are_cleaned_and_keep_their_type() {
        assert_eq!(full_name("photo\u{202E}gpj", "exe"), "photogpj.exe");
        assert_eq!(full_name(" a/b\\c\n ", "pdf"), "a-b-c.pdf");
        assert_eq!(full_name("", "jpg"), "File.jpg");
        assert_eq!(full_name(&"x".repeat(300), "pdf").chars().count(), NAME_MAX);
    }

    #[test]
    fn content_disposition_follows_rfc_6266() {
        let file = Model {
            created_at: chrono::Utc::now().into(),
            updated_at: chrono::Utc::now().into(),
            id: 1,
            name: "Café \"menu\".pdf".to_string(),
            content_type: "application/pdf".to_string(),
            size_bytes: 1,
            storage_key: "k".to_string(),
            organisation_id: 1,
            folder_id: 1,
            uploaded_by_id: None,
        };
        assert_eq!(
            file.disposition(false),
            r#"inline; filename="Caf_ _menu_.pdf"; filename*=UTF-8''Caf%C3%A9%20%22menu%22%2Epdf"#
        );
        assert!(file.disposition(true).starts_with("attachment;"));
        assert_eq!(human_size(1_887_436), "1.8 MB");
        assert_eq!(human_size(219_136), "214 KB");
    }
}
