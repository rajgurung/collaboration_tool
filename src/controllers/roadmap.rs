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
        order::{self, BUSY},
        project_owners,
        projects::{
            self, CloseScope, CloseSummary, MoveRefusal, OwnerRights, Progress, ProjectParams,
            ACCENTS, DONE, DONE_SHOWN, LANES,
        },
        task_assignees, tasks,
    },
    views::{
        forms::{field_errors, invalid_form},
        layout::Person,
        time,
    },
};

/// The sheet's body: the form, and the "Mark project done" confirm step
/// beside it. A refused save re-renders both.
const FORM_ID: &str = "project-sheet";

const ADD_REFUSAL: &str = "Only admins can add projects.";
const MOVE_REFUSAL: &str = "Only admins can move projects between lanes.";

#[derive(Debug, Default, Deserialize)]
struct LaneQuery {
    lane: Option<String>,
    /// Any value shows every done project, not just the newest few.
    all: Option<String>,
    /// Any value opens the page in Reorder mode, after a phone lane switch.
    reorder: Option<String>,
}

/// A closed project's card in the Done column.
#[derive(Debug, Serialize)]
struct DoneCard {
    id: i64,
    name: String,
    accent: String,
    summary: String,
    owners: Owners,
    /// "25 Oct" this year, "12 Dec 2025" before.
    closed_on: String,
    /// Who marked it done, while they are on the team.
    closed_by: Option<String>,
    done_count: usize,
    /// Open tasks: left open by the close, or set back since.
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
    owners: Owners,
    blocked: usize,
    /// Dropping it on Done opens the close confirm only for those allowed.
    can_close: bool,
    /// The edit pencil shows only for those allowed ([`can_edit`]).
    can_edit: bool,
}

/// A card's owner stack: up to three avatars, then "+N".
#[derive(Debug, Serialize, PartialEq, Eq)]
struct Owners {
    shown: Vec<Person>,
    more: usize,
    /// "Owners: Tom, Ana, Leo and Maya", "Owner: Leo", or empty.
    label: String,
}

/// The owners on the team, in the order they were added. A removed member
/// left in the list is skipped.
fn owners_of(ids: &[i64], team: &[Person]) -> Owners {
    let people: Vec<&Person> = ids
        .iter()
        .filter_map(|id| team.iter().find(|p| p.id == *id))
        .collect();
    let names: Vec<&str> = people.iter().map(|p| p.username.as_str()).collect();
    let label = match names.as_slice() {
        [] => String::new(),
        [one] => format!("Owner: {one}"),
        [init @ .., last] => format!("Owners: {} and {last}", init.join(", ")),
    };
    Owners {
        shown: people.iter().take(3).map(|p| (*p).clone()).collect(),
        more: people.len().saturating_sub(3),
        label,
    }
}

/// Who may change a project's owners in an edit; see [`OwnerRights`].
fn owner_rights(member: &CurrentMember) -> OwnerRights {
    if member.acting {
        OwnerRights::No
    } else if member.can_manage() {
        OwnerRights::Any
    } else {
        OwnerRights::IfOwner(member.user.id)
    }
}

fn lane_or_default(lane: Option<&str>) -> &str {
    match lane {
        Some(l) if LANES.contains(&l) || l == DONE => l,
        _ => "now",
    }
}

/// Who may open a project's edit sheet and save it: owners, admins and the
/// project's owners (`owners`). A project owner who isn't a manager can't
/// change its lane ([`projects::Model::update_from`]).
fn can_edit(member: &CurrentMember, owners: &[i64]) -> bool {
    member.can_manage() || owners.contains(&member.user.id)
}

/// The 403 for an edit by someone [`can_edit`] refuses.
fn not_editable() -> Error {
    forbidden("Only admins and this project's owners can change it.")
}

/// Adding, deleting, reordering and changing lanes are for owners and
/// admins: they plan the roadmap.
fn require_planner(member: &CurrentMember, refusal: &str) -> Result<()> {
    if member.can_manage() {
        Ok(())
    } else {
        Err(forbidden(refusal))
    }
}

/// Owners, admins and the project's owners (`owners`), through their own
/// membership. A close changes other people's tasks and notifies them in
/// this person's name, so a super admin acting here can't.
fn can_close(member: &CurrentMember, owners: &[i64]) -> bool {
    !member.acting && projects::Model::closable_by(member.can_manage(), member.user.id, owners)
}

/// Whose tasks this person's close marks done, from their own membership:
/// owners and admins every open task, a project owner only their own and
/// unassigned ones. Only for someone [`can_close`] allows.
fn close_scope(member: &CurrentMember) -> CloseScope {
    if member.can_manage() {
        CloseScope::Everything
    } else {
        CloseScope::OwnWork(member.user.id)
    }
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
    let people = team(ctx, org_id).await?;
    let names: HashMap<i64, String> = people.iter().map(|p| (p.id, p.username.clone())).collect();
    let owners = project_owners::Model::by_project(&ctx.db, org_id).await?;
    let owners_for = |id: i64| owners.get(&id).map_or(&[][..], Vec::as_slice);
    let projects = projects::Model::list_for_org(&ctx.db, org_id).await?;
    let all_tasks = tasks::Model::list_for_org(&ctx.db, org_id).await?;
    let mut lanes: Vec<serde_json::Value> = LANES
        .iter()
        .map(|key| {
            let cards: Vec<Card> = projects
                .iter()
                .filter(|p| p.in_lane(key))
                .map(|p| {
                    let own_tasks = all_tasks.iter().filter(|t| t.project_id == Some(p.id));
                    Card {
                        id: p.id,
                        name: p.name.clone(),
                        status: p.status.clone(),
                        progress: Progress::of(p.id, &all_tasks),
                        accent: p.accent.clone(),
                        summary: p.summary.clone(),
                        owners: owners_of(owners_for(p.id), &people),
                        blocked: own_tasks.filter(|t| t.status == "blocked").count(),
                        can_close: can_close(member, owners_for(p.id)),
                        can_edit: can_edit(member, owners_for(p.id)),
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
                owners: owners_of(owners_for(p.id), &people),
                closed_on,
                closed_by: p.closed_by_id.and_then(|id| names.get(&id).cloned()),
                done_count: summary.done,
                open_count: summary.open(),
                can_reopen: can_close(member, owners_for(p.id)),
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
        "shown": DONE_SHOWN,
        "show_all": show_all,
    }));
    Ok(serde_json::json!({
        "lane": lane,
        "lanes": lanes,
        "show_all": show_all,
        // "+ Project", Reorder and card drags are for those who plan.
        "can_plan": member.can_manage(),
    }))
}

#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Query(query): Query<LaneQuery>,
) -> Result<Response> {
    let lane = lane_or_default(query.lane.as_deref());
    let mut data = lane_data(&ctx, &member, lane, query.all.is_some()).await?;
    data["reordering"] = serde_json::json!(query.reorder.is_some() && member.can_manage());
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
/// `owners` are the project's, for an edit.
fn form_values(
    params: Option<&ProjectParams>,
    project: Option<(&projects::Model, &[i64])>,
) -> serde_json::Value {
    match (params, project) {
        (Some(p), _) => serde_json::json!({
            "name": p.name, "lane": p.lane, "status": p.status,
            "accent": p.accent, "owner_ids": p.owner_ids,
            "summary": p.summary,
        }),
        (None, Some((p, owners))) => serde_json::json!({
            "name": p.name, "lane": p.lane, "status": p.status,
            "accent": p.accent, "owner_ids": owners, "summary": p.summary,
        }),
        (None, None) => serde_json::json!({
            "name": "", "lane": "now", "status": "Planned",
            "accent": ACCENTS[0], "owner_ids": [], "summary": "",
        }),
    }
}

/// What the "Mark project done" box and its confirm step say: the counts
/// for this person's close ([`close_scope`]), who is on the tasks it leaves
/// open, and who would get a notice (every open task's assignees, those it
/// closes and those it leaves, and the project's owners, on the team, not
/// the viewer).
async fn close_data(
    ctx: &AppContext,
    member: &CurrentMember,
    project: &projects::Model,
    owners: &[i64],
) -> Result<serde_json::Value> {
    let org_id = member.org.id;
    let all_tasks = tasks::Model::list_for_org(&ctx.db, org_id).await?;
    let assignees = task_assignees::Model::by_task(&ctx.db, org_id).await?;
    let scope = close_scope(member);
    let summary = CloseSummary::for_close(project.id, &all_tasks, &assignees, scope);
    let on = |t: &tasks::Model| assignees.get(&t.id).map_or(&[][..], Vec::as_slice);
    let open: Vec<&tasks::Model> = all_tasks
        .iter()
        .filter(|t| t.project_id == Some(project.id) && t.status != "done")
        .collect();
    // People on the tasks left open, in the order first met.
    let mut keepers: Vec<i64> = Vec::new();
    for id in open
        .iter()
        .filter(|t| !scope.closes(on(t)))
        .flat_map(|t| on(t))
    {
        if *id != member.user.id && !keepers.contains(id) {
            keepers.push(*id);
        }
    }
    let mut told: Vec<i64> = open.iter().flat_map(|t| on(t)).copied().collect();
    told.extend(owners);
    let team = memberships::Model::team(&ctx.db, org_id).await?;
    let name_of = |id: &i64| team.iter().find(|(t, _)| t == id).map(|(_, n)| n.clone());
    let kept_names: Vec<String> = keepers.iter().filter_map(name_of).collect();
    let who = projects::names(&kept_names, keepers.len() - kept_names.len());
    let names: Vec<String> = team
        .iter()
        .filter(|(id, _)| *id != member.user.id && told.contains(id))
        .map(|(_, name)| name.clone())
        .collect();
    Ok(serde_json::json!({
        "sentence": summary.sentence(&who),
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
    // A new project's maker picks its owners, unless a super admin acting.
    let mut owners_locked = member.acting;
    // Only owners and admins move a project between lanes.
    let mut lane_locked = false;
    // For the edit form's delete confirm and "Made with Claude" line. Only a
    // bool about Claude reaches the template, never the grant.
    let project = match project_id {
        Some(id) => {
            let project = projects::Model::find_in_org(&ctx.db, member.org.id, id).await?;
            let owners = project_owners::Model::for_project(&ctx.db, member.org.id, id).await?;
            owners_locked = !owner_rights(member).allows(&owners);
            lane_locked = !member.can_manage();
            let close = if can_close(member, &owners) {
                Some(close_data(ctx, member, &project, &owners).await?)
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
            "owners_locked": owners_locked,
            "lane_locked": lane_locked,
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
    require_planner(&member, ADD_REFUSAL)?;
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
    let owners = project_owners::Model::for_project(&ctx.db, member.org.id, id).await?;
    if !can_edit(&member, &owners) {
        return Err(not_editable());
    }
    let values = form_values(None, Some((&project, &owners)));
    let data = form_data(&ctx, &member, Some(id), values, &Default::default()).await?;
    format::render().view(&v, "roadmap/_form.html", member.fragment(data))
}

#[debug_handler]
async fn create(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    axum_extra::extract::Form(mut params): axum_extra::extract::Form<ProjectParams>,
) -> Result<Response> {
    require_planner(&member, ADD_REFUSAL)?;
    // A super admin acting here makes the project, but not its owners, as
    // with an edit.
    if member.acting {
        params.owner_ids.clear();
    }
    let result = projects::Model::create(&ctx.db, member.org.id, &params, None).await;
    if let Ok(project) = &result {
        let owners = project_owners::Model::for_project(&ctx.db, member.org.id, project.id).await?;
        super::notifications::project_saved(&ctx, &member, &[], project, &owners).await?;
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
    axum_extra::extract::Form(params): axum_extra::extract::Form<ProjectParams>,
) -> Result<Response> {
    let project = projects::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    // Only a sheet opened before a close gets here. A close that commits
    // after this read lets the save through, onto a lane it keeps for a
    // reopen, so nothing shows wrongly.
    if project.is_closed() {
        return closed_refused(&project);
    }
    // As for a close, checked before the save. The save locks the row and
    // reads the owners again for the owner list; the lane stays a manager's.
    let owners = project_owners::Model::for_project(&ctx.db, member.org.id, id).await?;
    if !can_edit(&member, &owners) {
        return Err(not_editable());
    }
    let result = project
        .update_from(&ctx.db, &params, owner_rights(&member), member.can_manage())
        .await;
    if let Ok(saved) = &result {
        super::notifications::project_saved(
            &ctx,
            &member,
            &saved.owners_before,
            &saved.project,
            &saved.owners,
        )
        .await?;
    }
    let result = result.map(|saved| saved.project);
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

/// A drop or an arrow on the roadmap: the lane, and the visible card it
/// lands before (or after).
#[derive(Debug, Deserialize)]
struct MoveForm {
    lane: String,
    #[serde(default)]
    before: Option<i64>,
    #[serde(default)]
    after: Option<i64>,
}

/// Puts a project at a spot in a lane. Owners and admins only. No toast and
/// no notices: a lane change never sent any.
#[debug_handler]
async fn move_project(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(form): Form<MoveForm>,
) -> Result<Response> {
    require_planner(&member, MOVE_REFUSAL)?;
    let place = super::tasks::move_place(form.before, form.after)?;
    let project =
        match projects::Model::move_to(&ctx.db, member.org.id, id, &form.lane, place).await {
            Ok(Ok(project)) => project,
            Ok(Err(MoveRefusal::Stale)) => return super::tasks::move_refused(None),
            Ok(Err(MoveRefusal::Closed(project))) => return closed_refused(&project),
            Err(err) => return super::tasks::move_failed(err),
        };
    if !headers.contains_key("hx-request") {
        let to = member.url(&format!("/roadmap?lane={}", project.lane));
        return Ok(redirect_response(&headers, &to));
    }
    format::render().header("HX-Reswap", "none").empty()
}

#[derive(Debug, Deserialize)]
struct LaneForm {
    #[serde(rename = "lane")]
    to: String,
}

/// Moves a project to another lane, from a drag on the roadmap. Owners and
/// admins only; Done is not a lane, so a
/// drop there goes through the close confirm instead.
#[debug_handler]
async fn move_lane(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(form): Form<LaneForm>,
) -> Result<Response> {
    require_planner(&member, MOVE_REFUSAL)?;
    if !LANES.contains(&form.to.as_str()) {
        return Err(Error::BadRequest("Choose now, next or later.".to_string()));
    }
    let project = projects::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    if project.is_closed() {
        return closed_refused(&project);
    }
    // Closed is refused above, so this is only "already in that lane".
    let unchanged = project.in_lane(&form.to);
    let project = if unchanged {
        project
    } else {
        project.move_to_lane(&ctx.db, &form.to).await?
    };
    if !headers.contains_key("hx-request") {
        let to = member.url(&format!("/roadmap?lane={}", project.lane));
        return Ok(redirect_response(&headers, &to));
    }
    let mut trigger = serde_json::json!({ "roadmap-changed": true });
    if !unchanged {
        trigger["toast"] = serde_json::json!({
            "kind": "success",
            "message": format!("{} moved to {}.", project.name, projects::lane_label(&project.lane)),
        });
    }
    format::render()
        .header("HX-Trigger", trigger.to_string())
        .header("HX-Reswap", "none")
        .empty()
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
    let removed = match projects::Model::remove(&ctx.db, member.org.id, id).await {
        Ok(removed) => removed,
        Err(err) => return busy_refused(err, "delete"),
    };
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

/// The check before a close or reopen: in this organisation (else 404), for
/// someone allowed (else 403). The model checks the owners again once the
/// project is locked.
async fn closable(ctx: &AppContext, member: &CurrentMember, id: i64) -> Result<()> {
    member.require_own_membership()?;
    projects::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    let owners = project_owners::Model::for_project(&ctx.db, member.org.id, id).await?;
    if !can_close(member, &owners) {
        return Err(not_closable());
    }
    Ok(())
}

fn not_closable() -> Error {
    forbidden("Only this project's owners and admins can do that.")
}

/// A close, reopen or delete that waited too long behind a card move: 409
/// with a "try again" toast, as a move gets. Other errors pass through.
fn busy_refused(err: ModelError, what: &str) -> Result<Response> {
    if order::is_busy(&err) {
        tracing::warn!(error = %err, "a project {what} waited too long for a card move");
        return super::tasks::move_refused(Some(BUSY));
    }
    Err(err.into())
}

/// Marks a project done with its open tasks. Owners, admins and the
/// project's owners; an owner who isn't a manager closes only their own and
/// unassigned tasks ([`close_scope`]). A second close is a no-op.
#[debug_handler]
async fn close(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response> {
    closable(&ctx, &member, id).await?;
    let closed = match projects::Model::close(
        &ctx.db,
        member.org.id,
        id,
        member.user.id,
        close_scope(&member),
    )
    .await
    {
        Ok(Some(closed)) => closed,
        Ok(None) => return Err(not_closable()),
        Err(err) => return busy_refused(err, "close"),
    };
    if !closed.already {
        // The close is already committed, so a failed notice must not turn
        // the reply into "nothing was changed".
        if let Err(err) = super::notifications::project_closed(
            &ctx,
            &member,
            &closed.project,
            &closed.tasks,
            &closed.left_open,
        )
        .await
        {
            tracing::error!(error = %err, project_id = id, "could not send the closed notices");
        }
    }
    let name = &closed.project.name;
    let closed_ids: Vec<i64> = closed.tasks.iter().map(|t| t.id).collect();
    let kept_ids: Vec<i64> = closed.left_open.iter().map(|t| t.id).collect();
    tracing::info!(
        user_id = member.user.id,
        org_id = member.org.id,
        project_id = id,
        tasks = closed.tasks.len(),
        left_open = closed.left_open.len(),
        ?closed_ids,
        ?kept_ids,
        already = closed.already,
        acting = member.acting,
        "marked a project done"
    );
    // The number really changed, which can be fewer than the confirm said.
    let message = match (closed.already, closed.tasks.len(), closed.left_open.len()) {
        (true, _, _) => format!("{name} is already done."),
        (false, 0, 0) => format!("{name} done."),
        (false, 1, 0) => format!("{name} done. 1 task closed."),
        (false, n, 0) => format!("{name} done. {n} tasks closed."),
        (false, 0, 1) => format!("{name} done. 1 task left open."),
        (false, 0, k) => format!("{name} done. {k} tasks left open."),
        (false, 1, k) => format!("{name} done. 1 task closed, {k} left open."),
        (false, n, k) => format!("{name} done. {n} tasks closed, {k} left open."),
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
    let reopened = match projects::Model::reopen(
        &ctx.db,
        member.org.id,
        id,
        member.user.id,
        member.can_manage(),
    )
    .await
    {
        Ok(Some(reopened)) => reopened,
        Ok(None) => return Err(not_closable()),
        Err(err) => return busy_refused(err, "reopen"),
    };
    let project = &reopened.project;
    if !reopened.already {
        // The reopen is already committed, as with a close.
        if let Err(err) =
            super::notifications::project_reopened(&ctx, &member, project, &reopened.tasks).await
        {
            tracing::error!(error = %err, project_id = id, "could not send the reopened notices");
        }
    }
    tracing::info!(
        user_id = member.user.id,
        org_id = member.org.id,
        project_id = id,
        tasks = reopened.tasks.len(),
        already = reopened.already,
        acting = member.acting,
        "reopened a project"
    );
    let back = format!(
        "{} is back in {}.",
        project.name,
        projects::lane_label(&project.lane)
    );
    let message = match (reopened.already, reopened.tasks.len()) {
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
            let mut values = form_values(Some(params), None);
            // The owners were ignored, not saved: show the project's own.
            if let Some(id) = project_id {
                let owners = project_owners::Model::for_project(&ctx.db, member.org.id, id).await?;
                if !owner_rights(member).allows(&owners) {
                    values["owner_ids"] = serde_json::json!(owners);
                }
            }
            let data = form_data(ctx, member, project_id, values, &errors).await?;
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
        .add("/roadmap/projects/{id}/lane", post(move_lane))
        .add("/roadmap/projects/{id}/move", post(move_project))
        .add("/roadmap/projects/{id}/close", post(close))
        .add("/roadmap/projects/{id}/reopen", post(reopen))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn team() -> Vec<Person> {
        Person::from_team(
            [(1, "tom"), (2, "ana"), (3, "leo"), (4, "maya")]
                .iter()
                .map(|(id, name)| (*id, (*name).to_string()))
                .collect(),
        )
    }

    fn names(owners: &Owners) -> Vec<&str> {
        owners.shown.iter().map(|p| p.username.as_str()).collect()
    }

    #[test]
    fn owners_of_shows_three_then_a_count() {
        let team = team();
        let none = owners_of(&[], &team);
        assert!(none.shown.is_empty());
        assert_eq!((none.more, none.label.as_str()), (0, ""));

        let one = owners_of(&[3], &team);
        assert_eq!(names(&one), ["leo"]);
        assert_eq!(one.label, "Owner: leo");

        let two = owners_of(&[2, 3], &team);
        assert_eq!(two.label, "Owners: ana and leo");

        let three = owners_of(&[1, 2, 3], &team);
        assert_eq!((names(&three).len(), three.more), (3, 0));

        let four = owners_of(&[1, 2, 3, 4], &team);
        assert_eq!(names(&four), ["tom", "ana", "leo"]);
        assert_eq!(four.more, 1);
        assert_eq!(four.label, "Owners: tom, ana, leo and maya");

        // 9 was removed from the team: skipped everywhere.
        let removed = owners_of(&[9, 3], &team);
        assert_eq!(names(&removed), ["leo"]);
        assert_eq!(removed.label, "Owner: leo");
    }
}
