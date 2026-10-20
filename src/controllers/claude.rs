//! "Connect Claude": how to add Collab Tool to claude.ai or Claude Code,
//! personal access tokens, and the list of connections with Revoke.
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use loco_rs::prelude::*;
use serde::Deserialize;

use crate::{
    data::settings::Settings,
    extractors::{
        current_member::{CurrentMember, PinnedMember, SWITCHED_ORG_TOKEN},
        session::redirect_response,
    },
    models::access_tokens::{self, PersonalParams, MAX_PERSONAL},
    views::{
        forms::{field_errors, FieldErrors},
        time,
    },
};

/// What the page shows besides the connections: a token just made (shown
/// once), or the form's errors.
#[derive(Default)]
struct Extra {
    new_token: Option<String>,
    name: String,
    errors: FieldErrors,
    error: Option<&'static str>,
}

/// The token form. `org_id` is the organisation the form was shown for, a
/// string checked by hand like on the Allow page.
#[derive(Deserialize)]
struct TokenForm {
    name: String,
    org_id: Option<String>,
}

async fn page(
    ctx: &AppContext,
    member: &CurrentMember,
    v: &TeraView,
    status: StatusCode,
    extra: Extra,
) -> Result<Response> {
    let mcp_url = Settings::from_context(ctx)?.mcp_url();
    let tz = member.tz();
    let connections: Vec<serde_json::Value> =
        access_tokens::Model::connections(&ctx.db, member.org.id, member.user.id)
            .await?
            .into_iter()
            .map(|c| {
                serde_json::json!({
                    "grant_id": c.grant_id,
                    "name": c.name,
                    "personal": c.kind == access_tokens::kind::PERSONAL,
                    "connected": time::local(c.created_at, tz).format("%-d %b %Y").to_string(),
                    "last_used": c.last_used_at
                        .map(|at| time::local(at, tz).format("%-d %b, %H:%M").to_string()),
                })
            })
            .collect();
    let mut response = format::render().status(status.as_u16()).view(
        v,
        "claude/index.html",
        member.page(
            "claude",
            data!({
                "mcp_url": mcp_url,
                "connections": connections,
                "max_tokens": MAX_PERSONAL,
                "new_token": extra.new_token,
                "name": extra.name,
                "errors": extra.errors,
                "error": extra.error,
                "org_id": member.org.id,
            }),
        ),
    )?;
    // A new token is in this page; keep it out of every cache.
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    page(&ctx, &member, &v, StatusCode::OK, Extra::default()).await
}

/// Makes a personal token and shows it once, in this response only: never
/// in a redirect or a URL, which would end up in logs.
#[debug_handler]
async fn create_token(
    PinnedMember(member): PinnedMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Form(form): Form<TokenForm>,
) -> Result<Response> {
    // The page explains how to leave; a token would be for the wrong organisation.
    if member.acting {
        return page(&ctx, &member, &v, StatusCode::FORBIDDEN, Extra::default()).await;
    }
    if !member.shown_for_this_org(form.org_id.as_deref()) {
        let extra = Extra {
            error: Some(SWITCHED_ORG_TOKEN),
            name: form.name,
            ..Extra::default()
        };
        return page(&ctx, &member, &v, StatusCode::CONFLICT, extra).await;
    }
    let params = PersonalParams { name: form.name };
    let mcp_url = Settings::from_context(&ctx)?.mcp_url();
    match access_tokens::Model::create_personal(
        &ctx.db,
        member.org.id,
        member.user.id,
        &params,
        &mcp_url,
    )
    .await
    {
        Ok((_, token)) => {
            let extra = Extra {
                new_token: Some(token),
                ..Extra::default()
            };
            page(&ctx, &member, &v, StatusCode::OK, extra).await
        }
        Err(err) => {
            let errors = field_errors(&err).ok_or(err)?;
            let extra = Extra {
                name: params.name,
                errors,
                ..Extra::default()
            };
            page(&ctx, &member, &v, StatusCode::UNPROCESSABLE_ENTITY, extra).await
        }
    }
}

/// Disconnects a personal token or an OAuth connection (every token in it).
#[debug_handler]
async fn revoke(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(grant_id): Path<Uuid>,
) -> Result<Response> {
    access_tokens::Model::revoke_mine(&ctx.db, member.org.id, member.user.id, grant_id).await?;
    Ok(redirect_response(&headers, &member.url("/settings/claude")))
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/settings/claude", get(index))
        .add("/settings/claude/tokens", post(create_token))
        .add("/settings/claude/grants/{grant_id}/revoke", post(revoke))
}
