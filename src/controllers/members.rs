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
}

impl MemberRow {
    fn new(membership: &memberships::Model, user: &users::Model, me: &CurrentMember) -> Self {
        Self {
            id: membership.id,
            username: membership.username.clone(),
            email: user.email.clone(),
            role: membership.role.clone(),
            status: membership.status.clone(),
            color: avatar_color(&membership.username),
            is_me: user.id == me.user.id,
        }
    }
}

#[derive(Debug, Deserialize)]
struct RoleForm {
    role: String,
}

#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let rows: Vec<MemberRow> = memberships::Model::list_for_org(&ctx.db, member.org.id)
        .await?
        .iter()
        .map(|(m, u)| MemberRow::new(m, u, &member))
        .collect();
    let (pending, active): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .filter(|r| r.status != memberships::status::REJECTED)
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
    let membership = memberships::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    let user = users::Model::find_by_id(&ctx.db, membership.user_id).await?;
    let row = MemberRow::new(&membership, &user, member);
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
}
