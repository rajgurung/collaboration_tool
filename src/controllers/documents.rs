//! The Documents space: folders of files every member can open. Every route
//! goes through `CurrentMember` and refuses a super admin acting inside
//! someone else's organisation. Files are streamed through the app after a
//! membership check; storage is never exposed and nothing is presigned.
use std::collections::HashMap;

use axum::{
    extract::{multipart::MultipartError, DefaultBodyLimit, Request},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
};
use bytes::{Bytes, BytesMut};
use chrono::{DateTime, FixedOffset, Utc};
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    extractors::{
        current_member::{forbidden, CurrentMember},
        session::redirect_response,
    },
    models::{
        documents::{self, MAX_BYTES},
        folders::{self, FolderParams},
        logins, memberships, messages,
    },
    views::{
        forms::{field_errors, invalid_form, toast},
        layout::avatar_color,
        time,
    },
};

const LOGINS_ONLY: &str =
    "The Logins folder holds logins only. Add photos and files to another folder.";

/// The upload route's body limit: the 25 MB file plus room for the form.
pub const UPLOAD_BODY_LIMIT: usize = 26 * 1024 * 1024;

#[derive(Debug, Serialize)]
pub struct FolderRow {
    id: i64,
    name: String,
    summary: String,
    selected: bool,
}

/// A login in a folder's list. Only plain fields: never a secret.
#[derive(Debug, Serialize)]
struct LoginRow {
    id: i64,
    title: String,
    username: String,
    added_by: String,
    color: &'static str,
    added: String,
}

/// One row of a folder: a file or a login, newest first.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Item {
    File(FileRow),
    Login(LoginRow),
}

#[derive(Debug, Serialize)]
struct FileRow {
    id: i64,
    name: String,
    stem: String,
    ext: &'static str,
    label: &'static str,
    is_image: bool,
    opens_inline: bool,
    uploader: String,
    color: &'static str,
    added: String,
    size: String,
    can_delete: bool,
}

/// "Just now", "Today 14:31", "Yesterday", then "3 Oct", in the organisation's zone.
fn added(at: DateTime<FixedOffset>, tz: chrono_tz::Tz) -> String {
    if (Utc::now() - at.with_timezone(&Utc)).num_minutes() < 1 {
        return "Just now".to_string();
    }
    let local = time::local(at, tz);
    let today = time::today(tz);
    if local.date_naive() == today {
        local.format("Today %H:%M").to_string()
    } else if today.pred_opt() == Some(local.date_naive()) {
        "Yesterday".to_string()
    } else {
        local.format("%-d %b").to_string()
    }
}

/// "Empty", "1 file", "4 files, 1 login", "2 logins".
fn item_count(files: u64, logins: u64) -> String {
    let plural = |n: u64, one: &str| match n {
        0 => None,
        1 => Some(format!("1 {one}")),
        n => Some(format!("{n} {one}s")),
    };
    let parts: Vec<String> = [plural(files, "file"), plural(logins, "login")]
        .into_iter()
        .flatten()
        .collect();
    if parts.is_empty() {
        "Empty".to_string()
    } else {
        parts.join(", ")
    }
}

/// The folder list, with counts and when each last changed.
///
/// # Errors
/// On database errors.
pub async fn folder_rows(
    ctx: &AppContext,
    member: &CurrentMember,
    selected: Option<i64>,
) -> Result<Vec<FolderRow>> {
    Ok(folders::Model::list_for_org(&ctx.db, member.org.id)
        .await?
        .into_iter()
        .map(|s| {
            let updated = s.updated_at.map(|at| {
                format!(
                    " · Updated {}",
                    time::local(at, member.tz()).format("%-d %b")
                )
            });
            FolderRow {
                id: s.folder.id,
                summary: format!(
                    "{}{}",
                    item_count(s.files, s.logins),
                    updated.unwrap_or_default()
                ),
                name: s.folder.name,
                selected: selected == Some(s.folder.id),
            }
        })
        .collect())
}

/// What `documents/_files.html` needs for one folder.
async fn files_data(
    ctx: &AppContext,
    member: &CurrentMember,
    folder: &folders::Model,
) -> Result<serde_json::Value> {
    // Everyone who ever joined, so files keep their uploader's name.
    let names: HashMap<i64, String> = memberships::Model::list_for_org(&ctx.db, member.org.id)
        .await?
        .into_iter()
        .map(|(m, _)| (m.user_id, m.username))
        .collect();
    let name_of = |id: Option<i64>| {
        id.and_then(|id| names.get(&id).cloned())
            .unwrap_or_else(|| "Former member".to_string())
    };
    let files = documents::Model::for_folder(&ctx.db, member.org.id, folder.id).await?;
    let logins = logins::Model::for_folder(&ctx.db, member.org.id, folder.id).await?;
    let count = item_count(files.len() as u64, logins.len() as u64);
    let mut items: Vec<(DateTime<FixedOffset>, Item)> = files
        .into_iter()
        .map(|f| {
            let uploader = name_of(f.uploaded_by_id);
            (
                f.created_at,
                Item::File(FileRow {
                    id: f.id,
                    stem: f.stem().to_string(),
                    ext: f.ext(),
                    label: f.label(),
                    is_image: f.is_image(),
                    opens_inline: f.opens_inline(),
                    color: avatar_color(&uploader),
                    uploader,
                    added: added(f.created_at, member.tz()),
                    size: documents::human_size(f.size_bytes),
                    can_delete: f.can_delete(member.user.id, member.can_manage()),
                    name: f.name,
                }),
            )
        })
        .collect();
    items.extend(logins.into_iter().map(|l| {
        let added_by = name_of(l.created_by_id);
        (
            l.created_at,
            Item::Login(LoginRow {
                id: l.id,
                title: l.title,
                username: l.username,
                color: avatar_color(&added_by),
                added_by,
                added: added(l.created_at, member.tz()),
            }),
        )
    }));
    items.sort_by_key(|item| std::cmp::Reverse(item.0));
    let items: Vec<Item> = items.into_iter().map(|(_, item)| item).collect();
    Ok(data!({
        "folder": {
            "id": folder.id,
            "name": folder.name,
            "note": folder.note,
            "is_logins": folder.is_logins(),
        },
        "count": count,
        "empty": items.is_empty(),
        "items": items,
    }))
}

/// The `#files` part of a folder page, after a change, with a toast.
async fn files_partial(
    ctx: &AppContext,
    v: &TeraView,
    member: &CurrentMember,
    folder: &folders::Model,
    message: &str,
) -> Result<Response> {
    format::render()
        .header("HX-Trigger", toast("success", message))
        .view(
            v,
            "documents/_files.html",
            member.page("documents", files_data(ctx, member, folder).await?),
        )
}

/// A refusal shown as an error toast, leaving the page as it is.
fn refuse(message: &str) -> Result<Response> {
    format::render()
        .status(422)
        .header("HX-Reswap", "none")
        .header("HX-Trigger", toast("error", message))
        .text(message)
}

#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    member.require_own_membership()?;
    format::render().view(
        &v,
        "documents/index.html",
        member.page(
            "documents",
            data!({ "folders": folder_rows(&ctx, &member, None).await? }),
        ),
    )
}

#[debug_handler]
async fn show_folder(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<i64>,
) -> Result<Response> {
    member.require_own_membership()?;
    let folder = folders::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    let mut page = files_data(&ctx, &member, &folder).await?;
    page["folders"] = serde_json::to_value(folder_rows(&ctx, &member, Some(folder.id)).await?)?;
    if member.can_manage() {
        page["login_form"] = super::logins::new_login_form(&ctx, &member, &folder).await?;
    }
    format::render().view(&v, "documents/folder.html", member.page("documents", page))
}

/// Owners and admins add a folder, then land in it.
#[debug_handler]
async fn create_folder(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    Form(params): Form<FolderParams>,
) -> Result<Response> {
    member.require_own_membership()?;
    member.require_manager()?;
    match folders::Model::create(&ctx.db, member.org.id, member.user.id, &params).await {
        Ok(folder) => Ok(redirect_response(
            &headers,
            &member.url(&format!("/documents/folders/{}", folder.id)),
        )),
        Err(err) => folder_form_error(&v, &member, None, &params, err),
    }
}

/// Owners and admins rename a folder or change its note.
#[debug_handler]
async fn edit_folder(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(params): Form<FolderParams>,
) -> Result<Response> {
    member.require_own_membership()?;
    member.require_manager()?;
    let folder = folders::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    match folder.edit(&ctx.db, &params).await {
        Ok(folder) => Ok(redirect_response(
            &headers,
            &member.url(&format!("/documents/folders/{}", folder.id)),
        )),
        Err(err) => folder_form_error(&v, &member, Some(id), &params, err),
    }
}

fn folder_form_error(
    v: &TeraView,
    member: &CurrentMember,
    id: Option<i64>,
    params: &FolderParams,
    err: ModelError,
) -> Result<Response> {
    let errors = field_errors(&err).ok_or(err)?;
    let form_id = id.map_or_else(
        || "new-folder-form".to_string(),
        |id| format!("folder-{id}-form"),
    );
    let action = id.map_or_else(
        || member.url("/documents/folders"),
        |id| member.url(&format!("/documents/folders/{id}")),
    );
    invalid_form(
        v,
        "documents/_folder_form.html",
        &form_id,
        member.page(
            "documents",
            data!({
                "form_id": form_id,
                "action": action,
                "values": { "name": params.name, "note": params.note },
                "errors": errors,
                "submit": if id.is_some() { "Save" } else { "Create folder" },
            }),
        ),
    )
}

/// Owners and admins delete a folder once it is empty.
#[debug_handler]
async fn delete_folder(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response> {
    member.require_own_membership()?;
    member.require_manager()?;
    let folder = folders::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    match folder.delete_if_empty(&ctx.db).await {
        Ok(()) => Ok(redirect_response(&headers, &member.url("/documents"))),
        Err(ModelError::Message(message)) => refuse(&message),
        Err(err) => Err(err.into()),
    }
}

/// Reads the one `file` part of an upload, refusing it as soon as it passes
/// 25 MB rather than reading the rest.
async fn read_file(multipart: &mut Multipart) -> std::result::Result<(String, Bytes), String> {
    read_upload(multipart, None)
        .await
        .map(|(name, bytes, _)| (name, bytes))
}

/// Reads the `file` part of an upload, as [`read_file`], and the text of the
/// `text_field` part if one is named and sent. The text may come before or
/// after the file; it is cut to 4,000 characters, which is more than any
/// field it is used for allows.
///
/// # Errors
/// The refusal to show, for a file over 25 MB, a broken upload or no file.
pub async fn read_upload(
    multipart: &mut Multipart,
    text_field: Option<&str>,
) -> std::result::Result<(String, Bytes, String), String> {
    let failed = |err: MultipartError| {
        if err.status() == StatusCode::PAYLOAD_TOO_LARGE {
            documents::too_big()
        } else {
            "Stopped part way. Check your connection and try again.".to_string()
        }
    };
    let mut file = None;
    let mut text = String::new();
    while let Some(mut field) = multipart.next_field().await.map_err(failed)? {
        if text_field.is_some() && field.name() == text_field {
            text = field
                .text()
                .await
                .map_err(failed)?
                .chars()
                .take(4000)
                .collect();
            continue;
        }
        if field.name() != Some("file") || file.is_some() {
            continue;
        }
        let name = field.file_name().unwrap_or("File").to_string();
        let mut bytes = BytesMut::new();
        while let Some(chunk) = field.chunk().await.map_err(failed)? {
            if bytes.len() + chunk.len() > MAX_BYTES {
                return Err(documents::too_big());
            }
            bytes.extend_from_slice(&chunk);
        }
        file = Some((name, bytes.freeze()));
        if text_field.is_none() {
            break;
        }
    }
    let (name, bytes) = file.ok_or_else(|| "Choose a file to add.".to_string())?;
    Ok((name, bytes, text))
}

/// Any member adds one file to a folder. Success refreshes the file list;
/// a refusal adds an error row above it, saying what to do next.
#[debug_handler]
async fn upload(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<i64>,
    mut multipart: Multipart,
) -> Result<Response> {
    member.require_own_membership()?;
    let folder = folders::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    if folder.is_logins() {
        return refuse(LOGINS_ONLY);
    }
    let (name, refusal) = match read_file(&mut multipart).await {
        Ok((name, bytes)) => {
            match documents::Model::upload(&ctx, &folder, member.user.id, &name, bytes).await {
                Ok(file) => {
                    let message = format!("Added {}", file.name);
                    return files_partial(&ctx, &v, &member, &folder, &message).await;
                }
                Err(ModelError::Message(message)) => (name, message),
                Err(err) => return Err(err.into()),
            }
        }
        Err(message) => (String::new(), message),
    };
    format::render()
        .status(422)
        .header("HX-Retarget", "#uploads")
        .header("HX-Reswap", "afterbegin")
        .view(
            &v,
            "documents/_upload_error.html",
            member.fragment(data!({ "name": name, "message": refusal })),
        )
}

#[derive(Debug, Deserialize)]
struct OpenQuery {
    download: Option<String>,
}

/// Streams a file to a member of its organisation. Photos and PDFs open in
/// the browser, everything else downloads (see [`send_stored`]).
#[debug_handler]
async fn open_file(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    Path(id): Path<i64>,
    Query(query): Query<OpenQuery>,
) -> Result<Response> {
    member.require_own_membership()?;
    let file = documents::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    send_stored(
        &ctx,
        &file.storage_key,
        &file.content_type,
        file.size_bytes,
        &file.name,
        query.download.is_some(),
    )
    .await
}

/// Streams stored bytes with the headers every file is sent with. The caller
/// has already checked who may see them. The headers stop the browser from
/// guessing another type, caching it, or letting any page frame it.
///
/// # Errors
/// 500 when storage can't be read.
pub async fn send_stored(
    ctx: &AppContext,
    key: &str,
    content_type: &str,
    size: i64,
    name: &str,
    download: bool,
) -> Result<Response> {
    let stream = ctx
        .storage
        .download_stream(std::path::Path::new(key))
        .await
        .map_err(|err| {
            tracing::error!(error = %err, key, "could not read a stored file");
            Error::InternalServerError
        })?;
    let value = |value: String| {
        HeaderValue::from_str(&value).map_err(|_| Error::string("invalid header value"))
    };
    let mut response = Response::new(stream.into_body());
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, value(content_type.to_string())?);
    headers.insert(header::CONTENT_LENGTH, value(size.to_string())?);
    headers.insert(
        header::CONTENT_DISPOSITION,
        value(documents::disposition_for(name, content_type, download))?,
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(documents::content_security_policy_for(content_type)),
    );
    // Documents routes also get this from `no_framing`; chat routes don't.
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    Ok(response)
}

#[derive(Debug, Deserialize)]
struct RenameForm {
    name: String,
}

/// Any member renames a file. The extension stays.
#[debug_handler]
async fn rename_file(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<i64>,
    Form(form): Form<RenameForm>,
) -> Result<Response> {
    member.require_own_membership()?;
    let file = documents::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    let folder = folders::Model::find_in_org(&ctx.db, member.org.id, file.folder_id).await?;
    match file.rename(&ctx.db, &form.name).await {
        Ok(file) => {
            let message = format!("Renamed to {}", file.name);
            files_partial(&ctx, &v, &member, &folder, &message).await
        }
        Err(ModelError::Message(message)) => refuse(&message),
        Err(err) => Err(err.into()),
    }
}

/// The uploader, owners and admins delete a file, for everyone, for good.
#[debug_handler]
async fn delete_file(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<i64>,
) -> Result<Response> {
    member.require_own_membership()?;
    let file = documents::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    if !file.can_delete(member.user.id, member.can_manage()) {
        return Err(forbidden(
            "Only the person who added a file, owners and admins can delete it.",
        ));
    }
    let folder = folders::Model::find_in_org(&ctx.db, member.org.id, file.folder_id).await?;
    let name = file.name.clone();
    // A #general photo: open chats swap it for "Photo removed".
    let chat_photo = messages::Model::for_document(&ctx.db, member.org.id, file.id).await?;
    file.delete_file(&ctx).await?;
    if let Some(message) = chat_photo {
        tracing::info!(
            org_id = member.org.id,
            conversation_id = message.conversation_id,
            message_id = message.id,
            actor_id = member.user.id,
            by_manager = message.user_id != member.user.id,
            "chat photo deleted from Documents"
        );
        if let Err(err) = super::chat_ws::publish_photo_removed(&ctx, &message) {
            tracing::error!(error = %err, message_id = message.id, "could not broadcast a removed photo");
        }
    }
    files_partial(&ctx, &v, &member, &folder, &format!("Deleted {name}")).await
}

/// No Documents page or file may be shown inside a frame, on any site.
pub async fn no_framing(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    if !headers.contains_key(header::CONTENT_SECURITY_POLICY) {
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("frame-ancestors 'none'"),
        );
    }
    response
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/documents")
        .add("/", get(index))
        .add("/folders", post(create_folder))
        .add("/folders/{id}", get(show_folder))
        .add("/folders/{id}", post(edit_folder))
        .add("/folders/{id}/delete", post(delete_folder))
        .add(
            "/folders/{id}/files",
            post(upload).layer(DefaultBodyLimit::max(UPLOAD_BODY_LIMIT)),
        )
        .add("/files/{id}", get(open_file))
        .add("/files/{id}/rename", post(rename_file))
        .add("/files/{id}/delete", post(delete_file))
        .layer(middleware::from_fn(no_framing))
}
