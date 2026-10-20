use axum::{
    http::{header, HeaderMap, HeaderValue},
    Extension,
};
use loco_rs::prelude::*;

use crate::{
    extractors::{
        current_member::{CurrentMember, CURRENT_ORG_COOKIE},
        current_user::CurrentUser,
        session::{
            append_cookie, cleared_acting_org_cookie, current_org_cookie, redirect_response,
        },
    },
    initializers::org_path::PathOrg,
    models::{
        memberships,
        organisations::{self, NewOrgParams, AT_CAP, MAX_ORGS_CREATED},
    },
    views::{
        forms::{field_errors, invalid_form, FieldErrors},
        links::{org_path, section_path},
    },
};

/// Opens `org` in this tab, the same way for the old switch and a create:
/// remembers it in `current_org`, clears `acting_org`, and goes to `path`
/// there. Other tabs keep their own address, so nothing else is closed or
/// ended.
fn enter_org(
    ctx: &AppContext,
    headers: &HeaderMap,
    org: &organisations::Model,
    path: &str,
) -> Result<Response> {
    let mut response = redirect_response(headers, &org_path(&org.slug, path));
    append_cookie(&mut response, &current_org_cookie(ctx, org.id)?)?;
    append_cookie(&mut response, &cleared_acting_org_cookie(ctx)?)?;
    Ok(response)
}

/// The org the request's `current_org` cookie names, for the logs. Only a
/// number, so a hand-edited cookie cannot write into them.
fn from_org(headers: &HeaderMap) -> Option<i64> {
    cookie::CookieJar::from_headers(headers)
        .get(CURRENT_ORG_COOKIE)
        .and_then(|c| c.value().parse::<i64>().ok())
}

/// The old switch, kept for one release for tabs drawn before the switcher
/// became links (and the old stale banner's "Back"). It opens Home in an org
/// where the user is active. `CurrentUser`, not `CurrentMember`, and only
/// ever a POST. Removed in PR 3.
#[debug_handler]
async fn switch(
    // First, so `/o/{slug}/orgs/{id}/switch` is a 404 before anything else.
    path_org: Option<Extension<PathOrg>>,
    CurrentUser(user): CurrentUser,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response> {
    if path_org.is_some() {
        return Err(Error::NotFound);
    }
    // A 404 for every refusal, so it does not reveal whether the org exists.
    if memberships::Model::active_in(&ctx.db, id, user.id)
        .await?
        .is_none()
    {
        return Err(Error::NotFound);
    }
    let org = organisations::Model::find_by_id(&ctx.db, id).await?;
    let response = enter_org(&ctx, &headers, &org, "/dashboard")?;
    tracing::info!(
        user_id = user.id,
        from_org = from_org(&headers),
        to_org = id,
        "switched organisation"
    );
    Ok(response)
}

/// The create form's fields, plus `as_page`, which the `/orgs/new` page
/// form sends so its redrawn form keeps the page's buttons.
#[derive(Debug, serde::Deserialize)]
struct CreateForm {
    #[serde(flatten)]
    params: NewOrgParams,
    #[serde(default)]
    as_page: Option<String>,
}

/// What the "New organisation" form shows: the typed values and their
/// errors, or the at-cap state in place of the fields. Over HTMX the form
/// alone, for the dialog, the phone sheet or the page's own form (which
/// says so with `on_page`); otherwise the `/orgs/new` page.
fn new_form(
    member: &CurrentMember,
    headers: &HeaderMap,
    on_page: bool,
    values: (&str, &str),
    errors: &FieldErrors,
) -> (&'static str, serde_json::Value) {
    let fragment = headers.contains_key("hx-request");
    let as_page = on_page || !fragment;
    let data = member.page(
        "orgs",
        data!({
            "form": { "organisation_name": values.0, "name": values.1 },
            "errors": errors,
            "at_cap": errors.contains_key(AT_CAP),
            "max_orgs": MAX_ORGS_CREATED,
            "as_page": as_page,
        }),
    );
    let template = if fragment {
        "orgs/_new_form.html"
    } else {
        "orgs/new.html"
    };
    (template, data)
}

fn no_store(mut response: Response) -> Response {
    let h = response.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::VARY, HeaderValue::from_static("HX-Request"));
    response
}

/// The "New organisation" form, starting with your username here. At the
/// cap it says so instead; the `POST` checks again under its lock.
#[debug_handler]
async fn new(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
) -> Result<Response> {
    let mut errors = FieldErrors::new();
    if organisations::Model::created_count(&ctx.db, member.user.id).await? >= MAX_ORGS_CREATED {
        errors.insert(AT_CAP.to_string(), String::new());
    }
    let (template, data) = new_form(&member, &headers, false, ("", &member.username), &errors);
    Ok(no_store(format::render().view(&v, template, data)?))
}

/// Creates an organisation with the person as its owner, then opens it in
/// this tab. It reads only `member.user` and the
/// current org's time zone, which it copies unless acting: the acting org
/// is someone else's. `CurrentMember`, so a stale tab is refused like any
/// other write.
#[debug_handler]
async fn create(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    Form(CreateForm { params, as_page }): Form<CreateForm>,
) -> Result<Response> {
    let timezone = (!member.acting).then_some(member.org.timezone.as_str());
    match organisations::Model::create_for_user(&ctx.db, &member.user, &params, timezone).await {
        Ok(org) => {
            let response = enter_org(&ctx, &headers, &org, "/dashboard?created=1")?;
            tracing::info!(
                user_id = member.user.id,
                org_id = org.id,
                from_org = from_org(&headers),
                acting_org = member.acting.then_some(member.org.id),
                "created organisation"
            );
            Ok(response)
        }
        Err(err) => {
            let errors = field_errors(&err).ok_or(err)?;
            let values = (params.organisation_name.as_str(), params.name.as_str());
            let (template, data) = new_form(&member, &headers, as_page.is_some(), values, &errors);
            if template == "orgs/new.html" {
                Ok(no_store(
                    format::render().status(422).view(&v, template, data)?,
                ))
            } else {
                Ok(no_store(invalid_form(&v, template, "org-new-form", data)?))
            }
        }
    }
}

/// Which list the switcher's rows open in the other org: the page's
/// `active` key, mapped through [`section_path`], so a hand-made value only
/// ever gives Home.
#[derive(Debug, Default, serde::Deserialize)]
struct ListQuery {
    section: Option<String>,
}

/// Your organisations. Over HTMX it is the list the switcher menu and sheet
/// load when they first open; otherwise a page, for browsers without
/// JavaScript. Each other org is a link to the same section there.
#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Response> {
    let section = section_path(query.section.as_deref().unwrap_or_default());
    let (active, pending): (Vec<_>, Vec<_>) =
        memberships::Model::for_user_with_orgs(&ctx.db, member.user.id)
            .await?
            .into_iter()
            .partition(|(m, _)| m.is_active());
    let row = |(m, org): &(memberships::Model, organisations::Model)| {
        serde_json::json!({
            "id": org.id,
            "name": org.name,
            "url": org_path(&org.slug, section),
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
        .add("/orgs", post(create))
        .add("/orgs/new", get(new))
        .add("/orgs/current", get(current))
        .add("/orgs/{id}/switch", post(switch))
}
