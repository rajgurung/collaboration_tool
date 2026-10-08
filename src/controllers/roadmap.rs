use std::collections::HashMap;

use axum::http::HeaderMap;
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    extractors::{current_member::CurrentMember, session::redirect_response},
    models::{
        memberships,
        projects::{self, Progress, ProjectParams, ACCENTS, LANES},
        tasks,
    },
    views::{
        forms::{field_errors, invalid_form},
        layout::{avatar_color, Person},
    },
};

const FORM_ID: &str = "project-form";

#[derive(Debug, Default, Deserialize)]
struct LaneQuery {
    lane: Option<String>,
}

#[derive(Debug, Serialize)]
struct Card {
    id: i64,
    name: String,
    status: String,
    progress: Progress,
    accent: String,
    summary: String,
    owner: Option<String>,
    owner_color: &'static str,
    blocked: usize,
}

fn lane_or_default(lane: Option<&str>) -> &str {
    match lane {
        Some(l) if LANES.contains(&l) => l,
        _ => "now",
    }
}

/// Approved members, for owner pickers and names on cards.
async fn team(ctx: &AppContext, org_id: i64) -> Result<Vec<Person>> {
    Ok(Person::from_team(
        memberships::Model::team(&ctx.db, org_id).await?,
    ))
}

/// Every lane with its cards. Desktop shows them side by side; phones show
/// the chosen `lane` with a switcher.
async fn lane_data(ctx: &AppContext, org_id: i64, lane: &str) -> Result<serde_json::Value> {
    let names: HashMap<i64, String> = team(ctx, org_id)
        .await?
        .into_iter()
        .map(|p| (p.id, p.username))
        .collect();
    let projects = projects::Model::list_for_org(&ctx.db, org_id).await?;
    let all_tasks = tasks::Model::list_for_org(&ctx.db, org_id).await?;
    let lanes: Vec<serde_json::Value> = LANES
        .iter()
        .map(|key| {
            let cards: Vec<Card> = projects
                .iter()
                .filter(|p| p.lane == *key)
                .map(|p| {
                    let owner = p.owner_id.and_then(|id| names.get(&id).cloned());
                    let own_tasks = all_tasks.iter().filter(|t| t.project_id == p.id);
                    Card {
                        id: p.id,
                        name: p.name.clone(),
                        status: p.status.clone(),
                        progress: Progress::of(p.id, &all_tasks),
                        accent: p.accent.clone(),
                        summary: p.summary.clone(),
                        owner_color: owner.as_deref().map_or("#e8dfce", avatar_color),
                        owner,
                        blocked: own_tasks.filter(|t| t.status == "blocked").count(),
                    }
                })
                .collect();
            let (label, hint) = match *key {
                "now" => ("Now", "What the team is building right now"),
                "next" => ("Next", "Starting once the current work lands"),
                _ => ("Later", "Ideas we are keeping for later"),
            };
            serde_json::json!({
                "key": key,
                "label": label,
                "hint": hint,
                "count": cards.len(),
                "current": *key == lane,
                "cards": cards,
            })
        })
        .collect();
    Ok(serde_json::json!({ "lane": lane, "lanes": lanes }))
}

#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Query(query): Query<LaneQuery>,
) -> Result<Response> {
    let lane = lane_or_default(query.lane.as_deref());
    let data = lane_data(&ctx, member.org.id, lane).await?;
    format::render().view(&v, "roadmap/index.html", member.page("roadmap", data))
}

/// The lane on its own, for HTMX refreshes after a project changes.
#[debug_handler]
async fn list_partial(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Query(query): Query<LaneQuery>,
) -> Result<Response> {
    let lane = lane_or_default(query.lane.as_deref());
    let data = lane_data(&ctx, member.org.id, lane).await?;
    format::render().view(&v, "roadmap/_lane.html", data)
}

/// The values a form starts with, or shows back after a failed submit.
fn form_values(
    params: Option<&ProjectParams>,
    project: Option<&projects::Model>,
) -> serde_json::Value {
    match (params, project) {
        (Some(p), _) => serde_json::json!({
            "name": p.name, "lane": p.lane, "status": p.status,
            "accent": p.accent, "owner_id": p.owner_id, "summary": p.summary,
        }),
        (None, Some(p)) => serde_json::json!({
            "name": p.name, "lane": p.lane, "status": p.status,
            "accent": p.accent, "owner_id": p.owner_id.map(|id| id.to_string()).unwrap_or_default(),
            "summary": p.summary,
        }),
        (None, None) => serde_json::json!({
            "name": "", "lane": "now", "status": "Planned",
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
    if let Ok(project) = &result {
        super::notifications::project_saved(&ctx, &member, None, project).await?;
    }
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
    let owner_before = project.owner_id;
    let result = project.update_from(&ctx.db, &params).await;
    if let Ok(project) = &result {
        super::notifications::project_saved(&ctx, &member, owner_before, project).await?;
    }
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

/// Success closes the dialog and refreshes the lane (or redirects without
/// HTMX); validation errors re-render the form in place.
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
            let trigger = serde_json::json!({ "toast": { "kind": "success", "message": message }, "roadmap-changed": true });
            format::render()
                .header("HX-Trigger", trigger.to_string())
                .header("HX-Reswap", "none")
                .empty()
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
        .add("/roadmap/list", get(list_partial))
        .add("/roadmap/projects/new", get(new))
        .add("/roadmap/projects", post(create))
        .add("/roadmap/projects/{id}/edit", get(edit))
        .add("/roadmap/projects/{id}", post(update))
}
