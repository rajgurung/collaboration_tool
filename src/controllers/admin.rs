use axum::http::HeaderMap;
use loco_rs::prelude::*;

use crate::{
    extractors::{
        current_user::CurrentUser,
        session::{acting_org_cookie, cleared_acting_org_cookie, redirect_response},
    },
    models::{memberships, organisations, users},
    views::layout::avatar_color,
};

/// Anyone who is not the super admin gets a plain 404, so the admin area is not advertised.
fn require_super_admin(user: &users::Model) -> Result<()> {
    if user.is_super_admin {
        Ok(())
    } else {
        Err(Error::NotFound)
    }
}

#[debug_handler]
async fn index(
    CurrentUser(user): CurrentUser,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    require_super_admin(&user)?;
    let orgs = organisations::Model::admin_overview(&ctx.db).await?;
    let names: std::collections::HashMap<i64, String> =
        orgs.iter().map(|o| (o.id, o.name.clone())).collect();
    let memberships = memberships::Model::all_by_user(&ctx.db).await?;
    let people: Vec<serde_json::Value> = users::Model::all_newest_first(&ctx.db)
        .await?
        .into_iter()
        .map(|u| {
            let membership = memberships.get(&u.id);
            serde_json::json!({
                "username": u.name,
                "email": u.email,
                "color": avatar_color(&u.name),
                "org": membership.and_then(|m| names.get(&m.organisation_id)),
                "role": membership.map(|m| m.role.clone()),
                "status": membership.map(|m| m.status.clone()),
                "is_super_admin": u.is_super_admin,
                "joined": u.created_at.format("%d %b %Y").to_string(),
            })
        })
        .collect();
    format::render().view(
        &v,
        "admin/index.html",
        data!({
            "active": "admin",
            "org": { "name": "Platform admin" },
            "me": { "username": user.name, "color": avatar_color(&user.name), "is_super_admin": true },
            "orgs": orgs,
            "people": people,
        }),
    )
}

/// Work inside any organisation as its owner.
#[debug_handler]
async fn enter(
    CurrentUser(user): CurrentUser,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response> {
    require_super_admin(&user)?;
    let org = organisations::Model::find_by_id(&ctx.db, id).await?;
    tracing::info!(
        admin = user.email,
        org = org.slug,
        "super admin entered organisation"
    );
    let mut response = redirect_response(&headers, "/dashboard");
    append_cookie(&mut response, &acting_org_cookie(&ctx, org.id)?)?;
    Ok(response)
}

/// Back to the admin's own organisation (or the admin page if they have none).
#[debug_handler]
async fn leave(
    CurrentUser(user): CurrentUser,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
) -> Result<Response> {
    require_super_admin(&user)?;
    let mut response = redirect_response(&headers, "/admin");
    append_cookie(&mut response, &cleared_acting_org_cookie(&ctx)?)?;
    Ok(response)
}

fn append_cookie(response: &mut Response, cookie: &cookie::Cookie<'_>) -> Result<()> {
    response.headers_mut().append(
        axum::http::header::SET_COOKIE,
        cookie
            .to_string()
            .parse()
            .map_err(|_| Error::string("invalid cookie header"))?,
    );
    Ok(())
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/admin", get(index))
        .add("/admin/orgs/{id}/enter", post(enter))
        .add("/admin/leave", post(leave))
}
