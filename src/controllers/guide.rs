//! The "Getting started" guide on Home: close it, bring it back, and note
//! that the welcome pop-up was seen.
use axum::http::HeaderMap;
use loco_rs::prelude::*;

use crate::{
    extractors::{current_member::CurrentMember, session::redirect_response},
    models::memberships,
    views::forms::toast,
};

/// Closes the guide for good. With HTMX the card swaps itself out.
#[debug_handler]
async fn dismiss(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<Response> {
    memberships::Model::set_guide_hidden(&ctx.db, member.org.id, member.user.id, true).await?;
    if !headers.contains_key("hx-request") {
        return Ok(redirect_response(&headers, "/dashboard"));
    }
    format::render()
        .header(
            "HX-Trigger",
            toast(
                "info",
                "Guide closed. Bring it back from your account page any time.",
            ),
        )
        .html("")
}

/// Brings the guide back on Home.
#[debug_handler]
async fn show(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<Response> {
    memberships::Model::set_guide_hidden(&ctx.db, member.org.id, member.user.id, false).await?;
    Ok(redirect_response(&headers, "/dashboard"))
}

/// The welcome pop-up was closed or finished; it doesn't show again.
#[debug_handler]
async fn welcomed(member: CurrentMember, State(ctx): State<AppContext>) -> Result<Response> {
    memberships::Model::mark_welcomed(&ctx.db, member.org.id, member.user.id).await?;
    format::empty()
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/guide/dismiss", post(dismiss))
        .add("/guide/show", post(show))
        .add("/guide/welcomed", post(welcomed))
}
