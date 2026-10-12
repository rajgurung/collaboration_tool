use std::collections::HashMap;

use axum::http::HeaderMap;
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    data::settings::Settings,
    extractors::{current_member::CurrentMember, session::redirect_response},
    models::{memberships, organisations, users},
    views::{
        forms::{field_errors, toast},
        layout::avatar_color,
        time,
    },
};

#[derive(Debug, Serialize)]
struct MemberRow {
    id: i64,
    username: String,
    email: String,
    role: String,
    status: String,
    color: &'static str,
    is_me: bool,
    can_remove: bool,
}

impl MemberRow {
    fn new(
        membership: &memberships::Model,
        user: &users::Model,
        me: &CurrentMember,
        owners: usize,
    ) -> Self {
        Self {
            id: membership.id,
            username: membership.username.clone(),
            email: user.email.clone(),
            role: membership.role.clone(),
            status: membership.status.clone(),
            color: avatar_color(&membership.username),
            is_me: user.id == me.user.id,
            can_remove: !me.acting && membership.can_remove(me.user.id, &me.role, owners),
        }
    }
}

/// Someone who was removed, for the owners' "Removed" list.
#[derive(Debug, Serialize)]
struct RemovedRow {
    username: String,
    color: &'static str,
    removed_by: String,
    removed_on: String,
}

fn active_owners(rows: &[(memberships::Model, users::Model)]) -> usize {
    rows.iter()
        .filter(|(m, _)| m.is_active() && m.role == memberships::role::OWNER)
        .count()
}

#[derive(Debug, Deserialize)]
struct RoleForm {
    role: String,
}

#[derive(Debug, Deserialize)]
struct IndexQuery {
    removed: Option<i64>,
}

#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Query(query): Query<IndexQuery>,
) -> Result<Response> {
    let all = memberships::Model::list_for_org(&ctx.db, member.org.id).await?;
    let owners = active_owners(&all);
    let names: HashMap<i64, &str> = all
        .iter()
        .map(|(m, _)| (m.user_id, m.username.as_str()))
        .collect();
    let removed: Vec<RemovedRow> = all
        .iter()
        .filter(|(m, _)| m.is_removed())
        .map(|(m, _)| RemovedRow {
            username: m.username.clone(),
            color: avatar_color(&m.username),
            removed_by: m
                .removed_by_id
                .and_then(|id| names.get(&id).map(|n| (*n).to_string()))
                .unwrap_or_else(|| "someone".to_string()),
            removed_on: m.removed_at.map_or_else(String::new, |at| {
                time::local(at, member.tz()).format("%-d %b %Y").to_string()
            }),
        })
        .collect();
    // The name comes from the database, never from the URL.
    let flash = query
        .removed
        .and_then(|id| all.iter().find(|(m, _)| m.id == id && m.is_removed()))
        .map(|(m, _)| {
            data!({
                "kind": "success",
                "message": format!("{} was removed and signed out.", m.username),
            })
        });
    let (pending, active): (Vec<_>, Vec<_>) = all
        .iter()
        .map(|(m, u)| MemberRow::new(m, u, &member, owners))
        .filter(|r| {
            r.status == memberships::status::PENDING || r.status == memberships::status::ACTIVE
        })
        .partition(|r| r.status == memberships::status::PENDING);
    let join_url = format!(
        "{}/join/{}",
        Settings::from_context(&ctx)?.app_url,
        member.org.slug
    );
    format::render().view(
        &v,
        "members/index.html",
        member.page(
            "members",
            data!({
                "active_members": active,
                "pending": if member.can_manage() { pending } else { Vec::new() },
                "removed": if member.is_owner() { removed } else { Vec::new() },
                "flash": flash,
                "join_url": join_url,
                "timezone": member.org.timezone,
                "timezones": time::all_names(),
                "local_now": chrono::Utc::now().with_timezone(&member.tz()).format("%H:%M %Z").to_string(),
            }),
        ),
    )
}

#[debug_handler]
async fn approve(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response> {
    member.require_manager()?;
    let target = memberships::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    let result = target.approve(&ctx.db, member.user.id).await;
    respond(&ctx, &v, &member, &headers, id, result, "approved").await
}

#[debug_handler]
async fn reject(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response> {
    member.require_manager()?;
    let target = memberships::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    let result = target.reject(&ctx.db, member.user.id).await;
    respond(&ctx, &v, &member, &headers, id, result, "declined").await
}

#[debug_handler]
async fn change_role(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(form): Form<RoleForm>,
) -> Result<Response> {
    member.require_owner()?;
    let target = memberships::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    let result = target.change_role(&ctx.db, &form.role).await;
    respond(&ctx, &v, &member, &headers, id, result, "updated").await
}

/// HTMX requests get the member's refreshed row plus a toast. Plain form posts
/// go back to the members page. Rule violations (e.g. approving twice) are
/// shown as an error toast rather than a failed request.
async fn respond(
    ctx: &AppContext,
    v: &TeraView,
    member: &CurrentMember,
    headers: &HeaderMap,
    id: i64,
    result: ModelResult<memberships::Model>,
    verb: &str,
) -> Result<Response> {
    let toast = match result {
        Ok(m) => {
            serde_json::json!({ "kind": "success", "message": format!("{} {verb}.", m.username) })
        }
        Err(ModelError::Message(msg)) => serde_json::json!({ "kind": "error", "message": msg }),
        Err(err) => return Err(err.into()),
    };
    if !headers.contains_key("hx-request") {
        return Ok(redirect_response(headers, "/members"));
    }
    let all = memberships::Model::list_for_org(&ctx.db, member.org.id).await?;
    let (membership, user) = all
        .iter()
        .find(|(m, _)| m.id == id)
        .ok_or(Error::NotFound)?;
    let row = MemberRow::new(membership, user, member, active_owners(&all));
    let template = if row.status == memberships::status::PENDING
        || row.status == memberships::status::REJECTED
    {
        "members/_pending_row.html"
    } else {
        "members/_row.html"
    };
    format::render()
        .header(
            "HX-Trigger",
            serde_json::json!({ "toast": toast }).to_string(),
        )
        .view(v, template, member.page("members", data!({ "row": row })))
}

/// Owners and admins remove someone from the organisation, then see the
/// members page again. A refusal is an error toast.
#[debug_handler]
async fn remove(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response> {
    member.require_manager()?;
    member.require_own_membership()?;
    match memberships::Model::remove(&ctx, member.org.id, member.user.id, id).await {
        // A full reload, so the counts and the Removed list are current.
        Ok(removed) => Ok(redirect_response(
            &headers,
            &format!("/members?removed={}", removed.id),
        )),
        Err(ModelError::Message(message)) => format::render()
            .status(422)
            .header("HX-Reswap", "none")
            .header("HX-Trigger", toast("error", &message))
            .text(&message),
        Err(err) => Err(err.into()),
    }
}

#[derive(Debug, Deserialize)]
struct TimezoneForm {
    timezone: String,
}

/// Owners and admins choose the organisation's time zone. Times are stored in
/// UTC and shown in this zone everywhere in the app.
#[debug_handler]
async fn set_timezone(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Form(form): Form<TimezoneForm>,
) -> Result<Response> {
    member.require_manager()?;
    let org = organisations::Model::find_by_id(&ctx.db, member.org.id).await?;
    match org.set_timezone(&ctx.db, &form.timezone).await {
        Ok(org) if headers.contains_key("hx-request") => format::render()
            .header(
                "HX-Trigger",
                toast("success", &format!("Times now show in {}", org.timezone)),
            )
            .empty(),
        Ok(_) => Ok(redirect_response(&headers, "/members")),
        Err(err) => {
            let message = field_errors(&err)
                .and_then(|e| e.values().next().cloned())
                .ok_or(err)?;
            format::render()
                .status(422)
                .header("HX-Reswap", "none")
                .header("HX-Trigger", toast("error", &message))
                .empty()
        }
    }
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/members", get(index))
        .add("/members/timezone", post(set_timezone))
        .add("/members/{id}/approve", post(approve))
        .add("/members/{id}/reject", post(reject))
        .add("/members/{id}/role", post(change_role))
        .add("/members/{id}/remove", post(remove))
}
