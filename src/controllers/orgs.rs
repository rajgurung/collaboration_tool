use axum::http::{header, HeaderMap, HeaderValue};
use loco_rs::prelude::*;

use crate::{
    extractors::{
        current_member::{CurrentMember, CURRENT_ORG_COOKIE},
        current_user::CurrentUser,
        session::{
            append_cookie, cleared_acting_org_cookie, current_org_cookie, redirect_response,
        },
    },
    models::{memberships, organisations, users},
};

/// Where a switch lands. A constant: the switch never follows a `next`.
const AFTER_SWITCH: &str = "/dashboard?switched=1";

/// Makes this browser work in another organisation where the user is an
/// active member. `CurrentUser`, not `CurrentMember`, so someone on a waiting
/// page can still switch to an org that has approved them, and a stale tab
/// may switch. Only ever a POST: a link or redirect must never be able to
/// switch someone.
#[debug_handler]
async fn switch(
    CurrentUser(user): CurrentUser,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response> {
    // A 404 for every refusal, so it does not reveal whether the org exists.
    if memberships::Model::active_in(&ctx.db, id, user.id)
        .await?
        .is_none()
    {
        return Err(Error::NotFound);
    }
    // The unlock is already tied to its org. This is a backstop.
    users::Model::end_unlock(&ctx.db, user.id).await?;

    let mut response = redirect_response(&headers, AFTER_SWITCH);
    append_cookie(&mut response, &current_org_cookie(&ctx, id)?)?;
    append_cookie(&mut response, &cleared_acting_org_cookie(&ctx)?)?;
    users::Model::close_live_connections(&ctx, user.id);

    // Only a number is logged, so a hand-edited cookie cannot write into logs.
    let from_org = cookie::CookieJar::from_headers(&headers)
        .get(CURRENT_ORG_COOKIE)
        .and_then(|c| c.value().parse::<i64>().ok());
    tracing::info!(
        user_id = user.id,
        from_org,
        to_org = id,
        "switched organisation"
    );
    Ok(response)
}

/// Your organisations. Over HTMX it is the list the switcher menu and sheet
/// load when they first open; otherwise a page, for browsers without
/// JavaScript. `CurrentMember`, so a stale tab opening the menu is moved
/// first.
#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
) -> Result<Response> {
    let (active, pending): (Vec<_>, Vec<_>) =
        memberships::Model::for_user_with_orgs(&ctx.db, member.user.id)
            .await?
            .into_iter()
            .partition(|(m, _)| m.is_active());
    let row = |(m, org): &(memberships::Model, organisations::Model)| {
        serde_json::json!({
            "id": org.id,
            "name": org.name,
            "role": m.role,
            // While acting, the acting org has its own row at the top.
            "current": !member.acting && org.id == member.org.id,
        })
    };
    let data = member.page(
        "orgs",
        data!({
            "active_orgs": active.iter().map(row).collect::<Vec<_>>(),
            "pending_orgs": pending.iter().map(row).collect::<Vec<_>>(),
        }),
    );
    let template = if headers.contains_key("hx-request") {
        "orgs/_list.html"
    } else {
        "orgs/index.html"
    };
    let mut response = format::render().view(&v, template, data)?;
    let h = response.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::VARY, HeaderValue::from_static("HX-Request"));
    Ok(response)
}

/// Nothing, through `CurrentMember`. `app.ts` sends it when a tab may have
/// gone stale (the chat socket closed, the notification stream came back, a
/// page restored from the back-forward cache, before an upload), so a stale
/// tab gets the refusal and moves.
#[debug_handler]
async fn current(_member: CurrentMember, State(_ctx): State<AppContext>) -> Result<Response> {
    Ok((
        axum::http::StatusCode::NO_CONTENT,
        [(header::CACHE_CONTROL, "no-store")],
    )
        .into_response())
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/orgs", get(index))
        .add("/orgs/current", get(current))
        .add("/orgs/{id}/switch", post(switch))
}
