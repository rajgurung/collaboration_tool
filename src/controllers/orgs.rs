use axum::http::HeaderMap;
use loco_rs::prelude::*;

use crate::{
    extractors::{
        current_member::CURRENT_ORG_COOKIE,
        current_user::CurrentUser,
        session::{
            append_cookie, cleared_acting_org_cookie, current_org_cookie, redirect_response,
        },
    },
    models::{memberships, users},
};

/// Makes this browser work in another organisation where the user is an
/// active member. `CurrentUser`, not `CurrentMember`, so someone on a waiting
/// page can still switch to an org that has approved them. Only ever a POST:
/// a link or redirect must never be able to switch someone.
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

    let mut response = redirect_response(&headers, "/dashboard");
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

pub fn routes() -> Routes {
    Routes::new().add("/orgs/{id}/switch", post(switch))
}
