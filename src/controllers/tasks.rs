use std::collections::HashMap;

use axum::http::HeaderMap;
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    controllers::notifications::{self, TaskBefore},
    extractors::{current_member::CurrentMember, session::redirect_response},
    models::{
        field_error, memberships, note_reactions,
        notifications::{mention_parts, Part, EVERYONE},
        projects,
        saved_views::{self, Setup},
        task_assignees,
        task_notes::{self, NoteParams},
        tasks::{self, TaskParams, PRIORITIES, STATUSES},
    },
    views::{
        forms::{field_errors, invalid_form, toast, FieldErrors},
        layout::{avatar_color, Person},
        reactions::{chips, picks, Pick, ReactionChip, ReactionRow, FORMER_MEMBER},
        time,
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
/// How many done tasks a board lane shows; the rest are in the Done view.
const DONE_SHOWN: usize = 5;
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
    /// Narrows the list to one project or one person, e.g. from a board lane.
    #[serde(default)]
    project: Option<i64>,
    #[serde(default)]
    person: Option<i64>,
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
    project_id: Option<i64>,
    project: String,
    accent: String,
    meta: String,
    due: String,
    overdue: bool,
    priority: String,
    notes: usize,
    people: Vec<Chip>,
    #[serde(skip)]
    done_at: Option<chrono::DateTime<chrono::FixedOffset>>,
}

#[derive(Debug, Serialize)]
struct Group {
    label: String,
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
    person_id: Option<i64>,
    cells: Vec<Vec<Row>>,
    /// The Done cell shows only the latest few; this says there are more.
    more_done: bool,
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
    /// Today where the organisation is, for "overdue".
    today: chrono::NaiveDate,
    tz: chrono_tz::Tz,
}

impl Workspace {
    async fn load(ctx: &AppContext, member: &CurrentMember) -> Result<Self> {
        let org_id = member.org.id;
        let team_list = memberships::Model::team(&ctx.db, org_id).await?;
        Ok(Self {
            today: time::today(member.tz()),
            tz: member.tz(),
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
        let project = self.projects.iter().find(|p| Some(p.id) == t.project_id);
        let project_name = project.map(|p| p.name.clone()).unwrap_or_default();
        let today = self.today;
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
            done_at: t
                .done_at
                .or_else(|| (t.status == "done").then_some(t.updated_at)),
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

/// Which Done group a task finished in: "This week", "Last week", or its
/// month, e.g. "September 2026". Weeks start on Monday in the organisation's zone.
fn done_period(ws: &Workspace, done_at: chrono::DateTime<chrono::FixedOffset>) -> String {
    let day = time::local(done_at, ws.tz).date_naive();
    let this_week = ws.today.week(chrono::Weekday::Mon).first_day();
    if day >= this_week {
        "This week".to_string()
    } else if day >= this_week - chrono::Duration::days(7) {
        "Last week".to_string()
    } else {
        day.format("%B %Y").to_string()
    }
}

/// Narrows the task list to one project or one person.
#[derive(Debug, Default, Clone, Copy)]
struct Within {
    project: Option<i64>,
    person: Option<i64>,
}

impl Within {
    fn from_query(query: &ListQuery) -> Self {
        // One at a time: a project wins over a person.
        Self {
            project: query.project,
            person: query.person.filter(|_| query.project.is_none()),
        }
    }

    /// The project or person's name, or `None` when there is no narrowing or
    /// the id isn't in this organisation (then the list isn't narrowed).
    fn name(self, ws: &Workspace) -> Option<String> {
        if let Some(id) = self.project {
            ws.projects
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.name.clone())
        } else {
            self.person.and_then(|id| ws.team.get(&id).cloned())
        }
    }

    fn matches(self, row: &Row) -> bool {
        self.project.is_none_or(|id| row.project_id == Some(id))
            && self
                .person
                .is_none_or(|id| row.people.iter().any(|p| p.id == id))
    }

    fn query(self) -> String {
        match (self.project, self.person) {
            (Some(id), _) => format!("&project={id}"),
            (None, Some(id)) => format!("&person={id}"),
            (None, None) => String::new(),
        }
    }
}

/// The task list for one filter and search, grouped by status, plus filter
/// counts. The Done filter groups by when tasks were finished instead.
fn list_data(ws: &Workspace, me: i64, filter: &str, q: &str, within: Within) -> serde_json::Value {
    let needle = q.trim().to_lowercase();
    let within_name = within.name(ws);
    let within = if within_name.is_some() {
        within
    } else {
        Within::default()
    };
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
    let mut rows: Vec<Row> = ws
        .tasks
        .iter()
        .filter(|t| matches(t, filter))
        .map(|t| ws.row(t))
        .filter(|r| matches_search(r, &needle) && within.matches(r))
        .collect();
    let mut groups: Vec<Group> = Vec::new();
    if filter == "done" {
        rows.sort_by_key(|r| std::cmp::Reverse(r.done_at));
        for row in rows {
            let label = row
                .done_at
                .map(|at| done_period(ws, at))
                .unwrap_or_default();
            match groups.last_mut() {
                Some(group) if group.label == label => group.rows.push(row),
                _ => groups.push(Group {
                    label,
                    rows: vec![row],
                }),
            }
        }
    } else {
        groups = order
            .iter()
            .map(|(_, label)| Group {
                label: (*label).to_string(),
                rows: Vec::new(),
            })
            .collect();
        for row in rows {
            if let Some(i) = order.iter().position(|(key, _)| *key == row.status) {
                groups[i].rows.push(row);
            }
        }
        groups.retain(|g| !g.rows.is_empty());
    }
    serde_json::json!({
        "filters": filters,
        "filter": filter,
        "groups": groups,
        "q": q,
        "within": within_name,
        "within_project": within.project,
        "within_person": within.person,
        "within_query": within.query(),
    })
}

/// The board: status columns, with one swimlane per project, person, or none.
/// The board's lanes and toolbar state. `layout` is "board" or "list"; the
/// desktop list shows the same lanes as rows.
fn board_data(
    ws: &Workspace,
    me: i64,
    scope: &str,
    group: &str,
    q: &str,
    layout: &str,
) -> serde_json::Value {
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
                project_id: Option<i64>,
                person_id: Option<i64>,
                person: bool,
                rows: Vec<&Row>| {
        let done = rows.iter().filter(|r| r.status == "done").count();
        let total = rows.len();
        let cells = STATUSES
            .iter()
            .map(|(status, _, _)| {
                let mut cell: Vec<Row> = rows
                    .iter()
                    .filter(|r| r.status == *status)
                    .map(|r| (*r).clone())
                    .collect();
                if *status == "done" {
                    cell.sort_by_key(|r| std::cmp::Reverse(r.done_at));
                    cell.truncate(DONE_SHOWN);
                }
                cell
            })
            .collect();
        Lane {
            key,
            name,
            color,
            person,
            total,
            done,
            pct: (done * 100).checked_div(total).unwrap_or(0),
            project_id,
            person_id,
            cells,
            more_done: done > DONE_SHOWN,
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
                            None,
                            Some(*id),
                            true,
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
                    None,
                    None,
                    true,
                    nobody,
                ));
            }
            lanes
        }
        "none" => vec![lane(
            "all".into(),
            "All tasks".into(),
            "#f2a93b".into(),
            None,
            None,
            false,
            rows.iter().collect(),
        )],
        _ => {
            let mut lanes: Vec<Lane> = ws
                .projects
                .iter()
                .filter_map(|p| {
                    let theirs: Vec<&Row> =
                        rows.iter().filter(|r| r.project_id == Some(p.id)).collect();
                    (!theirs.is_empty()).then(|| {
                        lane(
                            format!("project-{}", p.id),
                            p.name.clone(),
                            p.accent.clone(),
                            Some(p.id),
                            None,
                            false,
                            theirs,
                        )
                    })
                })
                .collect();
            let chores: Vec<&Row> = rows.iter().filter(|r| r.project_id.is_none()).collect();
            if !chores.is_empty() {
                lanes.push(lane(
                    "no-project".into(),
                    "No project".into(),
                    "#d6d3cb".into(),
                    None,
                    None,
                    false,
                    chores,
                ));
            }
            lanes
        }
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
        "layout": layout,
    })
}

fn layout(view: Option<&str>) -> &'static str {
    if view == Some("list") {
        "list"
    } else {
        "board"
    }
}

#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Query(query): Query<ListQuery>,
) -> Result<Response> {
    let ws = Workspace::load(&ctx, &member).await?;
    let views = saved_views::Model::list_for(&ctx.db, member.org.id, member.user.id).await?;
    // A plain visit to Tasks opens your default view, if you starred one.
    let mut query = query;
    let plain = query.view.is_none()
        && query.scope.is_none()
        && query.group.is_none()
        && query.filter.is_none()
        && query.q.is_empty();
    if let Some(default) = views.iter().find(|v| v.is_default).filter(|_| plain) {
        let setup = default.setup();
        query.filter = Some(setup.scope.clone());
        query.scope = Some(setup.scope);
        query.group = Some(setup.group);
        query.view = Some(setup.view);
        query.q = setup.q;
    }
    let filter = pick(query.filter.as_deref(), &FILTERS, "mine");
    let scope = pick(query.scope.as_deref(), &SCOPES, "all");
    let group = pick(query.group.as_deref(), &GROUPS, "project");
    let view = layout(query.view.as_deref());
    let current = Setup {
        scope: scope.to_string(),
        group: group.to_string(),
        view: view.to_string(),
        q: query.q.trim().to_string(),
    };
    let mut data = list_data(
        &ws,
        member.user.id,
        filter,
        &query.q,
        Within::from_query(&query),
    );
    data["views"] = serde_json::json!(views
        .iter()
        .map(|v| serde_json::json!({
            "id": v.id,
            "name": v.name,
            "url": v.setup().url(),
            "is_default": v.is_default,
            "current": filter != "done" && v.setup() == current,
        }))
        .collect::<Vec<_>>());
    data["setup_query"] = serde_json::json!(current.query());
    data["board"] = board_data(&ws, member.user.id, scope, group, &query.q, view);
    data["view"] = serde_json::json!(view);
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
    let ws = Workspace::load(&ctx, &member).await?;
    let filter = pick(query.filter.as_deref(), &FILTERS, "mine");
    format::render().view(
        &v,
        "tasks/_list.html",
        member.fragment(list_data(
            &ws,
            member.user.id,
            filter,
            &query.q,
            Within::from_query(&query),
        )),
    )
}

/// The board (or the desktop list, with `view=list`) on its own, for HTMX refreshes.
#[debug_handler]
async fn board_partial(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Query(query): Query<ListQuery>,
) -> Result<Response> {
    let ws = Workspace::load(&ctx, &member).await?;
    let scope = pick(query.scope.as_deref(), &SCOPES, "all");
    let group = pick(query.group.as_deref(), &GROUPS, "project");
    let view = layout(query.view.as_deref());
    let data = serde_json::json!({
        "board": board_data(&ws, member.user.id, scope, group, &query.q, view),
    });
    let template = if view == "list" {
        "tasks/_table.html"
    } else {
        "tasks/_board.html"
    };
    format::render().view(&v, template, member.fragment(data))
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
    // Grouped by roadmap lane so the picker reads Now, Next, Later.
    let all_projects = projects::Model::list_for_org(&ctx.db, member.org.id).await?;
    let project_groups: Vec<serde_json::Value> = projects::LANES
        .iter()
        .filter_map(|lane| {
            let projects: Vec<serde_json::Value> = all_projects
                .iter()
                .filter(|p| p.lane == *lane)
                .map(|p| serde_json::json!({ "id": p.id, "name": p.name }))
                .collect();
            let label = match *lane {
                "now" => "Now",
                "next" => "Next",
                _ => "Later",
            };
            (!projects.is_empty())
                .then(|| serde_json::json!({ "label": label, "projects": projects }))
        })
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
        "project_groups": project_groups,
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
    format::render().view(&v, "tasks/_form.html", member.fragment(data))
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
        Ok(_) if !headers.contains_key("hx-request") => {
            Ok(redirect_response(&headers, &member.url("/tasks")))
        }
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
            invalid_form(&v, "tasks/_form.html", TASK_FORM_ID, member.fragment(data))
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
        project_id: task.project_id.map(|id| id.to_string()).unwrap_or_default(),
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
    format::render().view(&v, "tasks/_form.html", member.fragment(data))
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
        Ok(_) if !headers.contains_key("hx-request") => {
            Ok(redirect_response(&headers, &member.url("/tasks")))
        }
        Ok(task) => {
            let data = sheet_data(&ctx, &member, &task, "", &FieldErrors::new()).await?;
            let trigger = serde_json::json!({ "toast": { "kind": "success", "message": "Task saved" }, "tasks-changed": true });
            format::render()
                .header("HX-Trigger", trigger.to_string())
                .view(&v, "tasks/_sheet.html", member.fragment(data))
        }
        Err(err) => {
            let errors = field_errors(&err).ok_or(err)?;
            let data =
                task_form_data(&ctx, &member, Some(id), form_values(&params), &errors).await?;
            invalid_form(&v, "tasks/_form.html", TASK_FORM_ID, member.fragment(data))
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
        return Ok(redirect_response(&headers, &member.url("/tasks")));
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
    let project = match task.project_id {
        Some(id) => projects::Model::find_in_org(&ctx.db, member.org.id, id)
            .await
            .ok(),
        None => None,
    };
    let list = task_notes::Model::list_for_task(&ctx.db, task).await?;
    let ids: Vec<i64> = list.iter().map(|n| n.id).collect();
    let mut reactions = note_reactions::Model::for_notes(&ctx.db, member.org.id, &ids).await?;
    let note_names = mentionable(&team_list);
    let notes: Vec<NoteView> = list
        .iter()
        .map(|n| {
            let rows = reaction_rows(reactions.remove(&n.id).unwrap_or_default(), &team);
            note_view(n, &team, &note_names, &rows, member)
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
    // The @ menu offers "everyone" first when there is anyone else to tell,
    // except to a super admin acting here, whose @everyone tells nobody.
    let others = team_list
        .iter()
        .filter(|(id, _)| *id != member.user.id)
        .count();
    let offer_everyone = others > 0 && !member.acting;
    let names = mention_names(&team_list, member.user.id);
    let names = if offer_everyone {
        format!("{EVERYONE},{names}")
    } else {
        names
    };
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
            "mention_names": names,
            "everyone_hint": if offer_everyone { everyone_hint(others) } else { String::new() },
            "errors": errors,
        }),
    ))
}

/// One note as its viewer sees it, with its reaction chips and quick picks.
#[derive(Debug, Serialize)]
struct NoteView {
    id: i64,
    task_id: i64,
    author: String,
    color: &'static str,
    parts: Vec<Part>,
    at: String,
    reactions: Vec<ReactionChip>,
    picks: Vec<Pick>,
}

fn reaction_rows(
    rows: Vec<note_reactions::Model>,
    team: &HashMap<i64, String>,
) -> Vec<ReactionRow> {
    rows.into_iter()
        .map(|r| ReactionRow::named(r.emoji, r.user_id, team))
        .collect()
}

/// The names a note's text can mention: the team, and "everyone" so that
/// "@everyone" is highlighted.
fn mentionable(team_list: &[(i64, String)]) -> Vec<(i64, String)> {
    team_list
        .iter()
        .cloned()
        // Not a person; only its name is used here.
        .chain([(0, EVERYONE.to_string())])
        .collect()
}

fn note_view(
    note: &task_notes::Model,
    team: &HashMap<i64, String>,
    mentionable: &[(i64, String)],
    rows: &[ReactionRow],
    member: &CurrentMember,
) -> NoteView {
    let author = note
        .author_id
        .and_then(|id| team.get(&id).cloned())
        .unwrap_or_else(|| FORMER_MEMBER.to_string());
    let reactions = chips(rows, member.user.id);
    NoteView {
        id: note.id,
        task_id: note.task_id,
        color: avatar_color(&author),
        author,
        parts: mention_parts(&note.body, mentionable),
        at: time::local(note.created_at, member.tz())
            .format("%a %-d %b, %H:%M")
            .to_string(),
        picks: picks(&reactions),
        reactions,
    }
}

/// The hint under "@everyone" in the note box's @ menu.
fn everyone_hint(others: usize) -> String {
    if others == 1 {
        "Notify the whole team (1 person)".to_string()
    } else {
        format!("Notify the whole team ({others} people)")
    }
}

/// Names the @ picker offers: teammates first, then the viewer, who can tag
/// themselves as a reminder (it notifies them too).
pub fn mention_names(people: &[(i64, String)], me: i64) -> String {
    let (mine, others): (Vec<_>, Vec<_>) = people.iter().partition(|(id, _)| *id == me);
    others
        .iter()
        .chain(mine.iter())
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
    format::render().view(&v, "tasks/_sheet.html", member.fragment(data))
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
        return Ok(redirect_response(&headers, &member.url("/tasks")));
    }
    let data = sheet_data(&ctx, &member, &task, "", &FieldErrors::new()).await?;
    let trigger = serde_json::json!({ "toast": { "kind": "success", "message": "Status updated" }, "tasks-changed": true });
    format::render()
        .header("HX-Trigger", trigger.to_string())
        .view(&v, "tasks/_sheet.html", member.fragment(data))
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
        Ok(_) if !headers.contains_key("hx-request") => {
            Ok(redirect_response(&headers, &member.url("/tasks")))
        }
        Ok(_) => {
            let data = sheet_data(&ctx, &member, &task, "", &FieldErrors::new()).await?;
            let trigger = serde_json::json!({ "toast": { "kind": "success", "message": "Note posted" }, "tasks-changed": true });
            format::render()
                .header("HX-Trigger", trigger.to_string())
                .view(&v, "tasks/_sheet.html", member.fragment(data))
        }
        Err(err) => {
            let errors = field_errors(&err).ok_or(err)?;
            let data = sheet_data(&ctx, &member, &task, &params.body, &errors).await?;
            invalid_form(
                &v,
                "tasks/_note_form.html",
                NOTE_FORM_ID,
                member.fragment(data),
            )
        }
    }
}

/// The state the client wants: `on` is "1" to add `emoji`, "0" to remove it.
#[derive(Debug, Default, Deserialize)]
struct NoteReactionForm {
    #[serde(default)]
    emoji: String,
    #[serde(default)]
    on: String,
}

/// Sets the viewer's reaction on a note and answers with the note. No
/// notification and no live update.
#[debug_handler]
async fn react_note(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path((id, note_id)): Path<(i64, i64)>,
    Form(form): Form<NoteReactionForm>,
) -> Result<Response> {
    // A reaction carries a person's name, so it needs a real membership.
    member.require_own_membership()?;
    let task = tasks::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    let note = task_notes::Model::find_on_task(&ctx.db, &task, note_id).await?;
    let me = member.user.id;
    let result = match form.on.as_str() {
        "1" | "0" => {
            note_reactions::Model::set(&ctx.db, &note, me, &form.emoji, form.on == "1").await
        }
        _ => Err(field_error("on", "Couldn't change your reaction.")),
    };
    if let Err(err) = result {
        let errors = field_errors(&err).ok_or(err)?;
        let text = errors.values().next().cloned().unwrap_or_default();
        return format::render()
            .status(422)
            .header("HX-Reswap", "none")
            .header("HX-Trigger", toast("error", &text))
            .empty();
    }
    let team_list = memberships::Model::team(&ctx.db, member.org.id).await?;
    let team: HashMap<i64, String> = team_list.iter().cloned().collect();
    let rows = note_reactions::Model::for_notes(&ctx.db, member.org.id, &[note.id])
        .await?
        .remove(&note.id)
        .unwrap_or_default();
    let view = note_view(
        &note,
        &team,
        &mentionable(&team_list),
        &reaction_rows(rows, &team),
        &member,
    );
    format::render().view(
        &v,
        "tasks/_note.html",
        member.fragment(data!({ "note": view })),
    )
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
        .add("/tasks/{id}/notes/{note_id}/reactions", post(react_note))
}
