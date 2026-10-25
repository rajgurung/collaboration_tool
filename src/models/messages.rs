use std::{collections::HashMap, path::Path};

use bytes::Bytes;
use loco_rs::prelude::*;
use sea_orm::FromQueryResult;
use serde::Deserialize;

pub use super::_entities::messages::{ActiveModel, Column, Entity, Model};
use super::{
    conversation_members::ReadSpan,
    conversations::{self, kind},
    documents, folders,
};

pub type Messages = Entity;

/// How many messages a conversation (or a thread) shows when it is opened.
pub const HISTORY_LIMIT: u64 = 200;

/// One person's replies in one thread: how many, and when the first and the
/// last were sent.
#[derive(Debug, Clone, PartialEq, Eq, FromQueryResult)]
pub struct ReplierStat {
    pub parent_id: i64,
    pub user_id: i64,
    pub replies: i64,
    pub first_at: DateTimeWithTimeZone,
    pub last_at: DateTimeWithTimeZone,
}

#[derive(Debug, Deserialize, Validate)]
pub struct MessageParams {
    #[validate(length(min = 1, max = 1000, message = "Messages are 1 to 1,000 characters."))]
    pub body: String,
}

/// A photo's caption: optional, so only a maximum.
#[derive(Debug, Validate)]
struct CaptionParams {
    #[validate(length(max = 1000, message = "Captions are up to 1,000 characters."))]
    body: String,
}

/// Photo types a chat shows in place. HEIC is left out: most browsers can't
/// draw it, and phones send JPEG when a page asks for `image/*`. The
/// `messages-own-photo` constraint holds the same list.
pub const PHOTO_TYPES: [&str; 3] = ["image/jpeg", "image/png", "image/webp"];
pub const NOT_A_PHOTO: &str = "Only photos can be posted here. Use a JPG, PNG or WebP.";
pub const PHOTOS_NOT_HERE: &str = "Photos can't be posted here.";

/// Where a message's photo is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotoFile<'a> {
    /// A group or DM photo, stored on the message itself.
    Own {
        key: &'a str,
        content_type: &'a str,
        size: i64,
        name: &'a str,
    },
    /// A #general photo: a file in the "Chat photos" folder.
    Document(i64),
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
    /// The latest messages in a conversation, oldest first. Thread replies
    /// are not part of the main feed.
    ///
    /// # Errors
    /// On database errors.
    pub async fn recent<C: ConnectionTrait>(
        db: &C,
        conversation: &conversations::Model,
    ) -> ModelResult<Vec<Self>> {
        let mut latest = Entity::find()
            .in_tenant(conversation.organisation_id)
            .filter(Column::ConversationId.eq(conversation.id))
            .filter(Column::ParentId.is_null())
            .order_by_desc(Column::CreatedAt)
            .order_by_desc(Column::Id)
            .limit(HISTORY_LIMIT)
            .all(db)
            .await?;
        latest.reverse();
        Ok(latest)
    }

    /// Messages from other people that a read just covered, newest first and
    /// capped at what a conversation shows. Thread replies carry no receipts.
    ///
    /// # Errors
    /// On database errors.
    pub async fn read_in_span<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        conversation_id: i64,
        reader_id: i64,
        span: &ReadSpan,
    ) -> ModelResult<Vec<Self>> {
        let mut query = Entity::find()
            .in_tenant(org_id)
            .filter(Column::ConversationId.eq(conversation_id))
            .filter(Column::UserId.ne(reader_id))
            .filter(Column::ParentId.is_null())
            .filter(Column::CreatedAt.lte(span.to));
        if let Some(from) = span.from {
            query = query.filter(Column::CreatedAt.gt(from));
        }
        Ok(query
            .order_by_desc(Column::CreatedAt)
            .order_by_desc(Column::Id)
            .limit(HISTORY_LIMIT)
            .all(db)
            .await?)
    }

    /// The newest top-level message in each of the given conversations, for
    /// the chat list. A thread reply never shows there.
    ///
    /// # Errors
    /// On database errors.
    pub async fn latest_per_conversation<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        conversation_ids: &[i64],
    ) -> ModelResult<HashMap<i64, Self>> {
        let mut latest = HashMap::new();
        for message in Entity::find()
            .in_tenant(org_id)
            .filter(Column::ConversationId.is_in(conversation_ids.iter().copied()))
            .filter(Column::ParentId.is_null())
            .order_by_desc(Column::CreatedAt)
            .order_by_desc(Column::Id)
            .all(db)
            .await?
        {
            latest.entry(message.conversation_id).or_insert(message);
        }
        Ok(latest)
    }

    /// A message in this conversation and its organisation. Every route that
    /// takes a message id goes through this, after checking membership.
    ///
    /// # Errors
    /// `EntityNotFound` for an id in another conversation or organisation.
    pub async fn find_in_conversation<C: ConnectionTrait>(
        db: &C,
        conversation: &conversations::Model,
        id: i64,
    ) -> ModelResult<Self> {
        Entity::find()
            .in_tenant(conversation.organisation_id)
            .filter(Column::Id.eq(id))
            .filter(Column::ConversationId.eq(conversation.id))
            .one(db)
            .await?
            .ok_or(ModelError::EntityNotFound)
    }

    /// A top-level message in this conversation and its organisation, which
    /// a thread can hang from. A reply's id is not a thread.
    ///
    /// # Errors
    /// `EntityNotFound` for a reply, or an id in another conversation or
    /// organisation.
    pub async fn find_thread_parent<C: ConnectionTrait>(
        db: &C,
        conversation: &conversations::Model,
        id: i64,
    ) -> ModelResult<Self> {
        Entity::find()
            .in_tenant(conversation.organisation_id)
            .filter(Column::Id.eq(id))
            .filter(Column::ConversationId.eq(conversation.id))
            .filter(Column::ParentId.is_null())
            .one(db)
            .await?
            .ok_or(ModelError::EntityNotFound)
    }

    /// Saves a message. The caller must already know the sender is a member.
    ///
    /// # Errors
    /// Validation errors, or database errors.
    pub async fn create<C: ConnectionTrait>(
        db: &C,
        conversation: &conversations::Model,
        user_id: i64,
        params: &MessageParams,
    ) -> ModelResult<Self> {
        Self::insert(db, conversation, None, user_id, params).await
    }

    /// Saves a reply in `parent`'s thread. Threads are one level deep, and the
    /// parent must be in this conversation and organisation. The caller must
    /// already know the sender is a member.
    ///
    /// # Errors
    /// `EntityNotFound` for a parent that is a reply or belongs elsewhere,
    /// validation errors, or database errors.
    pub async fn create_reply<C: ConnectionTrait>(
        db: &C,
        conversation: &conversations::Model,
        parent: &Self,
        user_id: i64,
        params: &MessageParams,
    ) -> ModelResult<Self> {
        if parent.conversation_id != conversation.id
            || parent.organisation_id != conversation.organisation_id
            || parent.parent_id.is_some()
        {
            return Err(ModelError::EntityNotFound);
        }
        Self::insert(db, conversation, Some(parent.id), user_id, params).await
    }

    /// Saves a photo message, with an optional caption. The caller must
    /// already know the sender is a member.
    ///
    /// Where the file goes depends on the conversation. A #general photo goes
    /// into the "Chat photos" folder in Documents, which every member can
    /// open, as everyone is in #general. A group or DM photo is private to
    /// that conversation, so it is stored on the message itself under
    /// `orgs/{org}/chat/{conversation}/` and never appears in Documents.
    ///
    /// # Errors
    /// `Message` for a file that is empty, too big or not a photo;
    /// validation errors for a long caption; storage or database errors
    /// otherwise.
    pub async fn create_photo(
        ctx: &AppContext,
        conversation: &conversations::Model,
        user_id: i64,
        caption: &str,
        file_name: &str,
        bytes: Bytes,
    ) -> ModelResult<Self> {
        let body = caption.trim().to_string();
        ValidatorTrait::validate(&CaptionParams { body: body.clone() })?;
        if bytes.is_empty() {
            return Err(ModelError::msg("That file is empty."));
        }
        if bytes.len() > documents::MAX_BYTES {
            return Err(ModelError::msg(&documents::too_big()));
        }
        let content_type = documents::sniff(&bytes, file_name)
            .filter(|t| PHOTO_TYPES.contains(t))
            .ok_or_else(|| ModelError::msg(NOT_A_PHOTO))?;
        match conversation.kind.as_str() {
            kind::CHANNEL => {
                Self::create_documents_photo(ctx, conversation, user_id, body, file_name, bytes)
                    .await
            }
            kind::GROUP | kind::DM => {
                Self::create_own_photo(
                    ctx,
                    conversation,
                    user_id,
                    body,
                    file_name,
                    content_type,
                    bytes,
                )
                .await
            }
            _ => Err(ModelError::msg(PHOTOS_NOT_HERE)),
        }
    }

    /// A #general photo: the file goes into "Chat photos", then the message
    /// points at it.
    async fn create_documents_photo(
        ctx: &AppContext,
        conversation: &conversations::Model,
        user_id: i64,
        body: String,
        file_name: &str,
        bytes: Bytes,
    ) -> ModelResult<Self> {
        let folder = folders::Model::chat_photos(&ctx.db, conversation.organisation_id).await?;
        let file = documents::Model::upload(ctx, &folder, user_id, file_name, bytes).await?;
        let saved = ActiveModel {
            body: ActiveValue::Set(body),
            conversation_id: ActiveValue::Set(conversation.id),
            user_id: ActiveValue::Set(user_id),
            document_id: ActiveValue::Set(Some(file.id)),
            has_photo: ActiveValue::Set(true),
            ..Default::default()
        }
        .set_tenant(conversation.organisation_id)?
        .insert(&ctx.db)
        .await;
        match saved {
            Ok(message) => Ok(message),
            Err(err) => {
                if let Err(cleanup) = file.delete_file(ctx).await {
                    tracing::error!(error = %cleanup, "could not delete an unposted chat photo");
                }
                Err(err.into())
            }
        }
    }

    /// A group or DM photo: one write to storage, then the message, which
    /// holds the key, the sniffed type, the size and a cleaned name. If the
    /// message can't be saved, the stored bytes are deleted again.
    async fn create_own_photo(
        ctx: &AppContext,
        conversation: &conversations::Model,
        user_id: i64,
        body: String,
        file_name: &str,
        content_type: &str,
        bytes: Bytes,
    ) -> ModelResult<Self> {
        let key = documents::new_key(
            conversation.organisation_id,
            &format!("chat/{}", conversation.id),
        );
        let message = ActiveModel {
            body: ActiveValue::Set(body),
            conversation_id: ActiveValue::Set(conversation.id),
            user_id: ActiveValue::Set(user_id),
            has_photo: ActiveValue::Set(true),
            photo_key: ActiveValue::Set(Some(key.clone())),
            photo_type: ActiveValue::Set(Some(content_type.to_string())),
            photo_size: ActiveValue::Set(Some(i64::try_from(bytes.len()).unwrap_or(i64::MAX))),
            photo_name: ActiveValue::Set(Some(documents::stored_name(file_name, content_type))),
            ..Default::default()
        }
        .set_tenant(conversation.organisation_id)?;
        ctx.storage
            .upload(Path::new(&key), &bytes)
            .await
            .map_err(|err| ModelError::Any(Box::new(err)))?;
        match message.insert(&ctx.db).await {
            Ok(message) => Ok(message),
            Err(err) => {
                if let Err(cleanup) = ctx.storage.delete(Path::new(&key)).await {
                    tracing::error!(error = %cleanup, key, "could not delete an unposted chat photo");
                }
                Err(err.into())
            }
        }
    }

    /// Where this message's photo is stored, if it still has one.
    #[must_use]
    pub fn photo_file(&self) -> Option<PhotoFile<'_>> {
        if !self.has_photo {
            return None;
        }
        if let (Some(key), Some(content_type), Some(size), Some(name)) = (
            self.photo_key.as_deref(),
            self.photo_type.as_deref(),
            self.photo_size,
            self.photo_name.as_deref(),
        ) {
            return Some(PhotoFile::Own {
                key,
                content_type,
                size,
                name,
            });
        }
        self.document_id.map(PhotoFile::Document)
    }

    /// The message had a photo that is gone: a #general photo deleted from
    /// Documents.
    #[must_use]
    pub fn photo_removed(&self) -> bool {
        self.has_photo && self.photo_file().is_none()
    }

    async fn insert<C: ConnectionTrait>(
        db: &C,
        conversation: &conversations::Model,
        parent_id: Option<i64>,
        user_id: i64,
        params: &MessageParams,
    ) -> ModelResult<Self> {
        let body = params.body.trim().to_string();
        ValidatorTrait::validate(&MessageParams { body: body.clone() })?;
        Ok(ActiveModel {
            body: ActiveValue::Set(body),
            conversation_id: ActiveValue::Set(conversation.id),
            user_id: ActiveValue::Set(user_id),
            parent_id: ActiveValue::Set(parent_id),
            ..Default::default()
        }
        .set_tenant(conversation.organisation_id)?
        .insert(db)
        .await?)
    }

    /// The latest replies in a thread, oldest first.
    ///
    /// # Errors
    /// On database errors.
    pub async fn replies<C: ConnectionTrait>(db: &C, parent: &Self) -> ModelResult<Vec<Self>> {
        let mut latest = Entity::find()
            .in_tenant(parent.organisation_id)
            .filter(Column::ParentId.eq(parent.id))
            .order_by_desc(Column::CreatedAt)
            .order_by_desc(Column::Id)
            .limit(HISTORY_LIMIT)
            .all(db)
            .await?;
        latest.reverse();
        Ok(latest)
    }

    /// Who replied in each of these threads, in the order they first replied.
    /// One grouped query, so a feed never asks per message. Threads with no
    /// replies are missing from the map.
    ///
    /// # Errors
    /// On database errors.
    pub async fn thread_stats<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        parent_ids: &[i64],
    ) -> ModelResult<HashMap<i64, Vec<ReplierStat>>> {
        let mut stats: HashMap<i64, Vec<ReplierStat>> = HashMap::new();
        if parent_ids.is_empty() {
            return Ok(stats);
        }
        let rows = Entity::find()
            .in_tenant(org_id)
            .filter(Column::ParentId.is_in(parent_ids.iter().copied()))
            .select_only()
            .column(Column::ParentId)
            .column(Column::UserId)
            .column_as(Column::Id.count(), "replies")
            .column_as(Column::CreatedAt.min(), "first_at")
            .column_as(Column::CreatedAt.max(), "last_at")
            .group_by(Column::ParentId)
            .group_by(Column::UserId)
            .into_model::<ReplierStat>()
            .all(db)
            .await?;
        for row in rows {
            stats.entry(row.parent_id).or_default().push(row);
        }
        for list in stats.values_mut() {
            list.sort_by_key(|s| (s.first_at, s.user_id));
        }
        Ok(stats)
    }
}

impl loco_rs::prelude::TenantEntity for Entity {
    type TenantId = i64;

    fn tenant_column() -> super::_entities::messages::Column {
        super::_entities::messages::Column::OrganisationId
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message() -> Model {
        Model {
            created_at: chrono::Utc::now().into(),
            updated_at: chrono::Utc::now().into(),
            id: 1,
            body: "Van fuel".to_string(),
            organisation_id: 1,
            conversation_id: 1,
            user_id: 1,
            parent_id: None,
            document_id: None,
            has_photo: false,
            photo_key: None,
            photo_type: None,
            photo_size: None,
            photo_name: None,
        }
    }

    #[test]
    fn photos_are_found_where_they_are_stored() {
        let text = message();
        assert_eq!(text.photo_file(), None);
        assert!(!text.photo_removed());

        let own = Model {
            has_photo: true,
            photo_key: Some("orgs/1/chat/1/k".to_string()),
            photo_type: Some("image/png".to_string()),
            photo_size: Some(12),
            photo_name: Some("Receipt.png".to_string()),
            ..message()
        };
        assert_eq!(
            own.photo_file(),
            Some(PhotoFile::Own {
                key: "orgs/1/chat/1/k",
                content_type: "image/png",
                size: 12,
                name: "Receipt.png",
            })
        );
        assert!(!own.photo_removed());

        let in_documents = Model {
            has_photo: true,
            document_id: Some(9),
            ..message()
        };
        assert_eq!(in_documents.photo_file(), Some(PhotoFile::Document(9)));
        assert!(!in_documents.photo_removed());

        let removed = Model {
            has_photo: true,
            ..message()
        };
        assert_eq!(removed.photo_file(), None);
        assert!(removed.photo_removed());
    }
}
