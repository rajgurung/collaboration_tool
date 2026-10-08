use std::collections::HashMap;

use axum::http::HeaderMap;
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    controllers::notifications::{self, TaskBefore},
    extractors::{current_member::CurrentMember, session::redirect_response},
    models::{
        memberships,
        notifications::mention_parts,
        projects, task_assignees,
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
/// Board scopes: whose tasks the board shows.
const SCOPES: [(&str, &str); 2] = [("all", "All"), ("mine", "Mine")];
/// Board swimlanes.
const GROUPS: [(&str, &str); 3] = [
    ("project", "Project"),
    ("person", "Person"),
    ("none", "None"),
];

#[derive(Debug, Default, Deserialize)]
struct ListQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    filter: Option<String>,
    #[serde(default)]
    new: Option<String>,
    /// A task to open in the side panel, e.g. from a notification.
    #[serde(default)]
    open: Option<i64>,
    #[serde(default)]
    view: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    group: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct NewQuery {
    #[serde(default)]
    status: String,
    #[serde(default)]
    project_id: String,
}

#[derive(Debug, Deserialize)]
struct StatusForm {
    status: String,
}

#[derive(Debug, Clone, Serialize)]
struct Chip {
    id: i64,
    name: String,
    color: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct Row {
    id: i64,
    title: String,
    status: String,
    project_id: i64,
    project: String,
    accent: String,
    meta: String,
    due: String,
    overdue: bool,
    priority: String,
    notes: usize,
    people: Vec<Chip>,
}

#[derive(Debug, Serialize)]
struct Group {
    label: &'static str,
    rows: Vec<Row>,
}

#[derive(Debug, Serialize)]
struct Lane {
    key: String,
    name: String,
    /// A project's accent, or a person's avatar colour.
    color: String,
    person: bool,
    total: usize,
    done: usize,
    pct: usize,
    project_id: Option<i64>,
    cells: Vec<Vec<Row>>,
}

fn due_label(date: Option<chrono::NaiveDate>) -> String {
    date.map_or_else(String::new, |d| format!("Due {}", d.format("%a %-d %b")))
}

fn pick<'a>(value: Option<&'a str>, allowed: &[(&'a str, &str)], default: &'a str) -> &'a str {
    match value {
        Some(v) if allowed.iter().any(|(key, _)| *key == v) => v,
        _ => default,
    }
}

/// Everything the list and board views need, loaded once.
struct Workspace {
    team: HashMap<i64, String>,
    team_order: Vec<i64>,
    projects: Vec<projects::Model>,
    tasks: Vec<tasks::Model>,
    assignees: HashMap<i64, Vec<i64>>,
    notes: HashMap<i64, usize>,
}

impl Workspace {
    async fn load(ctx: &AppContext, org_id: i64) -> Result<Self> {
        let team_list = memberships::Model::team(&ctx.db, org_id).await?;
        Ok(Self {
            team_order: team_list.iter().map(|(id, _)| *id).collect(),
            team: team_list.into_iter().collect(),
            projects: projects::Model::list_for_org(&ctx.db, org_id).await?,
            tasks: tasks::Model::list_for_org(&ctx.db, org_id).await?,
            assignees: task_assignees::Model::by_task(&ctx.db, org_id).await?,
            notes: task_notes::Model::counts_for_org(&ctx.db, org_id).await?,
        })
    }

    fn assigned_to(&self, task: &tasks::Model, user_id: i64) -> bool {
        self.assignees
            .get(&task.id)
            .is_some_and(|ids| ids.contains(&user_id))
    }

    fn chip(&self, user_id: i64) -> Option<Chip> {
        self.team.get(&user_id).map(|name| Chip {
            id: user_id,
            color: avatar_color(name),
            name: name.clone(),
        })
    }

    fn row(&self, t: &tasks::Model) -> Row {
        let project = self.projects.iter().find(|p| p.id == t.project_id);
        let project_name = project.map(|p| p.name.clone()).unwrap_or_default();
        let today = chrono::Utc::now().date_naive();
        let due = due_label(t.due_on);
        Row {
            id: t.id,
            title: t.title.clone(),
            status: t.status.clone(),
            project_id: t.project_id,
            accent: project.map_or_else(|| "#9a968d".to_string(), |p| p.accent.clone()),
            meta: [due.as_str(), project_name.as_str()]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(" · "),
            project: project_name,
            due: t
                .due_on
                .map_or_else(String::new, |d| d.format("%a %-d %b").to_string()),
            overdue: t.due_on.is_some_and(|d| d < today) && t.status != "done",
            priority: t.priority.clone(),
            notes: self.notes.get(&t.id).copied().unwrap_or(0),
            people: self
                .assignees
                .get(&t.id)
                .map(|ids| ids.iter().filter_map(|id| self.chip(*id)).collect())
                .unwrap_or_default(),
        }
    }
}

fn matches_search(row: &Row, needle: &str) -> bool {
    needle.is_empty()
        || row.title.to_lowercase().contains(needle)
        || row.project.to_lowercase().contains(needle)
        || row
            .people
            .iter()
            .any(|p| p.name.to_lowercase().contains(needle))
}

/// The task list for one filter and search, grouped by status, plus filter counts.
fn list_data(ws: &Workspace, me: i64, filter: &str, q: &str) -> serde_json::Value {
    let needle = q.trim().to_lowercase();
    let matches = |t: &tasks::Model, f: &str| match f {
        "mine" => ws.assigned_to(t, me) && t.status != "done",
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
                "count": ws.tasks.iter().filter(|t| matches(t, key)).count(),
                "current": *key == filter,
            })
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
    for row in ws
        .tasks
        .iter()
        .filter(|t| matches(t, filter))
        .map(|t| ws.row(t))
        .filter(|r| matches_search(r, &needle))
    {
        if let Some(i) = order.iter().position(|(key, _)| *key == row.status) {
            groups[i].rows.push(row);
        }
    }
    groups.retain(|g| !g.rows.is_empty());
    serde_json::json!({ "filters": filters, "filter": filter, "groups": groups, "q": q })
}

/// The board: status columns, with one swimlane per project, person, or none.
fn board_data(ws: &Workspace, me: i64, scope: &str, group: &str, q: &str) -> serde_json::Value {
    let needle = q.trim().to_lowercase();
    let rows: Vec<Row> = ws
        .tasks
        .iter()
        .filter(|t| scope != "mine" || ws.assigned_to(t, me))
        .map(|t| ws.row(t))
        .filter(|r| matches_search(r, &needle))
        .collect();

    let lane = |key: String,
                name: String,
                color: String,
                person: bool,
                project_id: Option<i64>,
                rows: Vec<&Row>| {
        let done = rows.iter().filter(|r| r.status == "done").count();
        let total = rows.len();
        Lane {
            key,
            name,
            color,
            person,
            total,
            done,
            pct: (done * 100).checked_div(total).unwrap_or(0),
            project_id,
            cells: STATUSES
                .iter()
                .map(|(status, _, _)| {
                    rows.iter()
                        .filter(|r| r.status == *status)
                        .map(|r| (*r).clone())
                        .collect()
                })
                .collect(),
        }
    };
    let lanes: Vec<Lane> = match group {
        "person" => {
            let mut lanes: Vec<Lane> = ws
                .team_order
                .iter()
                .filter(|id| scope != "mine" || **id == me)
                .filter_map(|id| {
                    let mine: Vec<&Row> = rows
                        .iter()
                        .filter(|r| r.people.iter().any(|p| p.id == *id))
                        .collect();
                    let name = ws.team.get(id)?;
                    (!mine.is_empty()).then(|| {
                        lane(
                            format!("person-{id}"),
                            name.clone(),
                            avatar_color(name).to_string(),
                            true,
                            None,
                            mine,
                        )
                    })
                })
                .collect();
            let nobody: Vec<&Row> = rows.iter().filter(|r| r.people.is_empty()).collect();
            if !nobody.is_empty() {
                lanes.push(lane(
                    "unassigned".into(),
                    "Unassigned".into(),
                    "#d6d3cb".into(),
                    true,
                    None,
                    nobody,
                ));
            }
            lanes
        }
        "none" => vec![lane(
            "all".into(),
            "All tasks".into(),
            "#f2a93b".into(),
            false,
            None,
            rows.iter().collect(),
        )],
        _ => ws
            .projects
            .iter()
            .filter_map(|p| {
                let theirs: Vec<&Row> = rows.iter().filter(|r| r.project_id == p.id).collect();
                (!theirs.is_empty()).then(|| {
                    lane(
                        format!("project-{}", p.id),
                        p.name.clone(),
                        p.accent.clone(),
                        false,
                        Some(p.id),
                        theirs,
                    )
                })
            })
            .collect(),
    };
    let columns: Vec<serde_json::Value> = STATUSES
        .iter()
        .map(|(key, label, color)| {
            serde_json::json!({
                "key": key, "label": label, "color": color,
                "count": rows.iter().filter(|r| r.status == *key).count(),
            })
        })
        .collect();
    let options = |list: &[(&str, &str)], current: &str| -> Vec<serde_json::Value> {
        list.iter()
            .map(|(key, label)| serde_json::json!({ "key": key, "label": label, "current": *key == current }))
            .collect()
    };
    serde_json::json!({
        "lanes": lanes,
        "columns": columns,
        "scope": scope,
        "group": group,
        "scopes": options(&SCOPES, scope),
        "groups": options(&GROUPS, group),
        "q": q,
    })
}

#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Query(query): Query<ListQuery>,
) -> Result<Response> {
    let ws = Workspace::load(&ctx, member.org.id).await?;
    let filter = pick(query.filter.as_deref(), &FILTERS, "mine");
    let scope = pick(query.scope.as_deref(), &SCOPES, "all");
    let group = pick(query.group.as_deref(), &GROUPS, "project");
    let mut data = list_data(&ws, member.user.id, filter, &query.q);
    data["board"] = board_data(&ws, member.user.id, scope, group, &query.q);
    data["view"] = serde_json::json!(if query.view.as_deref() == Some("list") {
        "list"
    } else {
        "board"
    });
    data["open_new"] = serde_json::json!(query.new.is_some());
    data["open_task"] = serde_json::json!(query.open);
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
    let ws = Workspace::load(&ctx, member.org.id).await?;
    let filter = pick(query.filter.as_deref(), &FILTERS, "mine");
    format::render().view(
        &v,
        "tasks/_list.html",
        list_data(&ws, member.user.id, filter, &query.q),
    )
}

/// The board on its own, for HTMX refreshes.
#[debug_handler]
async fn board_partial(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Query(query): Query<ListQuery>,
) -> Result<Response> {
    let ws = Workspace::load(&ctx, member.org.id).await?;
    let scope = pick(query.scope.as_deref(), &SCOPES, "all");
    let group = pick(query.group.as_deref(), &GROUPS, "project");
    let data =
        serde_json::json!({ "board": board_data(&ws, member.user.id, scope, group, &query.q) });
    format::render().view(&v, "tasks/_board.html", data)
}

/// The values a task form shows: a new task's defaults, an existing task, or what was submitted.
fn form_values(params: &TaskParams) -> serde_json::Value {
    serde_json::json!({
        "title": params.title, "project_id": params.project_id, "assignee_ids": params.assignee_ids,
        "priority": params.priority, "due_on": params.due_on, "status": params.status,
    })
}

async fn task_form_data(
    ctx: &AppContext,
    member: &CurrentMember,
    task_id: Option<i64>,
    values: serde_json::Value,
    errors: &FieldErrors,
) -> Result<serde_json::Value> {
    let team = Person::from_team(memberships::Model::team(&ctx.db, member.org.id).await?);
    let projects: Vec<serde_json::Value> = projects::Model::list_for_org(&ctx.db, member.org.id)
        .await?
        .into_iter()
        .map(|p| serde_json::json!({ "id": p.id, "name": p.name, "accent": p.accent }))
        .collect();
    let statuses: Vec<serde_json::Value> = STATUSES
        .iter()
        .map(
            |(key, label, color)| serde_json::json!({ "key": key, "label": label, "color": color }),
        )
        .collect();
    Ok(serde_json::json!({
        "task_id": task_id,
        "form": values,
        "errors": errors,
        "team": team,
        "projects": projects,
        "priorities": PRIORITIES,
        "statuses": statuses,
    }))
}

#[debug_handler]
async fn new(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Query(query): Query<NewQuery>,
) -> Result<Response> {
    let params = TaskParams {
        title: String::new(),
        project_id: query.project_id,
        assignee_ids: vec![member.user.id],
        priority: "medium".to_string(),
        due_on: String::new(),
        status: if tasks::is_status(&query.status) {
            query.status
        } else {
            String::new()
        },
    };
    let data = task_form_data(
        &ctx,
        &member,
        None,
        form_values(&params),
        &FieldErrors::new(),
    )
    .await?;
    format::render().view(&v, "tasks/_form.html", data)
}

#[debug_handler]
async fn create(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    axum_extra::extract::Form(params): axum_extra::extract::Form<TaskParams>,
) -> Result<Response> {
    let result = tasks::Model::create(&ctx.db, member.org.id, &params).await;
    if let Ok(task) = &result {
        notifications::task_saved(&ctx, &member, None, task).await?;
    }
    match result {
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
            let data = task_form_data(&ctx, &member, None, form_values(&params), &errors).await?;
            invalid_form(&v, "tasks/_form.html", TASK_FORM_ID, data)
        }
    }
}

#[debug_handler]
async fn edit(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<i64>,
) -> Result<Response> {
    let task = tasks::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    let params = TaskParams {
        title: task.title.clone(),
        project_id: task.project_id.to_string(),
        assignee_ids: task_assignees::Model::for_task(&ctx.db, member.org.id, task.id).await?,
        priority: task.priority.clone(),
        due_on: task.due_on.map(|d| d.to_string()).unwrap_or_default(),
        status: task.status.clone(),
    };
    let data = task_form_data(
        &ctx,
        &member,
        Some(id),
        form_values(&params),
        &FieldErrors::new(),
    )
    .await?;
    format::render().view(&v, "tasks/_form.html", data)
}

#[debug_handler]
async fn update(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    axum_extra::extract::Form(params): axum_extra::extract::Form<TaskParams>,
) -> Result<Response> {
    let task = tasks::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    let before = TaskBefore {
        assignees: task_assignees::Model::for_task(&ctx.db, member.org.id, task.id).await?,
        status: task.status.clone(),
    };
    let result = task.update_from(&ctx.db, &params).await;
    if let Ok(task) = &result {
        notifications::task_saved(&ctx, &member, Some(before), task).await?;
    }
    match result {
        Ok(_) if !headers.contains_key("hx-request") => Ok(redirect_response(&headers, "/tasks")),
        Ok(task) => {
            let data = sheet_data(&ctx, &member, &task, "", &FieldErrors::new()).await?;
            let trigger = serde_json::json!({ "toast": { "kind": "success", "message": "Task saved" }, "tasks-changed": true });
            format::render()
                .header("HX-Trigger", trigger.to_string())
                .view(&v, "tasks/_sheet.html", data)
        }
        Err(err) => {
            let errors = field_errors(&err).ok_or(err)?;
            let data =
                task_form_data(&ctx, &member, Some(id), form_values(&params), &errors).await?;
            invalid_form(&v, "tasks/_form.html", TASK_FORM_ID, data)
        }
    }
}

#[debug_handler]
async fn destroy(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response> {
    let task = tasks::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    task.remove(&ctx.db).await?;
    if !headers.contains_key("hx-request") {
        return Ok(redirect_response(&headers, "/tasks"));
    }
    let trigger = serde_json::json!({
        "toast": { "kind": "success", "message": "Task deleted" },
        "tasks-changed": true,
        "close-dialogs": true,
    });
    format::render()
        .header("HX-Trigger", trigger.to_string())
        .header("HX-Reswap", "none")
        .empty()
}

/// The sheet for one task: details, assignees, status switcher, notes and the note form.
async fn sheet_data(
    ctx: &AppContext,
    member: &CurrentMember,
    task: &tasks::Model,
    note_body: &str,
    errors: &FieldErrors,
) -> Result<serde_json::Value> {
    let team_list = memberships::Model::team(&ctx.db, member.org.id).await?;
    let team: HashMap<i64, String> = team_list.iter().cloned().collect();
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
                "parts": mention_parts(&n.body, &team_list),
                "at": n.created_at.format("%a %-d %b, %H:%M").to_string(),
            })
        })
        .collect();
    let people: Vec<Chip> = task_assignees::Model::for_task(&ctx.db, member.org.id, task.id)
        .await?
        .into_iter()
        .filter_map(|id| {
            team.get(&id).map(|name| Chip {
                id,
                color: avatar_color(name),
                name: name.clone(),
            })
        })
        .collect();
    let statuses: Vec<serde_json::Value> = STATUSES
        .iter()
        .map(|(key, label, color)| serde_json::json!({ "key": key, "label": label, "color": color, "current": *key == task.status }))
        .collect();
    let meta = [
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
                "project": project.as_ref().map(|p| p.name.clone()).unwrap_or_default(),
                "accent": project.as_ref().map_or_else(|| "#9a968d".to_string(), |p| p.accent.clone()),
                "people": people,
            },
            "statuses": statuses,
            "notes": notes,
            "note_body": note_body,
            "mention_names": mention_names(&team_list, member.user.id),
            "errors": errors,
        }),
    ))
}

/// Teammates the @ picker offers, everyone but the viewer.
pub fn mention_names(people: &[(i64, String)], me: i64) -> String {
    people
        .iter()
        .filter(|(id, _)| *id != me)
        .map(|(_, name)| name.as_str())
        .collect::<Vec<_>>()
        .join(",")
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
    let before = TaskBefore {
        assignees: task_assignees::Model::for_task(&ctx.db, member.org.id, task.id).await?,
        status: task.status.clone(),
    };
    let task = task.set_status(&ctx.db, &form.status).await?;
    notifications::task_saved(&ctx, &member, Some(before), &task).await?;
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
    let result = task_notes::Model::create(&ctx.db, &task, member.user.id, &params).await;
    if let Ok(note) = &result {
        notifications::note_added(&ctx, &member, &task, note).await?;
    }
    match result {
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
        .add("/tasks/board", get(board_partial))
        .add("/tasks/new", get(new))
        .add("/tasks/{id}", get(show))
        .add("/tasks/{id}", post(update))
        .add("/tasks/{id}/edit", get(edit))
        .add("/tasks/{id}/delete", post(destroy))
        .add("/tasks/{id}/status", post(set_status))
        .add("/tasks/{id}/notes", post(add_note))
}
