use std::collections::HashMap;

use axum::http::HeaderMap;
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};

use chrono::Datelike;

use crate::{
    data::notify_hub::NotifyHub,
    extractors::{
        current_member::{forbidden, CurrentMember},
        session::redirect_response,
    },
    models::{
        memberships,
        projects::{self, CloseSummary, Progress, ProjectParams, ACCENTS, DONE, DONE_SHOWN, LANES},
        task_assignees, tasks,
    },
    views::{
        forms::{field_errors, invalid_form},
        layout::{avatar_color, Person},
        time,
    },
};

/// The sheet's body: the form, and the "Mark project done" confirm step
/// beside it. A refused save re-renders both.
const FORM_ID: &str = "project-sheet";

#[derive(Debug, Default, Deserialize)]
struct LaneQuery {
    lane: Option<String>,
    /// Any value shows every done project, not just the newest few.
    all: Option<String>,
}

/// A closed project's card in the Done column.
#[derive(Debug, Serialize)]
struct DoneCard {
    id: i64,
    name: String,
    accent: String,
    summary: String,
    owner: Option<String>,
    owner_color: &'static str,
    /// "25 Oct" this year, "12 Dec 2025" before.
    closed_on: String,
    /// Who marked it done, while they are on the team.
    closed_by: Option<String>,
    done_count: usize,
    /// Tasks set back to open since the close.
    open_count: usize,
    can_reopen: bool,
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
        Some(l) if LANES.contains(&l) || l == DONE => l,
        _ => "now",
    }
}

/// Owners, admins and the project's owner, through their own membership. A
/// close changes other people's tasks and notifies them in this person's
/// name, so a super admin acting here can't.
fn can_close(member: &CurrentMember, project: &projects::Model) -> bool {
    !member.acting && project.closable_by(member.can_manage(), member.user.id)
}

/// Approved members, for owner pickers and names on cards.
async fn team(ctx: &AppContext, org_id: i64) -> Result<Vec<Person>> {
    Ok(Person::from_team(
        memberships::Model::team(&ctx.db, org_id).await?,
    ))
}

/// Every lane with its cards, then Done. Desktop shows them side by side;
/// phones show the chosen `lane` with a switcher. Done shows the newest
/// [`DONE_SHOWN`] unless `show_all`.
async fn lane_data(
    ctx: &AppContext,
    member: &CurrentMember,
    lane: &str,
    show_all: bool,
) -> Result<serde_json::Value> {
    let org_id = member.org.id;
    let names: HashMap<i64, String> = team(ctx, org_id)
        .await?
        .into_iter()
        .map(|p| (p.id, p.username))
        .collect();
    let projects = projects::Model::list_for_org(&ctx.db, org_id).await?;
    let all_tasks = tasks::Model::list_for_org(&ctx.db, org_id).await?;
    let mut lanes: Vec<serde_json::Value> = LANES
        .iter()
        .map(|key| {
            let cards: Vec<Card> = projects
                .iter()
                .filter(|p| p.in_lane(key))
                .map(|p| {
                    let owner = p.owner_id.and_then(|id| names.get(&id).cloned());
                    let own_tasks = all_tasks.iter().filter(|t| t.project_id == Some(p.id));
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
            let hint = match *key {
                "now" => "What the team is building right now",
                "next" => "Starting once the current work lands",
                _ => "Ideas we are keeping for later",
            };
            serde_json::json!({
                "key": key,
                "label": projects::lane_label(key),
                "hint": hint,
                "count": cards.len(),
                "current": *key == lane,
                "cards": cards,
            })
        })
        .collect();

    let tz = member.tz();
    let this_year = time::today(tz).year();
    let mut closed: Vec<&projects::Model> = projects.iter().filter(|p| p.is_closed()).collect();
    closed.sort_by_key(|p| std::cmp::Reverse((p.closed_at, p.id)));
    let count = closed.len();
    let shown = if show_all {
        count
    } else {
        count.min(DONE_SHOWN)
    };
    let cards: Vec<DoneCard> = closed[..shown]
        .iter()
        .map(|p| {
            let owner = p.owner_id.and_then(|id| names.get(&id).cloned());
            let summary = CloseSummary::of(p.id, &all_tasks);
            let closed_on = p.closed_at.map_or_else(String::new, |at| {
                let at = time::local(at, tz);
                let format = if at.year() == this_year {
                    "%-d %b"
                } else {
                    "%-d %b %Y"
                };
                at.format(format).to_string()
            });
            DoneCard {
                id: p.id,
                name: p.name.clone(),
                accent: p.accent.clone(),
                summary: p.summary.clone(),
                owner_color: owner.as_deref().map_or("#e8dfce", avatar_color),
                owner,
                closed_on,
                closed_by: p.closed_by_id.and_then(|id| names.get(&id).cloned()),
                done_count: summary.done,
                open_count: summary.open(),
                can_reopen: can_close(member, p),
            }
        })
        .collect();
    lanes.push(serde_json::json!({
        "key": DONE,
        "label": "Done",
        "hint": "Finished projects, newest first",
        "count": count,
        "current": lane == DONE,
        "cards": cards,
        "more": count > DONE_SHOWN,
        "show_all": show_all,
    }));
    Ok(serde_json::json!({ "lane": lane, "lanes": lanes, "show_all": show_all }))
}

#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Query(query): Query<LaneQuery>,
) -> Result<Response> {
    let lane = lane_or_default(query.lane.as_deref());
    let data = lane_data(&ctx, &member, lane, query.all.is_some()).await?;
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
    let data = lane_data(&ctx, &member, lane, query.all.is_some()).await?;
    format::render().view(&v, "roadmap/_lane.html", member.fragment(data))
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

/// What the "Mark project done" box and its confirm step say: the counts,
/// and who would get a notice (the open tasks' assignees and the project's
/// owner, on the team, not the viewer).
async fn close_data(
    ctx: &AppContext,
    member: &CurrentMember,
    project: &projects::Model,
) -> Result<serde_json::Value> {
    let org_id = member.org.id;
    let all_tasks = tasks::Model::list_for_org(&ctx.db, org_id).await?;
    let summary = CloseSummary::of(project.id, &all_tasks);
    let assignees = task_assignees::Model::by_task(&ctx.db, org_id).await?;
    let mut told: Vec<i64> = all_tasks
        .iter()
        .filter(|t| t.project_id == Some(project.id) && t.status != "done")
        .filter_map(|t| assignees.get(&t.id))
        .flatten()
        .copied()
        .collect();
    told.extend(project.owner_id);
    let names: Vec<String> = memberships::Model::team(&ctx.db, org_id)
        .await?
        .into_iter()
        .filter(|(id, _)| *id != member.user.id && told.contains(id))
        .map(|(_, name)| name)
        .collect();
    Ok(serde_json::json!({
        "sentence": summary.sentence(),
        "box_line": summary.box_line(),
        "notice": projects::notice_line(&names),
        "summary": summary,
    }))
}

async fn form_data(
    ctx: &AppContext,
    member: &CurrentMember,
    project_id: Option<i64>,
    values: serde_json::Value,
    errors: &crate::views::forms::FieldErrors,
) -> Result<serde_json::Value> {
    // For the edit form's delete confirm and "Made with Claude" line. Only a
    // bool about Claude reaches the template, never the grant.
    let project = match project_id {
        Some(id) => {
            let project = projects::Model::find_in_org(&ctx.db, member.org.id, id).await?;
            let close = if can_close(member, &project) {
                Some(close_data(ctx, member, &project).await?)
            } else {
                None
            };
            Some(serde_json::json!({
                "name": project.name,
                "task_count": tasks::Model::count_in_project(&ctx.db, member.org.id, id).await?,
                "made_with_claude": project.claude_grant_id.is_some(),
                "close": close,
            }))
        }
        None => None,
    };
    // `page` puts `me.can_manage` in, which shows the delete button.
    Ok(member.page(
        "roadmap",
        data!({
            "project_id": project_id,
            "project": project,
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
    format::render().view(&v, "roadmap/_form.html", member.fragment(data))
}

#[debug_handler]
async fn edit(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<i64>,
) -> Result<Response> {
    let project = projects::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    if project.is_closed() {
        return closed_refused(&project);
    }
    let values = form_values(None, Some(&project));
    let data = form_data(&ctx, &member, Some(id), values, &Default::default()).await?;
    format::render().view(&v, "roadmap/_form.html", member.fragment(data))
}

#[debug_handler]
async fn create(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    Form(params): Form<ProjectParams>,
) -> Result<Response> {
    let result = projects::Model::create(&ctx.db, member.org.id, &params, None).await;
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
    // Only a sheet opened before a close gets here. A close that commits
    // after this read lets the save through, onto a lane it keeps for a
    // reopen, so nothing shows wrongly.
    if project.is_closed() {
        return closed_refused(&project);
    }
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

/// Deletes a project with its tasks, their notes, assignees and reactions,
/// and the notifications about those tasks. Owners and admins, through their
/// own membership. Can't be undone, so it is logged.
#[debug_handler]
async fn destroy(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response> {
    member.require_manager()?;
    member.require_own_membership()?;
    let removed = projects::Model::remove(&ctx.db, member.org.id, id).await?;
    tracing::info!(
        user_id = member.user.id,
        org_id = member.org.id,
        project_id = removed.id,
        tasks = removed.tasks,
        acting = member.acting,
        "deleted a project"
    );
    if let Some(hub) = ctx.shared_store.get::<NotifyHub>() {
        hub.publish(member.org.id, &removed.notified);
    }
    if !headers.contains_key("hx-request") {
        return Ok(redirect_response(&headers, &member.url("/roadmap")));
    }
    let trigger = serde_json::json!({
        "toast": { "kind": "success", "message": "Project deleted" },
        "roadmap-changed": true,
        "close-dialogs": true,
    });
    format::render()
        .header("HX-Trigger", trigger.to_string())
        .header("HX-Reswap", "none")
        .empty()
}

/// The project for a close or reopen: in this organisation (else 404), for
/// someone allowed (else 403).
async fn closable(ctx: &AppContext, member: &CurrentMember, id: i64) -> Result<projects::Model> {
    member.require_own_membership()?;
    let project = projects::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    if !can_close(member, &project) {
        return Err(forbidden(
            "Only owners, admins and the project's owner can do that.",
        ));
    }
    Ok(project)
}

/// Marks a project done with its open tasks. Owners, admins and the
/// project's owner. A second close is a no-op.
#[debug_handler]
async fn close(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response> {
    closable(&ctx, &member, id).await?;
    let closed = projects::Model::close(&ctx.db, member.org.id, id, member.user.id).await?;
    if !closed.already {
        super::notifications::project_closed(&ctx, &member, &closed.project, &closed.tasks).await?;
    }
    let name = &closed.project.name;
    tracing::info!(
        user_id = member.user.id,
        org_id = member.org.id,
        project_id = id,
        tasks = closed.tasks.len(),
        already = closed.already,
        acting = member.acting,
        "marked a project done"
    );
    // The number really changed, which can be fewer than the confirm said.
    let message = match (closed.already, closed.tasks.len()) {
        (true, _) => format!("{name} is already done."),
        (false, 0) => format!("{name} done."),
        (false, 1) => format!("{name} done. 1 task closed."),
        (false, n) => format!("{name} done. {n} tasks closed."),
    };
    let to = member.url("/roadmap?lane=done");
    done_or_reopened(&headers, &to, &message)
}

/// Puts a closed project back in its lane, and its tasks back as they were.
#[debug_handler]
async fn reopen(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response> {
    closable(&ctx, &member, id).await?;
    let reopened = projects::Model::reopen(&ctx.db, member.org.id, id).await?;
    let project = &reopened.project;
    tracing::info!(
        user_id = member.user.id,
        org_id = member.org.id,
        project_id = id,
        tasks = reopened.restored,
        already = reopened.already,
        acting = member.acting,
        "reopened a project"
    );
    let back = format!(
        "{} is back in {}.",
        project.name,
        projects::lane_label(&project.lane)
    );
    let message = match (reopened.already, reopened.restored) {
        (true, _) => format!("{} is already open.", project.name),
        (false, 0) => back,
        (false, 1) => format!("{back} 1 task reopened."),
        (false, n) => format!("{back} {n} tasks reopened."),
    };
    let to = member.url(&format!("/roadmap?lane={}", project.lane));
    done_or_reopened(&headers, &to, &message)
}

/// After a close or reopen: the toast, a lane refresh and the sheet closed,
/// or a redirect to `to` without HTMX.
fn done_or_reopened(headers: &HeaderMap, to: &str, message: &str) -> Result<Response> {
    if !headers.contains_key("hx-request") {
        return Ok(redirect_response(headers, to));
    }
    let trigger = serde_json::json!({
        "toast": { "kind": "success", "message": message },
        "roadmap-changed": true,
        "close-dialogs": true,
    });
    format::render()
        .header("HX-Trigger", trigger.to_string())
        .header("HX-Reswap", "none")
        .empty()
}

/// A closed project can't be edited: reopen it first. Only a sheet or card
/// drawn before the close gets here, so the lane refreshes and the sheet
/// closes. Never an `HX-Redirect`, which would read as a stale tab.
fn closed_refused(project: &projects::Model) -> Result<Response> {
    let trigger = serde_json::json!({
        "toast": { "kind": "error", "message": projects::closed_refusal(&project.name) },
        "roadmap-changed": true,
        "close-dialogs": true,
    });
    format::render()
        .status(409)
        .header("HX-Trigger", trigger.to_string())
        .header("HX-Reswap", "none")
        .empty()
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
        Ok(_) if !headers.contains_key("hx-request") => {
            Ok(redirect_response(headers, &member.url("/roadmap")))
        }
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
            invalid_form(v, "roadmap/_form.html", FORM_ID, member.fragment(data))
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
        .add("/roadmap/projects/{id}/delete", post(destroy))
        .add("/roadmap/projects/{id}/close", post(close))
        .add("/roadmap/projects/{id}/reopen", post(reopen))
}
