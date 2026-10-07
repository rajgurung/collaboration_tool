use std::collections::HashMap;

use axum::http::HeaderMap;
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    extractors::{current_member::CurrentMember, session::redirect_response},
    models::{
        conversations::{self, kind, GroupParams},
        memberships,
        messages::{self, MessageParams},
    },
    views::{
        forms::{field_errors, invalid_form, toast, FieldErrors},
        layout::{avatar_color, Person},
    },
};

const GROUP_FORM_ID: &str = "group-form";
const FORMER_MEMBER: &str = "Former member";

#[derive(Debug, Default, Deserialize)]
struct ChatQuery {
    c: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct DmForm {
    user_id: i64,
}

#[derive(Debug, Serialize)]
struct NavItem {
    id: i64,
    kind: String,
    label: String,
    color: &'static str,
    last: Option<String>,
}

/// One rendered chat message. `own` decides the bubble style for the viewer.
#[derive(Debug, Clone, Serialize)]
pub struct MessageView {
    pub id: i64,
    pub author: String,
    pub color: &'static str,
    pub body: String,
    pub at: String,
    pub own: bool,
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
            color: avatar_color(&author),
            author,
            body: message.body.clone(),
            at: message.created_at.format("%d %b, %H:%M").to_string(),
            own: message.user_id == viewer_id,
        }
    }
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

#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Query(query): Query<ChatQuery>,
) -> Result<Response> {
    let me = member.user.id;
    let names = names(&ctx, member.org.id).await?;
    let list = conversations::Model::list_for_user(&ctx.db, member.org.id, me).await?;
    let ids: Vec<i64> = list.iter().map(|c| c.id).collect();
    let latest = messages::Model::latest_per_conversation(&ctx.db, member.org.id, &ids).await?;

    let mut nav = Vec::new();
    let mut members_by_conversation = HashMap::new();
    for c in &list {
        let members = c.member_ids(&ctx.db).await?;
        let label = label(c, &members, &names, me);
        nav.push(NavItem {
            id: c.id,
            kind: c.kind.clone(),
            color: avatar_color(&label),
            label,
            last: latest.get(&c.id).map(|m| m.body.chars().take(60).collect()),
        });
        members_by_conversation.insert(c.id, members);
    }

    let active = query
        .c
        .and_then(|id| list.iter().find(|c| c.id == id))
        .or_else(|| list.first());
    let (active_view, feed) = match active {
        Some(c) => {
            let members = &members_by_conversation[&c.id];
            let people: Vec<Person> = members
                .iter()
                .filter_map(|id| names.get(id).map(|n| (*id, n.clone())))
                .map(|(id, username)| Person {
                    id,
                    color: avatar_color(&username),
                    username,
                })
                .collect();
            let feed: Vec<MessageView> = messages::Model::recent(&ctx.db, c)
                .await?
                .iter()
                .map(|m| MessageView::new(m, &names, me))
                .collect();
            let view = serde_json::json!({
                "id": c.id,
                "kind": c.kind,
                "label": label(c, members, &names, me),
                "members": people,
            });
            (Some(view), feed)
        }
        None => (None, Vec::new()),
    };

    format::render().view(
        &v,
        "chat/index.html",
        member.page(
            "chat",
            data!({
                "channels": nav.iter().filter(|n| n.kind != kind::DM).collect::<Vec<_>>(),
                "dms": nav.iter().filter(|n| n.kind == kind::DM).collect::<Vec<_>>(),
                "conversation": active_view,
                "messages": feed,
                "team_size": names.len(),
            }),
        ),
    )
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
    }
    match result {
        Ok(_) if !headers.contains_key("hx-request") => Ok(redirect_response(
            &headers,
            &format!("/chat?c={}", conversation.id),
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
    let names = names(&ctx, member.org.id).await?;
    let feed: Vec<MessageView> = messages::Model::recent(&ctx.db, &conversation)
        .await?
        .iter()
        .map(|m| MessageView::new(m, &names, member.user.id))
        .collect();
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
        Ok(group) => Ok(redirect_response(
            &headers,
            &format!("/chat?c={}", group.id),
        )),
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
        Ok(dm) => Ok(redirect_response(&headers, &format!("/chat?c={}", dm.id))),
        Err(ModelError::Message(msg)) => Err(Error::BadRequest(msg)),
        Err(err) => Err(err.into()),
    }
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/chat", get(index))
        .add("/chat/{id}/messages", post(send))
        .add("/chat/{id}/feed", get(feed))
        .add("/chat/groups/new", get(new_group))
        .add("/chat/groups", post(create_group))
        .add("/chat/dms/new", get(new_dm))
        .add("/chat/dms", post(start_dm))
}
