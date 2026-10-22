//! The gate for `/mcp`: an `Authorization: Bearer` token (OAuth or personal)
//! that is live, for this server, and belongs to someone who is still an
//! active member of the token's organisation. The member and the connection
//! go into the request extensions, where the MCP tools read them.
use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::Next,
};
use loco_rs::prelude::*;

use super::current_member::CurrentMember;
use crate::{
    data::settings::Settings,
    models::{access_tokens, claude_update_notices, memberships, organisations, users},
};

/// The Claude connection a bearer token belongs to. Never in a tool result.
#[derive(Clone, Copy, Debug)]
pub struct McpConnection {
    /// The same for the life of the connection, across token refreshes.
    pub grant_id: Uuid,
    /// The connection predates writes naming their organisation, and hasn't
    /// made one yet: tool results tell Claude to reload its tools.
    pub update_notice: bool,
}

/// Route layer for `/mcp`, added with `from_fn_with_state`.
pub async fn require_bearer(
    State(ctx): State<AppContext>,
    mut req: Request,
    next: Next,
) -> Response {
    let settings = match Settings::from_context(&ctx) {
        Ok(settings) => settings,
        Err(err) => return err.into_response(),
    };
    let Some(token) = bearer_token(req.headers()) else {
        return challenge(&settings, false);
    };
    match authenticate(&ctx, &settings, &token).await {
        Ok(Some((member, connection))) => {
            req.extensions_mut().insert(member);
            req.extensions_mut().insert(connection);
            next.run(req).await
        }
        Ok(None) => challenge(&settings, true),
        Err(err) => err.into_response(),
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then(|| token.to_string())
}

async fn authenticate(
    ctx: &AppContext,
    settings: &Settings,
    token: &str,
) -> Result<Option<(CurrentMember, McpConnection)>> {
    let Some(row) = access_tokens::Model::find_usable(&ctx.db, token, &settings.mcp_url()).await?
    else {
        return Ok(None);
    };
    // The token is for one organisation: check the membership in that one.
    let Some(membership) =
        memberships::Model::active_in(&ctx.db, row.organisation_id, row.user_id).await?
    else {
        return Ok(None);
    };
    // A locked account (too many wrong app passwords) is shut out until reset.
    let Some(user) = users::Entity::find_by_id(row.user_id)
        .one(&ctx.db)
        .await?
        .filter(|u| !u.is_locked())
    else {
        return Ok(None);
    };
    let org = organisations::Model::find_by_id(&ctx.db, row.organisation_id).await?;
    row.touch(&ctx.db).await?;
    let connection = McpConnection {
        grant_id: row.grant_id,
        update_notice: claude_update_notices::Model::due(&ctx.db, row.grant_id).await?,
    };
    Ok(Some((
        CurrentMember::from_membership(user, org, membership),
        connection,
    )))
}

/// 401 that tells the client where to find out how to sign in (RFC 9728).
fn challenge(settings: &Settings, invalid: bool) -> Response {
    let error = if invalid {
        r#"error="invalid_token", "#
    } else {
        ""
    };
    let value = format!(
        r#"Bearer {error}resource_metadata="{}/.well-known/oauth-protected-resource", scope="{}""#,
        settings.app_url,
        access_tokens::SCOPE
    );
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, value)],
        "Sign in to use Collab Tool.",
    )
        .into_response()
}
