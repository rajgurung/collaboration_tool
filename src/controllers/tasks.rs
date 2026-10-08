use std::collections::HashMap;

use axum::http::HeaderMap;
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    extractors::{current_member::CurrentMember, session::redirect_response},
    models::{
        memberships, projects,
        task_notes::{self, NoteParams},
        tasks::{self, TaskParams, PRIORITIES, STATUSES},
    },
    views::{
        forms::{field_errors, invalid_form, FieldErrors},
        layout::{avatar_color, Person},
    },
};

const TASK_FORM_ID: &str = "task-form";
const NOTE_FORM_ID: &str = "note-form";
const FILTERS: [(&str, &str); 4] = [
    ("mine", "Mine"),
    ("all", "All"),
    ("blocked", "Blocked"),
    ("done", "Done"),
];

#[derive(Debug, Default, Deserialize)]
struct ListQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    filter: Option<String>,
    #[serde(default)]
    new: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StatusForm {
    status: String,
}

#[derive(Debug, Serialize)]
struct Row {
    id: i64,
    title: String,
    status: String,
    project: String,
    meta: String,
    owner: Option<String>,
    owner_color: &'static str,
    notes: usize,
}

#[derive(Debug, Serialize)]
struct Group {
    label: &'static str,
    rows: Vec<Row>,
}

fn due_label(date: Option<chrono::NaiveDate>) -> String {
    date.map_or_else(String::new, |d| format!("Due {}", d.format("%a %-d %b")))
}

/// The task list for one filter and search, grouped by status, plus filter counts.
async fn list_data(
    ctx: &AppContext,
    member: &CurrentMember,
    filter: &str,
    q: &str,
) -> Result<serde_json::Value> {
    let org_id = member.org.id;
    let me = member.user.id;
    let names: HashMap<i64, String> = memberships::Model::team(&ctx.db, org_id)
        .await?
        .into_iter()
        .collect();
    let project_names: HashMap<i64, String> = projects::Model::list_for_org(&ctx.db, org_id)
        .await?
        .into_iter()
        .map(|p| (p.id, p.name))
        .collect();
    let counts = task_notes::Model::counts_for_org(&ctx.db, org_id).await?;
    let all = tasks::Model::list_for_org(&ctx.db, org_id).await?;
    let needle = q.trim().to_lowercase();

    let matches = |t: &tasks::Model, f: &str| match f {
        "mine" => t.owner_id == Some(me) && t.status != "done",
        "blocked" => t.status == "blocked",
        "done" => t.status == "done",
        _ => t.status != "done",
    };
    let filters: Vec<serde_json::Value> = FILTERS
        .iter()
        .map(|(key, label)| {
            serde_json::json!({
                "key": key,
                "label": label,
                "count": all.iter().filter(|t| matches(t, key)).count(),
                "current": *key == filter,
            })
        })
        .collect();

    let rows: Vec<Row> = all
        .iter()
        .filter(|t| matches(t, filter))
        .map(|t| {
            let owner = t.owner_id.and_then(|id| names.get(&id).cloned());
            let project = project_names
                .get(&t.project_id)
                .cloned()
                .unwrap_or_default();
            let due = due_label(t.due_on);
            Row {
                id: t.id,
                title: t.title.clone(),
                status: t.status.clone(),
                meta: [due.as_str(), project.as_str()]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(" · "),
                project,
                owner_color: owner.as_deref().map_or("#e8dfce", avatar_color),
                owner,
                notes: counts.get(&t.id).copied().unwrap_or(0),
            }
        })
        .filter(|r| {
            needle.is_empty()
                || r.title.to_lowercase().contains(&needle)
                || r.project.to_lowercase().contains(&needle)
                || r.owner
                    .as_deref()
                    .is_some_and(|o| o.to_lowercase().contains(&needle))
        })
        .collect();

    let order = [
        ("blocked", "Blocked"),
        ("progress", "In progress"),
        ("todo", "To do"),
        ("done", "Done"),
    ];
    let mut groups: Vec<Group> = order
        .iter()
        .map(|(_, label)| Group {
            label,
            rows: Vec::new(),
        })
        .collect();
    for row in rows {
        if let Some(i) = order.iter().position(|(key, _)| *key == row.status) {
            groups[i].rows.push(row);
        }
    }
    groups.retain(|g| !g.rows.is_empty());
    Ok(serde_json::json!({ "filters": filters, "filter": filter, "groups": groups, "q": q }))
}

fn filter_or_default(filter: Option<&str>) -> &str {
    match filter {
        Some(f) if FILTERS.iter().any(|(key, _)| *key == f) => f,
        _ => "mine",
    }
}

#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Query(query): Query<ListQuery>,
) -> Result<Response> {
    let filter = filter_or_default(query.filter.as_deref());
    let mut data = list_data(&ctx, &member, filter, &query.q).await?;
    data["open_new"] = serde_json::json!(query.new.is_some());
    format::render().view(&v, "tasks/index.html", member.page("tasks", data))
}

/// The list on its own, for HTMX refreshes (filters, search, after changes).
#[debug_handler]
async fn list_partial(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Query(query): Query<ListQuery>,
) -> Result<Response> {
    let filter = filter_or_default(query.filter.as_deref());
    let data = list_data(&ctx, &member, filter, &query.q).await?;
    format::render().view(&v, "tasks/_list.html", data)
}

async fn task_form_data(
    ctx: &AppContext,
    member: &CurrentMember,
    values: serde_json::Value,
    errors: &FieldErrors,
) -> Result<serde_json::Value> {
    let team = Person::from_team(memberships::Model::team(&ctx.db, member.org.id).await?);
    let projects: Vec<serde_json::Value> = projects::Model::list_for_org(&ctx.db, member.org.id)
        .await?
        .into_iter()
        .map(|p| serde_json::json!({ "id": p.id, "name": p.name }))
        .collect();
    Ok(serde_json::json!({
        "form": values,
        "errors": errors,
        "team": team,
        "projects": projects,
        "priorities": PRIORITIES,
    }))
}

#[debug_handler]
async fn new(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let values = serde_json::json!({
        "title": "", "project_id": "", "owner_id": member.user.id.to_string(), "priority": "medium", "due_on": "",
    });
    let data = task_form_data(&ctx, &member, values, &FieldErrors::new()).await?;
    format::render().view(&v, "tasks/_form.html", data)
}

#[debug_handler]
async fn create(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    Form(params): Form<TaskParams>,
) -> Result<Response> {
    match tasks::Model::create(&ctx.db, member.org.id, &params).await {
        Ok(_) if !headers.contains_key("hx-request") => Ok(redirect_response(&headers, "/tasks")),
        Ok(_) => {
            let trigger = serde_json::json!({
                "toast": { "kind": "success", "message": "Task added" },
                "tasks-changed": true,
            });
            format::render()
                .header("HX-Trigger", trigger.to_string())
                .header("HX-Reswap", "none")
                .empty()
        }
        Err(err) => {
            let errors = field_errors(&err).ok_or(err)?;
            let values = serde_json::json!({
                "title": params.title, "project_id": params.project_id, "owner_id": params.owner_id,
                "priority": params.priority, "due_on": params.due_on,
            });
            let data = task_form_data(&ctx, &member, values, &errors).await?;
            invalid_form(&v, "tasks/_form.html", TASK_FORM_ID, data)
        }
    }
}

/// The sheet for one task: details, status switcher, notes and the note form.
async fn sheet_data(
    ctx: &AppContext,
    member: &CurrentMember,
    task: &tasks::Model,
    note_body: &str,
    errors: &FieldErrors,
) -> Result<serde_json::Value> {
    let team: HashMap<i64, String> = memberships::Model::team(&ctx.db, member.org.id)
        .await?
        .into_iter()
        .collect();
    let project = projects::Model::find_in_org(&ctx.db, member.org.id, task.project_id)
        .await
        .ok();
    let notes: Vec<serde_json::Value> = task_notes::Model::list_for_task(&ctx.db, task)
        .await?
        .into_iter()
        .map(|n| {
            let author = n
                .author_id
                .and_then(|id| team.get(&id).cloned())
                .unwrap_or_else(|| "Former member".to_string());
            serde_json::json!({
                "color": avatar_color(&author),
                "author": author,
                "body": n.body,
                "at": n.created_at.format("%a %-d %b, %H:%M").to_string(),
            })
        })
        .collect();
    let owner = task.owner_id.and_then(|id| team.get(&id).cloned());
    let statuses: Vec<serde_json::Value> = STATUSES
        .iter()
        .map(|(key, label, _)| serde_json::json!({ "key": key, "label": if *key == "progress" { "Doing" } else { label }, "current": *key == task.status }))
        .collect();
    let meta = [
        project.map(|p| p.name).unwrap_or_default(),
        due_label(task.due_on),
        format!("{} priority", capitalise(&task.priority)),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join(" · ");
    Ok(member.page(
        "tasks",
        data!({
            "task": {
                "id": task.id,
                "title": task.title,
                "meta": meta,
                "owner_color": owner.as_deref().map_or("#e8dfce", avatar_color),
                "owner": owner,
            },
            "statuses": statuses,
            "notes": notes,
            "note_body": note_body,
            "errors": errors,
        }),
    ))
}

fn capitalise(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |c| {
        c.to_uppercase().collect::<String>() + chars.as_str()
    })
}

#[debug_handler]
async fn show(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<i64>,
) -> Result<Response> {
    let task = tasks::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    let data = sheet_data(&ctx, &member, &task, "", &FieldErrors::new()).await?;
    format::render().view(&v, "tasks/_sheet.html", data)
}

#[debug_handler]
async fn set_status(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(form): Form<StatusForm>,
) -> Result<Response> {
    let task = tasks::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    if !tasks::is_status(&form.status) {
        return Err(Error::BadRequest("Unknown status.".to_string()));
    }
    let task = task.set_status(&ctx.db, &form.status).await?;
    if !headers.contains_key("hx-request") {
        return Ok(redirect_response(&headers, "/tasks"));
    }
    let data = sheet_data(&ctx, &member, &task, "", &FieldErrors::new()).await?;
    let trigger = serde_json::json!({ "toast": { "kind": "success", "message": "Status updated" }, "tasks-changed": true });
    format::render()
        .header("HX-Trigger", trigger.to_string())
        .view(&v, "tasks/_sheet.html", data)
}

#[debug_handler]
async fn add_note(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(params): Form<NoteParams>,
) -> Result<Response> {
    let task = tasks::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    match task_notes::Model::create(&ctx.db, &task, member.user.id, &params).await {
        Ok(_) if !headers.contains_key("hx-request") => Ok(redirect_response(&headers, "/tasks")),
        Ok(_) => {
            let data = sheet_data(&ctx, &member, &task, "", &FieldErrors::new()).await?;
            let trigger = serde_json::json!({ "toast": { "kind": "success", "message": "Note posted" }, "tasks-changed": true });
            format::render()
                .header("HX-Trigger", trigger.to_string())
                .view(&v, "tasks/_sheet.html", data)
        }
        Err(err) => {
            let errors = field_errors(&err).ok_or(err)?;
            let data = sheet_data(&ctx, &member, &task, &params.body, &errors).await?;
            invalid_form(&v, "tasks/_note_form.html", NOTE_FORM_ID, data)
        }
    }
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/tasks", get(index))
        .add("/tasks", post(create))
        .add("/tasks/list", get(list_partial))
        .add("/tasks/new", get(new))
        .add("/tasks/{id}", get(show))
        .add("/tasks/{id}/status", post(set_status))
        .add("/tasks/{id}/notes", post(add_note))
}
