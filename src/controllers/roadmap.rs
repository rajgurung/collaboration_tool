use std::collections::HashMap;

use axum::http::HeaderMap;
use loco_rs::prelude::*;
use serde::Serialize;

use crate::{
    extractors::{current_member::CurrentMember, session::redirect_response},
    models::{
        memberships,
        projects::{self, ProjectParams, ACCENTS, LANES},
    },
    views::{
        forms::{field_errors, invalid_form, toast},
        layout::avatar_color,
    },
};

const FORM_ID: &str = "project-form";

#[derive(Debug, Serialize)]
struct Person {
    id: i64,
    username: String,
    color: &'static str,
}

#[derive(Debug, Serialize)]
struct Card {
    id: i64,
    name: String,
    status: String,
    progress: i64,
    accent: String,
    summary: String,
    owner: Option<String>,
    owner_color: &'static str,
}

#[derive(Debug, Serialize)]
struct Lane {
    key: &'static str,
    number: usize,
    subtitle: &'static str,
    cards: Vec<Card>,
}

/// Approved members, for owner pickers and names on cards.
async fn team(ctx: &AppContext, org_id: i64) -> Result<Vec<Person>> {
    Ok(memberships::Model::list_for_org(&ctx.db, org_id)
        .await?
        .into_iter()
        .filter(|(m, _)| m.is_active())
        .map(|(m, u)| Person {
            id: u.id,
            color: avatar_color(&m.username),
            username: m.username,
        })
        .collect())
}

async fn lanes(ctx: &AppContext, org_id: i64) -> Result<Vec<Lane>> {
    let names: HashMap<i64, String> = team(ctx, org_id)
        .await?
        .into_iter()
        .map(|p| (p.id, p.username))
        .collect();
    let projects = projects::Model::list_for_org(&ctx.db, org_id).await?;
    Ok(LANES
        .iter()
        .enumerate()
        .map(|(i, &key)| Lane {
            key,
            number: i + 1,
            subtitle: match key {
                "now" => "In progress now",
                "next" => "Coming next",
                _ => "Ideas for later",
            },
            cards: projects
                .iter()
                .filter(|p| p.lane == key)
                .map(|p| {
                    let owner = p.owner_id.and_then(|id| names.get(&id).cloned());
                    Card {
                        id: p.id,
                        name: p.name.clone(),
                        status: p.status.clone(),
                        progress: p.progress,
                        accent: p.accent.clone(),
                        summary: p.summary.clone(),
                        owner_color: owner.as_deref().map_or("#e8dfce", avatar_color),
                        owner,
                    }
                })
                .collect(),
        })
        .collect())
}

#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let lanes = lanes(&ctx, member.org.id).await?;
    format::render().view(
        &v,
        "roadmap/index.html",
        member.page("roadmap", data!({ "lanes": lanes })),
    )
}

/// The values a form starts with, or shows back after a failed submit.
fn form_values(
    params: Option<&ProjectParams>,
    project: Option<&projects::Model>,
) -> serde_json::Value {
    match (params, project) {
        (Some(p), _) => serde_json::json!({
            "name": p.name, "lane": p.lane, "status": p.status, "progress": p.progress,
            "accent": p.accent, "owner_id": p.owner_id, "summary": p.summary,
        }),
        (None, Some(p)) => serde_json::json!({
            "name": p.name, "lane": p.lane, "status": p.status, "progress": p.progress,
            "accent": p.accent, "owner_id": p.owner_id.map(|id| id.to_string()).unwrap_or_default(),
            "summary": p.summary,
        }),
        (None, None) => serde_json::json!({
            "name": "", "lane": "now", "status": "Planned", "progress": 0,
            "accent": ACCENTS[0], "owner_id": "", "summary": "",
        }),
    }
}

async fn form_data(
    ctx: &AppContext,
    member: &CurrentMember,
    project_id: Option<i64>,
    values: serde_json::Value,
    errors: &crate::views::forms::FieldErrors,
) -> Result<serde_json::Value> {
    Ok(member.page(
        "roadmap",
        data!({
            "project_id": project_id,
            "form": values,
            "errors": errors,
            "team": team(ctx, member.org.id).await?,
            "lanes": LANES,
            "accents": ACCENTS,
        }),
    ))
}

#[debug_handler]
async fn new(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let data = form_data(
        &ctx,
        &member,
        None,
        form_values(None, None),
        &Default::default(),
    )
    .await?;
    format::render().view(&v, "roadmap/_form.html", data)
}

#[debug_handler]
async fn edit(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<i64>,
) -> Result<Response> {
    let project = projects::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    let values = form_values(None, Some(&project));
    let data = form_data(&ctx, &member, Some(id), values, &Default::default()).await?;
    format::render().view(&v, "roadmap/_form.html", data)
}

#[debug_handler]
async fn create(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    Form(params): Form<ProjectParams>,
) -> Result<Response> {
    let result = projects::Model::create(&ctx.db, member.org.id, &params).await;
    saved(
        &ctx,
        &v,
        &member,
        &headers,
        None,
        &params,
        result,
        "Project added",
    )
    .await
}

#[debug_handler]
async fn update(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(params): Form<ProjectParams>,
) -> Result<Response> {
    let project = projects::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    let result = project.update_from(&ctx.db, &params).await;
    saved(
        &ctx,
        &v,
        &member,
        &headers,
        Some(id),
        &params,
        result,
        "Project saved",
    )
    .await
}

/// Success refreshes the lanes (or redirects without HTMX); validation errors
/// re-render the form in place.
#[allow(clippy::too_many_arguments)]
async fn saved(
    ctx: &AppContext,
    v: &TeraView,
    member: &CurrentMember,
    headers: &HeaderMap,
    project_id: Option<i64>,
    params: &ProjectParams,
    result: ModelResult<projects::Model>,
    message: &str,
) -> Result<Response> {
    match result {
        Ok(_) if !headers.contains_key("hx-request") => Ok(redirect_response(headers, "/roadmap")),
        Ok(_) => {
            let lanes = lanes(ctx, member.org.id).await?;
            format::render()
                .header("HX-Trigger", toast("success", message))
                .view(v, "roadmap/_lanes.html", data!({ "lanes": lanes }))
        }
        Err(err) => {
            let errors = field_errors(&err).ok_or(err)?;
            let data = form_data(
                ctx,
                member,
                project_id,
                form_values(Some(params), None),
                &errors,
            )
            .await?;
            invalid_form(v, "roadmap/_form.html", FORM_ID, data)
        }
    }
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/roadmap", get(index))
        .add("/roadmap/projects/new", get(new))
        .add("/roadmap/projects", post(create))
        .add("/roadmap/projects/{id}/edit", get(edit))
        .add("/roadmap/projects/{id}", post(update))
}
