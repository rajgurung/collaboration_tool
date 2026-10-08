//! The tools Claude can call. Each one acts as the member the bearer token
//! belongs to (put in the request by `extractors::bearer`), inside their one
//! organisation, through the same model methods and notifications as the web
//! pages. Nothing here takes an organisation id.
use std::collections::HashMap;

use axum::http::request::Parts;
use loco_rs::{app::AppContext, model::ModelError};
use rmcp::{
    handler::server::{router::tool::ToolRouter, tool::Extension, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    schemars, tool, tool_handler, tool_router, ErrorData, ServerHandler,
};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};

use crate::{
    controllers::notifications::{self, TaskBefore},
    extractors::current_member::CurrentMember,
    models::{
        memberships,
        projects::{self, Progress, ProjectParams, ACCENTS},
        task_assignees,
        task_notes::{self, NoteParams},
        tasks::{self, TaskParams},
    },
    views::{forms::field_errors, time},
};

const INSTRUCTIONS: &str = "Collab Tool is a small team's workspace. Tasks usually belong to a \
project; a task with no project is a chore. Projects sit in roadmap lanes: now, next or later. \
Task statuses are todo, progress, blocked and done; priorities are high, medium and low. A \
project's status is free text such as \"Active\" or \"Planned\". People are referred to by \
username; \"me\" means the person you are acting for. Changes notify teammates exactly as they \
would in the web app.";
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;

#[derive(Clone)]
pub struct CollabServer {
    ctx: AppContext,
    #[allow(dead_code)] // read by the `tool_handler` macro
    tool_router: ToolRouter<Self>,
}

impl CollabServer {
    #[must_use]
    pub fn new(ctx: AppContext) -> Self {
        Self {
            ctx,
            tool_router: Self::tool_router(),
        }
    }
}

/// `Some(None)` for an explicit `null`, `None` when the field is left out.
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateProject {
    #[schemars(description = "Project name, up to 80 characters.")]
    pub name: String,
    #[schemars(description = "Roadmap lane: now, next or later.")]
    pub lane: String,
    #[schemars(
        description = "Short free-text project status, up to 40 characters, e.g. \"Active\" or \"Planned\". Not a task status."
    )]
    pub status: String,
    #[schemars(description = "Optional summary, up to 500 characters.")]
    pub summary: Option<String>,
    #[schemars(description = "Username of the project owner. Leave out for no owner.")]
    pub owner: Option<String>,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ListTasks {
    #[schemars(description = "Only tasks in this project.")]
    pub project_id: Option<i64>,
    #[schemars(description = "Only tasks with this status: todo, progress, blocked or done.")]
    pub status: Option<String>,
    #[schemars(description = "Only tasks assigned to this username, or \"me\".")]
    pub assignee: Option<String>,
    #[schemars(description = "Only tasks whose title contains this text.")]
    pub query: Option<String>,
    #[schemars(description = "How many tasks to return: default 50, at most 200.")]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TaskId {
    #[schemars(description = "The task's id.")]
    pub task_id: i64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateTask {
    #[schemars(description = "What needs doing, up to 140 characters.")]
    pub title: String,
    #[schemars(description = "Project id. Leave out for a chore with no project.")]
    pub project_id: Option<i64>,
    #[schemars(description = "todo (default), progress, blocked or done.")]
    pub status: Option<String>,
    #[schemars(description = "high, medium (default) or low.")]
    pub priority: Option<String>,
    #[schemars(description = "Due date as YYYY-MM-DD.")]
    pub due_on: Option<String>,
    #[schemars(description = "Usernames to assign (\"me\" for yourself).")]
    pub assignees: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateTask {
    #[schemars(description = "The task's id.")]
    pub task_id: i64,
    #[schemars(description = "New title, up to 140 characters.")]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "present")]
    #[schemars(description = "Move to this project id, or null to make it a chore.")]
    pub project_id: Option<Option<i64>>,
    #[schemars(description = "todo, progress, blocked or done.")]
    pub status: Option<String>,
    #[schemars(description = "high, medium or low.")]
    pub priority: Option<String>,
    #[serde(default, deserialize_with = "present")]
    #[schemars(description = "Due date as YYYY-MM-DD, or null to clear it.")]
    pub due_on: Option<Option<String>>,
    #[schemars(
        description = "Replaces the whole list of assignees with these usernames (\"me\" for yourself). Use [] to unassign everyone. Leave out to keep the current assignees."
    )]
    pub assignees: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddTaskNote {
    #[schemars(description = "The task's id.")]
    pub task_id: i64,
    #[schemars(
        description = "The note, up to 2,000 characters. @username mentions notify people."
    )]
    pub body: String,
}

/// Why a tool call did not go through.
enum Fail {
    /// Something the caller can fix; returned as a tool error.
    Refused(String),
    /// Our fault; logged and returned as an internal error.
    Broken(String),
}

impl From<ModelError> for Fail {
    fn from(err: ModelError) -> Self {
        if let Some(fields) = field_errors(&err) {
            let message = fields
                .into_iter()
                .map(|(field, message)| format!("{}: {message}", tool_field(&field)))
                .collect::<Vec<_>>()
                .join(" ");
            return Self::Refused(message);
        }
        match err {
            ModelError::EntityNotFound => Self::Refused("Not found in your organisation.".into()),
            other => Self::Broken(other.to_string()),
        }
    }
}

impl From<loco_rs::Error> for Fail {
    fn from(err: loco_rs::Error) -> Self {
        Self::Broken(err.to_string())
    }
}

/// Form field names as the tools call them.
fn tool_field(field: &str) -> &str {
    match field {
        "assignee_ids" => "assignees",
        "owner_id" => "owner",
        other => other,
    }
}

type Outcome = Result<Value, Fail>;

fn finish(outcome: Outcome) -> Result<CallToolResult, ErrorData> {
    match outcome {
        Ok(value) => Ok(CallToolResult::success(vec![ContentBlock::text(
            value.to_string(),
        )])),
        Err(Fail::Refused(message)) => Ok(CallToolResult::error(vec![ContentBlock::text(message)])),
        Err(Fail::Broken(err)) => {
            tracing::error!(error = %err, "MCP tool failed");
            Err(ErrorData::internal_error("Something went wrong.", None))
        }
    }
}

fn member(parts: &Parts) -> Result<CurrentMember, Fail> {
    parts
        .extensions
        .get::<CurrentMember>()
        .cloned()
        .ok_or_else(|| Fail::Broken("no member on an /mcp request".into()))
}

/// The team as `(user_id, username)`, for turning usernames into ids and back.
struct Team(Vec<(i64, String)>);

impl Team {
    async fn load(ctx: &AppContext, member: &CurrentMember) -> Result<Self, Fail> {
        Ok(Self(
            memberships::Model::team(&ctx.db, member.org.id).await?,
        ))
    }

    fn id(&self, member: &CurrentMember, name: &str) -> Result<i64, Fail> {
        let name = name.trim().trim_start_matches('@');
        if name.eq_ignore_ascii_case("me") {
            return Ok(member.user.id);
        }
        self.0
            .iter()
            .find(|(_, username)| username.eq_ignore_ascii_case(name))
            .map(|(id, _)| *id)
            .ok_or_else(|| Fail::Refused(format!("No one called \"{name}\" is on the team.")))
    }

    fn ids(&self, member: &CurrentMember, names: &[String]) -> Result<Vec<i64>, Fail> {
        names.iter().map(|n| self.id(member, n)).collect()
    }

    fn names(&self) -> HashMap<i64, String> {
        self.0.iter().cloned().collect()
    }
}

fn task_json(
    task: &tasks::Model,
    projects: &HashMap<i64, String>,
    names: &HashMap<i64, String>,
    assignees: &[i64],
) -> Value {
    json!({
        "id": task.id,
        "title": task.title,
        "status": task.status,
        "priority": task.priority,
        "due_on": task.due_on,
        "project_id": task.project_id,
        "project": task.project_id.and_then(|id| projects.get(&id)),
        "assignees": assignees.iter().filter_map(|id| names.get(id)).collect::<Vec<_>>(),
    })
}

/// One task as the tools show it, with its project name and assignees.
async fn show_task(ctx: &AppContext, member: &CurrentMember, task: &tasks::Model) -> Outcome {
    let names = Team::load(ctx, member).await?.names();
    let projects = project_names(ctx, member).await?;
    let assignees = task_assignees::Model::for_task(&ctx.db, member.org.id, task.id).await?;
    Ok(task_json(task, &projects, &names, &assignees))
}

async fn project_names(
    ctx: &AppContext,
    member: &CurrentMember,
) -> Result<HashMap<i64, String>, Fail> {
    Ok(projects::Model::list_for_org(&ctx.db, member.org.id)
        .await?
        .into_iter()
        .map(|p| (p.id, p.name))
        .collect())
}

#[tool_router]
impl CollabServer {
    #[tool(
        description = "List every project with its lane, status, owner and progress (done/total tasks).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_projects(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        finish(self.list_projects_for(&parts).await)
    }

    #[tool(
        description = "Create a project in a roadmap lane. The owner, if any, is notified.",
        annotations(destructive_hint = false, open_world_hint = false)
    )]
    async fn create_project(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<CreateProject>,
    ) -> Result<CallToolResult, ErrorData> {
        finish(self.create_project_for(&parts, input).await)
    }

    #[tool(
        description = "List the active members of the team: username and role.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_members(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        finish(self.list_members_for(&parts).await)
    }

    #[tool(
        description = "List tasks, optionally filtered by project, status, assignee or title text.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_tasks(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<ListTasks>,
    ) -> Result<CallToolResult, ErrorData> {
        finish(self.list_tasks_for(&parts, input).await)
    }

    #[tool(
        description = "Get one task with its notes.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_task(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<TaskId>,
    ) -> Result<CallToolResult, ErrorData> {
        finish(self.get_task_for(&parts, input).await)
    }

    #[tool(
        description = "Create a task. Assignees and the project owner are notified.",
        annotations(destructive_hint = false, open_world_hint = false)
    )]
    async fn create_task(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<CreateTask>,
    ) -> Result<CallToolResult, ErrorData> {
        finish(self.create_task_for(&parts, input).await)
    }

    #[tool(
        description = "Change a task. Send only the fields to change; the rest stay as they are. `assignees` replaces the whole list.",
        annotations(destructive_hint = false, open_world_hint = false)
    )]
    async fn update_task(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<UpdateTask>,
    ) -> Result<CallToolResult, ErrorData> {
        finish(self.update_task_for(&parts, input).await)
    }

    #[tool(
        description = "Add a note to a task. @username mentions and the task's assignees are notified.",
        annotations(destructive_hint = false, open_world_hint = false)
    )]
    async fn add_task_note(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<AddTaskNote>,
    ) -> Result<CallToolResult, ErrorData> {
        finish(self.add_task_note_for(&parts, input).await)
    }
}

impl CollabServer {
    async fn list_projects_for(&self, parts: &Parts) -> Outcome {
        let member = member(parts)?;
        let db = &self.ctx.db;
        let names = Team::load(&self.ctx, &member).await?.names();
        let all_tasks = tasks::Model::list_for_org(db, member.org.id).await?;
        let projects: Vec<Value> = projects::Model::list_for_org(db, member.org.id)
            .await?
            .into_iter()
            .map(|p| {
                json!({
                    "id": p.id,
                    "name": p.name,
                    "lane": p.lane,
                    "status": p.status,
                    "summary": p.summary,
                    "owner": p.owner_id.and_then(|id| names.get(&id)),
                    "progress": Progress::of(p.id, &all_tasks),
                })
            })
            .collect();
        Ok(json!({ "projects": projects }))
    }

    async fn create_project_for(&self, parts: &Parts, input: CreateProject) -> Outcome {
        let member = member(parts)?;
        let db = &self.ctx.db;
        let team = Team::load(&self.ctx, &member).await?;
        let owner_id = match input.owner.as_deref().filter(|o| !o.trim().is_empty()) {
            Some(name) => team.id(&member, name)?.to_string(),
            None => String::new(),
        };
        let count = projects::Model::list_for_org(db, member.org.id)
            .await?
            .len();
        let params = ProjectParams {
            name: input.name,
            lane: input.lane.trim().to_lowercase(),
            status: input.status,
            accent: ACCENTS[count % ACCENTS.len()].to_string(),
            owner_id,
            summary: input.summary.unwrap_or_default(),
        };
        let project = projects::Model::create(db, member.org.id, &params).await?;
        notifications::project_saved(&self.ctx, &member, None, &project).await?;
        Ok(json!({
            "id": project.id,
            "name": project.name,
            "lane": project.lane,
            "status": project.status,
            "summary": project.summary,
            "owner": project.owner_id.and_then(|id| team.names().remove(&id)),
        }))
    }

    async fn list_members_for(&self, parts: &Parts) -> Outcome {
        let member = member(parts)?;
        let members: Vec<Value> = memberships::Model::list_for_org(&self.ctx.db, member.org.id)
            .await?
            .into_iter()
            .filter(|(m, _)| m.is_active())
            .map(|(m, _)| {
                json!({ "username": m.username, "role": m.role, "you": m.user_id == member.user.id })
            })
            .collect();
        Ok(json!({ "members": members }))
    }

    async fn list_tasks_for(&self, parts: &Parts, input: ListTasks) -> Outcome {
        let member = member(parts)?;
        let db = &self.ctx.db;
        if let Some(status) = &input.status {
            if !tasks::is_status(status) {
                return Err(Fail::Refused(
                    "status must be todo, progress, blocked or done.".into(),
                ));
            }
        }
        let team = Team::load(&self.ctx, &member).await?;
        let assignee = match &input.assignee {
            Some(name) => Some(team.id(&member, name)?),
            None => None,
        };
        let query = input.query.as_deref().map(str::to_lowercase);
        let limit = input.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let names = team.names();
        let projects = project_names(&self.ctx, &member).await?;
        let assignees = task_assignees::Model::by_task(db, member.org.id).await?;
        let none = Vec::new();
        let matching: Vec<Value> = tasks::Model::list_for_org(db, member.org.id)
            .await?
            .iter()
            .filter(|t| input.project_id.is_none_or(|id| t.project_id == Some(id)))
            .filter(|t| input.status.as_ref().is_none_or(|s| &t.status == s))
            .filter(|t| {
                assignee.is_none_or(|id| assignees.get(&t.id).is_some_and(|a| a.contains(&id)))
            })
            .filter(|t| {
                query
                    .as_ref()
                    .is_none_or(|q| t.title.to_lowercase().contains(q))
            })
            .map(|t| task_json(t, &projects, &names, assignees.get(&t.id).unwrap_or(&none)))
            .collect();
        let more = matching.len() > limit;
        Ok(json!({
            "tasks": matching.into_iter().take(limit).collect::<Vec<_>>(),
            "more": more,
        }))
    }

    async fn get_task_for(&self, parts: &Parts, input: TaskId) -> Outcome {
        let member = member(parts)?;
        let task = tasks::Model::find_in_org(&self.ctx.db, member.org.id, input.task_id).await?;
        let mut shown = show_task(&self.ctx, &member, &task).await?;
        let names = Team::load(&self.ctx, &member).await?.names();
        let notes: Vec<Value> = task_notes::Model::list_for_task(&self.ctx.db, &task)
            .await?
            .into_iter()
            .map(|n| {
                json!({
                    "author": n.author_id.and_then(|id| names.get(&id)),
                    "body": n.body,
                    "at": time::local(n.created_at, member.tz()).to_rfc3339(),
                })
            })
            .collect();
        shown["notes"] = json!(notes);
        Ok(shown)
    }

    async fn create_task_for(&self, parts: &Parts, input: CreateTask) -> Outcome {
        let member = member(parts)?;
        let team = Team::load(&self.ctx, &member).await?;
        let params = TaskParams {
            title: input.title,
            project_id: input
                .project_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
            assignee_ids: team.ids(&member, &input.assignees.unwrap_or_default())?,
            priority: input.priority.unwrap_or_else(|| "medium".to_string()),
            due_on: input.due_on.unwrap_or_default(),
            status: input.status.unwrap_or_default(),
        };
        let task = tasks::Model::create(&self.ctx.db, member.org.id, &params).await?;
        notifications::task_saved(&self.ctx, &member, None, &task).await?;
        show_task(&self.ctx, &member, &task).await
    }

    async fn update_task_for(&self, parts: &Parts, input: UpdateTask) -> Outcome {
        let member = member(parts)?;
        let db = &self.ctx.db;
        let task = tasks::Model::find_in_org(db, member.org.id, input.task_id).await?;
        let current = task_assignees::Model::for_task(db, member.org.id, task.id).await?;
        let assignee_ids = match &input.assignees {
            Some(names) => Team::load(&self.ctx, &member).await?.ids(&member, names)?,
            None => current.clone(),
        };
        let project_id = input.project_id.unwrap_or(task.project_id);
        let due_on = match input.due_on {
            Some(due) => due.unwrap_or_default(),
            None => task.due_on.map(|d| d.to_string()).unwrap_or_default(),
        };
        let params = TaskParams {
            title: input.title.unwrap_or_else(|| task.title.clone()),
            project_id: project_id.map(|id| id.to_string()).unwrap_or_default(),
            assignee_ids,
            priority: input.priority.unwrap_or_else(|| task.priority.clone()),
            due_on,
            status: input.status.unwrap_or_else(|| task.status.clone()),
        };
        let before = TaskBefore {
            assignees: current,
            status: task.status.clone(),
        };
        let task = task.update_from(db, &params).await?;
        notifications::task_saved(&self.ctx, &member, Some(before), &task).await?;
        show_task(&self.ctx, &member, &task).await
    }

    async fn add_task_note_for(&self, parts: &Parts, input: AddTaskNote) -> Outcome {
        let member = member(parts)?;
        let task = tasks::Model::find_in_org(&self.ctx.db, member.org.id, input.task_id).await?;
        let note = task_notes::Model::create(
            &self.ctx.db,
            &task,
            member.user.id,
            &NoteParams { body: input.body },
        )
        .await?;
        notifications::note_added(&self.ctx, &member, &task, &note).await?;
        Ok(json!({
            "id": note.id,
            "task_id": task.id,
            "author": member.username,
            "body": note.body,
            "at": time::local(note.created_at, member.tz()).to_rfc3339(),
        }))
    }
}

#[tool_handler]
impl ServerHandler for CollabServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("Collab Tool", env!("CARGO_PKG_VERSION"))
                    .with_title("Collab Tool"),
            )
            .with_instructions(INSTRUCTIONS)
    }
}
