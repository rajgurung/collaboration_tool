use std::collections::HashMap;

use axum::http::HeaderMap;
use chrono::{DateTime, FixedOffset, Utc};
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    extractors::{current_member::CurrentMember, session::redirect_response},
    models::{
        conversation_members,
        conversations::{self, kind, GroupParams},
        memberships,
        messages::{self, MessageParams},
        notifications::{mention_parts, Part},
    },
    views::{
        forms::{field_errors, invalid_form, toast, FieldErrors},
        layout::{avatar_color, Person},
    },
};

const GROUP_FORM_ID: &str = "group-form";
const FORMER_MEMBER: &str = "Former member";

#[derive(Debug, Default, Deserialize)]
struct LegacyQuery {
    c: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct UnreadQuery {
    style: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DmForm {
    user_id: i64,
}

/// One row in the conversation list.
#[derive(Debug, Serialize)]
pub struct ListItem {
    pub id: i64,
    pub kind: String,
    pub label: String,
    pub color: &'static str,
    pub last: Option<String>,
    pub time: String,
    pub unread: u64,
}

/// One rendered chat message. `own` decides the bubble style for the viewer;
/// `start` and `show_time` group consecutive messages from the same person.
#[derive(Debug, Clone, Serialize)]
pub struct MessageView {
    pub id: i64,
    pub author_id: i64,
    pub author: String,
    pub color: &'static str,
    pub body: String,
    /// The body split so mentions of teammates can be highlighted.
    pub parts: Vec<Part>,
    pub at: String,
    pub own: bool,
    pub start: bool,
    pub show_time: bool,
}

impl MessageView {
    #[must_use]
    pub fn new(message: &messages::Model, names: &HashMap<i64, String>, viewer_id: i64) -> Self {
        let author = names
            .get(&message.user_id)
            .cloned()
            .unwrap_or_else(|| FORMER_MEMBER.to_string());
        Self {
            id: message.id,
            author_id: message.user_id,
            color: avatar_color(&author),
            author,
            parts: mention_parts(
                &message.body,
                &names
                    .iter()
                    .map(|(id, n)| (*id, n.clone()))
                    .collect::<Vec<_>>(),
            ),
            body: message.body.clone(),
            at: message.created_at.format("%H:%M").to_string(),
            own: message.user_id == viewer_id,
            start: true,
            show_time: true,
        }
    }
}

/// Marks where each run of messages from one person starts and ends.
fn grouped(
    list: &[messages::Model],
    names: &HashMap<i64, String>,
    viewer_id: i64,
) -> Vec<MessageView> {
    let mut views: Vec<MessageView> = list
        .iter()
        .map(|m| MessageView::new(m, names, viewer_id))
        .collect();
    for i in 0..views.len() {
        let same_as_prev = i > 0 && views[i - 1].author_id == views[i].author_id;
        let same_as_next = i + 1 < views.len() && views[i + 1].author_id == views[i].author_id;
        views[i].start = !same_as_prev;
        views[i].show_time = !same_as_next;
    }
    views
}

/// Usernames of approved members, keyed by user id.
pub async fn names(ctx: &AppContext, org_id: i64) -> Result<HashMap<i64, String>> {
    Ok(memberships::Model::team(&ctx.db, org_id)
        .await?
        .into_iter()
        .collect())
}

/// What a conversation is called for this viewer: the channel name, or the other person in a DM.
fn label(
    conversation: &conversations::Model,
    members: &[i64],
    names: &HashMap<i64, String>,
    viewer_id: i64,
) -> String {
    if conversation.kind == kind::DM {
        members
            .iter()
            .find(|id| **id != viewer_id)
            .and_then(|id| names.get(id).cloned())
            .unwrap_or_else(|| FORMER_MEMBER.to_string())
    } else {
        conversation.name.clone().unwrap_or_default()
    }
}

/// "14:05" today, "Mon" this week, otherwise "3 Oct".
fn when(at: &DateTime<FixedOffset>) -> String {
    let age = Utc::now().signed_duration_since(at.with_timezone(&Utc));
    if at.date_naive() == Utc::now().date_naive() {
        at.format("%H:%M").to_string()
    } else if age.num_days() < 6 {
        at.format("%a").to_string()
    } else {
        at.format("%-d %b").to_string()
    }
}

/// The viewer's conversations with last message and unread count, most recent first.
pub async fn list_items(
    ctx: &AppContext,
    member: &CurrentMember,
    names: &HashMap<i64, String>,
) -> Result<Vec<ListItem>> {
    let me = member.user.id;
    let list = conversations::Model::list_for_user(&ctx.db, member.org.id, me).await?;
    let ids: Vec<i64> = list.iter().map(|c| c.id).collect();
    let latest = messages::Model::latest_per_conversation(&ctx.db, member.org.id, &ids).await?;
    let unread = conversation_members::Model::unread_counts(&ctx.db, member.org.id, me).await?;
    let mut items = Vec::with_capacity(list.len());
    for c in &list {
        let members = c.member_ids(&ctx.db).await?;
        let label = label(c, &members, names, me);
        let last = latest.get(&c.id);
        items.push((
            last.map(|m| m.created_at),
            ListItem {
                id: c.id,
                kind: c.kind.clone(),
                color: avatar_color(&label),
                last: last.map(|m| {
                    let who = if m.user_id == me {
                        "You".to_string()
                    } else {
                        names
                            .get(&m.user_id)
                            .cloned()
                            .unwrap_or_else(|| FORMER_MEMBER.to_string())
                    };
                    let body: String = m.body.chars().take(80).collect();
                    if c.kind == kind::DM && m.user_id != me {
                        body
                    } else {
                        format!("{who}: {body}")
                    }
                }),
                time: last.map(|m| when(&m.created_at)).unwrap_or_default(),
                unread: unread.get(&c.id).copied().unwrap_or(0),
                label,
            },
        ));
    }
    // Channels with activity first by recency; quiet ones keep their order at the end.
    items.sort_by_key(|item| std::cmp::Reverse(item.0));
    Ok(items.into_iter().map(|(_, item)| item).collect())
}

#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Query(legacy): Query<LegacyQuery>,
) -> Result<Response> {
    if let Some(id) = legacy.c {
        return format::redirect(&format!("/chat/{id}"));
    }
    let names = names(&ctx, member.org.id).await?;
    let items = list_items(&ctx, &member, &names).await?;
    format::render().view(
        &v,
        "chat/index.html",
        member.page("chat", data!({ "items": items })),
    )
}

#[debug_handler]
async fn show(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<i64>,
) -> Result<Response> {
    let me = member.user.id;
    let conversation =
        conversations::Model::find_for_member(&ctx.db, member.org.id, id, me).await?;
    conversation_members::Model::mark_read(&ctx.db, member.org.id, conversation.id, me).await?;
    let names = names(&ctx, member.org.id).await?;
    let items = list_items(&ctx, &member, &names).await?;
    let members = conversation.member_ids(&ctx.db).await?;
    let people: Vec<Person> = members
        .iter()
        .filter_map(|id| names.get(id).map(|n| (*id, n.clone())))
        .map(|(id, username)| Person {
            id,
            color: avatar_color(&username),
            username,
        })
        .collect();
    let feed = grouped(
        &messages::Model::recent(&ctx.db, &conversation).await?,
        &names,
        me,
    );
    let label = label(&conversation, &members, &names, me);
    format::render().view(
        &v,
        "chat/show.html",
        member.page(
            "chat",
            data!({
                "items": items,
                "conversation": {
                    "id": conversation.id,
                    "kind": conversation.kind,
                    "color": avatar_color(&label),
                    "label": label,
                    "members": people,
                    "mention_names": super::tasks::mention_names(
                        &people.iter().map(|p| (p.id, p.username.clone())).collect::<Vec<_>>(),
                        me,
                    ),
                },
                "messages": feed,
            }),
        ),
    )
}

/// Total unread across the viewer's conversations, as a badge (or nothing).
#[debug_handler]
async fn unread(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    Query(query): Query<UnreadQuery>,
) -> Result<Response> {
    let total: u64 =
        conversation_members::Model::unread_counts(&ctx.db, member.org.id, member.user.id)
            .await?
            .values()
            .sum();
    if total == 0 {
        return format::html("");
    }
    let class = if query.style.as_deref() == Some("tab") {
        "tab-badge"
    } else {
        "badge"
    };
    let shown = if total > 99 {
        "99+".to_string()
    } else {
        total.to_string()
    };
    format::html(&format!(
        r#"<span class="{class}" aria-label="{total} unread">{shown}</span>"#
    ))
}

/// Plain HTTP send, used when the live connection is not available.
#[debug_handler]
async fn send(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(params): Form<MessageParams>,
) -> Result<Response> {
    let conversation =
        conversations::Model::find_for_member(&ctx.db, member.org.id, id, member.user.id).await?;
    let result = messages::Model::create(&ctx.db, &conversation, member.user.id, &params).await;
    if let Ok(message) = &result {
        super::chat_ws::publish(&ctx, member.org.id, message).await?;
        super::notifications::message_sent(&ctx, member.org.id, &conversation, message).await?;
    }
    match result {
        Ok(_) if !headers.contains_key("hx-request") => Ok(redirect_response(
            &headers,
            &format!("/chat/{}", conversation.id),
        )),
        Ok(message) => {
            let names = names(&ctx, member.org.id).await?;
            let view = MessageView::new(&message, &names, member.user.id);
            format::render().view(&v, "chat/_message.html", data!({ "message": view }))
        }
        Err(err) => {
            let errors = field_errors(&err).ok_or(err)?;
            let message = errors.values().next().cloned().unwrap_or_default();
            format::render()
                .status(422)
                .header("HX-Reswap", "none")
                .header("HX-Trigger", toast("error", &message))
                .empty()
        }
    }
}

/// The message list on its own, for reloading after a reconnect.
#[debug_handler]
async fn feed(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<i64>,
) -> Result<Response> {
    let conversation =
        conversations::Model::find_for_member(&ctx.db, member.org.id, id, member.user.id).await?;
    conversation_members::Model::mark_read(&ctx.db, member.org.id, conversation.id, member.user.id)
        .await?;
    let names = names(&ctx, member.org.id).await?;
    let feed = grouped(
        &messages::Model::recent(&ctx.db, &conversation).await?,
        &names,
        member.user.id,
    );
    format::render().view(&v, "chat/_feed.html", data!({ "messages": feed }))
}

async fn team_except_me(ctx: &AppContext, member: &CurrentMember) -> Result<Vec<Person>> {
    Ok(
        Person::from_team(memberships::Model::team(&ctx.db, member.org.id).await?)
            .into_iter()
            .filter(|p| p.id != member.user.id)
            .collect(),
    )
}

#[debug_handler]
async fn new_group(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let team = team_except_me(&ctx, &member).await?;
    format::render().view(
        &v,
        "chat/_group_form.html",
        data!({ "team": team, "form": { "name": "", "member_ids": [] }, "errors": FieldErrors::new() }),
    )
}

#[debug_handler]
async fn create_group(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    axum_extra::extract::Form(params): axum_extra::extract::Form<GroupParams>,
) -> Result<Response> {
    match conversations::Model::create_group(&ctx.db, member.org.id, member.user.id, &params).await
    {
        Ok(group) => Ok(redirect_response(&headers, &format!("/chat/{}", group.id))),
        Err(err) => {
            let errors = field_errors(&err).ok_or(err)?;
            let team = team_except_me(&ctx, &member).await?;
            invalid_form(
                &v,
                "chat/_group_form.html",
                GROUP_FORM_ID,
                data!({ "team": team, "form": { "name": params.name, "member_ids": params.member_ids }, "errors": errors }),
            )
        }
    }
}

#[debug_handler]
async fn new_dm(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let team = team_except_me(&ctx, &member).await?;
    format::render().view(&v, "chat/_dm_picker.html", data!({ "team": team }))
}

#[debug_handler]
async fn start_dm(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Form(form): Form<DmForm>,
) -> Result<Response> {
    match conversations::Model::start_dm(&ctx.db, member.org.id, member.user.id, form.user_id).await
    {
        Ok(dm) => Ok(redirect_response(&headers, &format!("/chat/{}", dm.id))),
        Err(ModelError::Message(msg)) => Err(Error::BadRequest(msg)),
        Err(err) => Err(err.into()),
    }
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/chat", get(index))
        .add("/chat/unread", get(unread))
        .add("/chat/groups/new", get(new_group))
        .add("/chat/groups", post(create_group))
        .add("/chat/dms/new", get(new_dm))
        .add("/chat/dms", post(start_dm))
        .add("/chat/{id}", get(show))
        .add("/chat/{id}/messages", post(send))
        .add("/chat/{id}/feed", get(feed))
}
