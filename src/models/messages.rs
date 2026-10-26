use std::{collections::HashMap, path::Path};

use bytes::Bytes;
use loco_rs::prelude::*;
use sea_orm::FromQueryResult;
use serde::Deserialize;

pub use super::_entities::messages::{ActiveModel, Column, Entity, Model};
use super::{
    conversation_members::ReadSpan,
    conversations::{self, kind},
    documents, folders, notifications,
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

/// The sender, owners and admins may delete a message, or its photo, as
/// with a Documents file. The caller must already know the person is in the
/// conversation. A message with replies has one more rule:
/// [`may_delete_message`].
#[must_use]
pub fn may_delete(sender_id: i64, user_id: i64, can_manage: bool) -> bool {
    can_manage || sender_id == user_id
}

/// Shown, and returned with a 403, when the sender tries to delete a
/// message that has replies.
pub const HAS_REPLIES: &str = "This has replies. Ask an owner or admin to delete it.";

/// Never expected: replies can't carry photos. A delete refuses rather than
/// let the cascade leave a reply's stored photo behind.
pub const REPLY_HAS_PHOTO: &str = "A reply in this thread has a photo. Delete that reply first.";

/// Whether this person may delete a message with `replies` replies: owners
/// and admins any, the sender only one with none. The caller must already
/// know the person is in the conversation.
#[must_use]
pub fn may_delete_message(sender_id: i64, user_id: i64, can_manage: bool, replies: u64) -> bool {
    can_manage || (sender_id == user_id && replies == 0)
}

/// What [`Model::delete_message`] removed besides the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deleted {
    /// Replies that went with a thread parent.
    pub replies: u64,
    /// Who had bell notices about the message or its replies.
    pub notified: Vec<i64>,
}

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

    /// This message, locked with `FOR SHARE` until the caller's transaction
    /// ends, or `None` once it is deleted. Reactions, replies and chat notices
    /// are saved under it. A delete holds `FOR UPDATE`, so whichever comes
    /// second sees the other's result.
    ///
    /// # Errors
    /// On database errors.
    pub async fn lock_live<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        id: i64,
    ) -> ModelResult<Option<Self>> {
        Ok(Entity::find()
            .in_tenant(org_id)
            .filter(Column::Id.eq(id))
            .lock_shared()
            .one(db)
            .await?)
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
    /// The reply is saved under a lock on the parent ([`Self::lock_live`]), so
    /// a delete of the parent either counts it or happens first, and then
    /// this is refused.
    ///
    /// # Errors
    /// `EntityNotFound` for a parent that is a reply, belongs elsewhere or
    /// was deleted; validation errors, or database errors.
    pub async fn create_reply(
        db: &DatabaseConnection,
        conversation: &conversations::Model,
        parent: &Self,
        user_id: i64,
        params: &MessageParams,
    ) -> ModelResult<Self> {
        let txn = db.begin().await?;
        let live = Self::lock_live(&txn, conversation.organisation_id, parent.id).await?;
        let Some(parent) = live.filter(|p| {
            p.conversation_id == conversation.id
                && p.organisation_id == conversation.organisation_id
                && p.parent_id.is_none()
        }) else {
            // Dropping `txn` rolls it back.
            return Err(ModelError::EntityNotFound);
        };
        let reply = Self::insert(&txn, conversation, Some(parent.id), user_id, params).await?;
        txn.commit().await?;
        Ok(reply)
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

    /// The message had a photo that is gone: deleted from the chat, or a
    /// #general photo deleted from Documents.
    #[must_use]
    pub fn photo_removed(&self) -> bool {
        self.has_photo && self.photo_file().is_none()
    }

    /// The #general message whose photo is this Documents file, if any.
    ///
    /// # Errors
    /// On database errors.
    pub async fn for_document<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        file_id: i64,
    ) -> ModelResult<Option<Self>> {
        Ok(Entity::find()
            .in_tenant(org_id)
            .filter(Column::DocumentId.eq(file_id))
            .one(db)
            .await?)
    }

    /// Deletes this message's photo for everyone. Returns the message as it
    /// now is, and whether this call removed anything. The message, caption,
    /// reactions and thread stay; `has_photo` stays true, so it shows "Photo
    /// removed".
    ///
    /// The stored bytes go first, then the row: if storage fails nothing has
    /// changed and the delete can be tried again. A photo that is already
    /// gone is not an error. The caller checks membership and
    /// [`may_delete`].
    ///
    /// # Errors
    /// `EntityNotFound` for a message that never had a photo, or a #general
    /// message whose file is not a photo; storage or database errors
    /// otherwise.
    pub async fn delete_photo(self, ctx: &AppContext) -> ModelResult<(Self, bool)> {
        if !self.has_photo {
            return Err(ModelError::EntityNotFound);
        }
        match self.photo_file() {
            None => Ok((self, false)),
            Some(PhotoFile::Own { key, .. }) => {
                // Deleting a key that is already gone succeeds, so a retry
                // after a failed update finishes the job.
                ctx.storage
                    .delete(Path::new(key))
                    .await
                    .map_err(|err| ModelError::Any(Box::new(err)))?;
                let mut message = self.into_active_model();
                message.photo_key = ActiveValue::Set(None);
                message.photo_type = ActiveValue::Set(None);
                message.photo_size = ActiveValue::Set(None);
                message.photo_name = ActiveValue::Set(None);
                Ok((message.update(&ctx.db).await?, true))
            }
            Some(PhotoFile::Document(file_id)) => {
                let changed =
                    match documents::Model::find_in_org(&ctx.db, self.organisation_id, file_id)
                        .await
                    {
                        // Only ever a photo, as `open_photo` serves.
                        Ok(file) if !PHOTO_TYPES.contains(&file.content_type.as_str()) => {
                            return Err(ModelError::EntityNotFound);
                        }
                        // Bytes, then the row; the foreign key clears `document_id`.
                        Ok(file) => {
                            file.delete_file(ctx).await?;
                            true
                        }
                        // Someone else just deleted it.
                        Err(ModelError::EntityNotFound) => false,
                        Err(err) => return Err(err),
                    };
                Ok((
                    Self {
                        document_id: None,
                        ..self
                    },
                    changed,
                ))
            }
        }
    }

    /// How many replies this message's thread has.
    ///
    /// # Errors
    /// On database errors.
    pub async fn reply_count<C: ConnectionTrait>(&self, db: &C) -> ModelResult<u64> {
        Ok(Entity::find()
            .in_tenant(self.organisation_id)
            .filter(Column::ParentId.eq(self.id))
            .count(db)
            .await?)
    }

    /// Deletes this message for everyone, for good, with its photo,
    /// reactions and bell notices; a thread parent takes its replies, and
    /// theirs. `by_manager` is an owner or admin; anyone else may delete only
    /// a message with no replies, checked again under the lock. Returns how
    /// many replies went, and who had notices about them, for their badges.
    ///
    /// The photo goes first, as [`Self::delete_photo`] does it (bytes, then
    /// the row), because storage can't join a transaction. The rest is one
    /// transaction, and the database cascades replies, reactions and
    /// notices. Replies never carry photos, so the cascade leaves no bytes
    /// behind. The caller checks membership and [`may_delete`].
    ///
    /// # Errors
    /// `Message(HAS_REPLIES)` when the sender's message has replies;
    /// `EntityNotFound` when it is already gone; storage or database errors,
    /// including a lock wait of more than 5 seconds.
    pub async fn delete_message(self, ctx: &AppContext, by_manager: bool) -> ModelResult<Deleted> {
        if !by_manager && self.reply_count(&ctx.db).await? > 0 {
            return Err(ModelError::msg(HAS_REPLIES));
        }
        if self.photo_file().is_some() {
            match self.clone().delete_photo(ctx).await {
                Ok(_) => {}
                // A #general file that is no longer a photo type. It can't
                // happen today: a file's type is set only at upload, and chat
                // uploads are sniffed to photo types. If it ever did, this
                // chat must not delete someone's Documents file, so it stays
                // there and the message goes without it.
                Err(ModelError::EntityNotFound) => {}
                Err(err) => return Err(err),
            }
        }
        let txn = ctx.db.begin().await?;
        txn.execute_unprepared("SET LOCAL lock_timeout = '5s'")
            .await?;
        let message = Entity::find()
            .in_tenant(self.organisation_id)
            .filter(Column::Id.eq(self.id))
            .filter(Column::ConversationId.eq(self.conversation_id))
            .lock_exclusive()
            .one(&txn)
            .await?
            .ok_or(ModelError::EntityNotFound)?;
        // A reply that committed since the first count is seen here; one
        // still saving holds FOR SHARE on this row, so the lock above waited.
        let replies = message.reply_count(&txn).await?;
        if !by_manager && replies > 0 {
            // Dropping `txn` rolls it back.
            return Err(ModelError::msg(HAS_REPLIES));
        }
        // The replies are locked too: a notice or reaction still saving on
        // one holds FOR SHARE on it, so this waits for it, and no new one
        // can start.
        let locked = Entity::find()
            .in_tenant(message.organisation_id)
            .filter(Column::ParentId.eq(message.id))
            .lock_exclusive()
            .all(&txn)
            .await?;
        // The cascade deletes reply rows, never stored bytes. Replies can't
        // have photos today; if one ever does, refuse rather than orphan it.
        if locked.iter().any(|r| r.has_photo) {
            return Err(ModelError::msg(REPLY_HAS_PHOTO));
        }
        // Deleted here rather than by the cascade, to learn whose bell
        // badges change. Nothing new can point at these rows now.
        let mut notified: Vec<i64> = notifications::Entity::delete_many()
            .filter(notifications::Column::OrganisationId.eq(message.organisation_id))
            .filter(
                notifications::Column::MessageId
                    .is_in(std::iter::once(message.id).chain(locked.iter().map(|r| r.id))),
            )
            .exec_with_returning(&txn)
            .await?
            .into_iter()
            .map(|n| n.user_id)
            .collect();
        notified.sort_unstable();
        notified.dedup();
        Entity::delete_by_id(message.id)
            .filter(Column::OrganisationId.eq(message.organisation_id))
            .exec(&txn)
            .await?;
        txn.commit().await?;
        Ok(Deleted { replies, notified })
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

    #[test]
    fn the_sender_owners_and_admins_may_delete_a_photo() {
        assert!(may_delete(3, 3, false), "the sender");
        assert!(may_delete(3, 1, true), "an owner or admin");
        assert!(!may_delete(3, 4, false), "another member");
    }

    #[test]
    fn only_owners_and_admins_delete_a_message_with_replies() {
        assert!(may_delete_message(3, 3, false, 0), "the sender, no replies");
        assert!(!may_delete_message(3, 3, false, 2), "the sender, replies");
        assert!(may_delete_message(3, 1, true, 0), "an owner or admin");
        assert!(
            may_delete_message(3, 1, true, 3),
            "an owner or admin, replies"
        );
        assert!(!may_delete_message(3, 4, false, 0), "another member");
    }
}
