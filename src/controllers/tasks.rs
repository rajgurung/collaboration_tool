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
        forms::{field_errors, invalid_form, toast, FieldErrors},
        layout::{avatar_color, Person},
    },
};

const TASK_FORM_ID: &str = "task-form";
const NOTE_FORM_ID: &str = "note-form";

#[derive(Debug, Default, Deserialize)]
struct BoardQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    new: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StatusForm {
    status: String,
    #[serde(default)]
    q: String,
}

#[derive(Debug, Serialize)]
struct Card {
    id: i64,
    title: String,
    priority: String,
    status: String,
    project: String,
    owner: Option<String>,
    owner_color: &'static str,
    due: String,
    notes: usize,
}

#[derive(Debug, Serialize)]
struct Column {
    key: &'static str,
    label: &'static str,
    color: &'static str,
    cards: Vec<Card>,
}

/// Everything the board needs, filtered by the search text (title, owner or project).
struct Board {
    columns: Vec<Column>,
    team: Vec<Person>,
    projects: Vec<projects::Model>,
}

fn due_label(date: Option<chrono::NaiveDate>) -> String {
    date.map_or_else(|| "No date".to_string(), |d| d.format("%d %b").to_string())
}

async fn board(ctx: &AppContext, org_id: i64, q: &str) -> Result<Board> {
    let team = Person::from_team(memberships::Model::team(&ctx.db, org_id).await?);
    let names: HashMap<i64, &str> = team.iter().map(|p| (p.id, p.username.as_str())).collect();
    let projects = projects::Model::list_for_org(&ctx.db, org_id).await?;
    let project_names: HashMap<i64, &str> =
        projects.iter().map(|p| (p.id, p.name.as_str())).collect();
    let counts = task_notes::Model::counts_for_org(&ctx.db, org_id).await?;
    let needle = q.trim().to_lowercase();

    let cards: Vec<Card> = tasks::Model::list_for_org(&ctx.db, org_id)
        .await?
        .into_iter()
        .map(|t| {
            let owner = t
                .owner_id
                .and_then(|id| names.get(&id))
                .map(|n| (*n).to_string());
            Card {
                id: t.id,
                project: project_names
                    .get(&t.project_id)
                    .copied()
                    .unwrap_or_default()
                    .to_string(),
                owner_color: owner.as_deref().map_or("#e8dfce", avatar_color),
                owner,
                due: due_label(t.due_on),
                notes: counts.get(&t.id).copied().unwrap_or(0),
                title: t.title,
                priority: t.priority,
                status: t.status,
            }
        })
        .filter(|c| {
            needle.is_empty()
                || c.title.to_lowercase().contains(&needle)
                || c.project.to_lowercase().contains(&needle)
                || c.owner
                    .as_deref()
                    .is_some_and(|o| o.to_lowercase().contains(&needle))
        })
        .collect();

    let mut columns: Vec<Column> = STATUSES
        .iter()
        .map(|&(key, label, color)| Column {
            key,
            label,
            color,
            cards: Vec::new(),
        })
        .collect();
    for card in cards {
        if let Some(col) = columns.iter_mut().find(|c| c.key == card.status) {
            col.cards.push(card);
        }
    }
    Ok(Board {
        columns,
        team,
        projects,
    })
}

fn board_data(board: &Board, q: &str) -> serde_json::Value {
    serde_json::json!({
        "columns": board.columns,
        "statuses": STATUSES.iter().map(|(k, l, _)| serde_json::json!({ "key": k, "label": l })).collect::<Vec<_>>(),
        "q": q,
    })
}

#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Query(query): Query<BoardQuery>,
) -> Result<Response> {
    let board = board(&ctx, member.org.id, &query.q).await?;
    let mut data = board_data(&board, &query.q);
    data["open_new"] = serde_json::json!(query.new.is_some());
    format::render().view(&v, "tasks/index.html", member.page("tasks", data))
}

/// The board on its own, for HTMX refreshes.
#[debug_handler]
async fn board_partial(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Query(query): Query<BoardQuery>,
) -> Result<Response> {
    let board = board(&ctx, member.org.id, &query.q).await?;
    format::render().view(&v, "tasks/_board.html", board_data(&board, &query.q))
}

fn task_form_data(
    board: &Board,
    values: serde_json::Value,
    errors: &FieldErrors,
) -> serde_json::Value {
    serde_json::json!({
        "form": values,
        "errors": errors,
        "team": board.team,
        "projects": board.projects.iter().map(|p| serde_json::json!({ "id": p.id, "name": p.name })).collect::<Vec<_>>(),
        "priorities": PRIORITIES,
    })
}

#[debug_handler]
async fn new(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let board = board(&ctx, member.org.id, "").await?;
    let values = serde_json::json!({
        "title": "", "project_id": "", "owner_id": member.user.id.to_string(), "priority": "medium", "due_on": "",
    });
    format::render().view(
        &v,
        "tasks/_form.html",
        task_form_data(&board, values, &FieldErrors::new()),
    )
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
            let board = board(&ctx, member.org.id, "").await?;
            format::render()
                .header("HX-Trigger", toast("success", "Task added"))
                .view(&v, "tasks/_board.html", board_data(&board, ""))
        }
        Err(err) => {
            let errors = field_errors(&err).ok_or(err)?;
            let board = board(&ctx, member.org.id, "").await?;
            let values = serde_json::json!({
                "title": params.title, "project_id": params.project_id, "owner_id": params.owner_id,
                "priority": params.priority, "due_on": params.due_on,
            });
            invalid_form(
                &v,
                "tasks/_form.html",
                TASK_FORM_ID,
                task_form_data(&board, values, &errors),
            )
        }
    }
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
    task.set_status(&ctx.db, &form.status).await?;
    if !headers.contains_key("hx-request") {
        return Ok(redirect_response(&headers, "/tasks"));
    }
    let board = board(&ctx, member.org.id, &form.q).await?;
    format::render()
        .header("HX-Trigger", toast("success", "Task updated"))
        .view(&v, "tasks/_board.html", board_data(&board, &form.q))
}

/// The side sheet for one task: details, notes and the note form.
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
                "at": n.created_at.format("%d %b, %H:%M").to_string(),
            })
        })
        .collect();
    let status_label = STATUSES
        .iter()
        .find(|(k, _, _)| *k == task.status)
        .map_or("", |(_, l, _)| *l);
    Ok(member.page(
        "tasks",
        data!({
            "task": {
                "id": task.id,
                "title": task.title,
                "priority": task.priority,
                "status": status_label,
                "due": due_label(task.due_on),
                "project": project.map(|p| p.name).unwrap_or_default(),
                "owner": task.owner_id.and_then(|id| team.get(&id).cloned()),
            },
            "notes": notes,
            "note_body": note_body,
            "errors": errors,
        }),
    ))
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
            // `board-changed` makes the board refresh its note counts.
            let trigger = serde_json::json!({
                "toast": { "kind": "success", "message": "Note posted" },
                "board-changed": true,
            });
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
        .add("/tasks/board", get(board_partial))
        .add("/tasks/new", get(new))
        .add("/tasks/{id}", get(show))
        .add("/tasks/{id}/status", post(set_status))
        .add("/tasks/{id}/notes", post(add_note))
}
