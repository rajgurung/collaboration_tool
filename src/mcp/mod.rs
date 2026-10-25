//! The tools Claude can call. Each one acts as the member the bearer token
//! belongs to (put in the request by `extractors::bearer`), inside their one
//! organisation, through the same model methods and notifications as the web
//! pages. Nothing here takes an organisation id.
use std::collections::HashMap;

use axum::http::request::Parts;
use loco_rs::{app::AppContext, model::ModelError};
use rmcp::{
    handler::server::{router::tool::ToolRouter, tool::Extension, wrapper::Parameters},
    model::{
        CacheScope, CallToolResult, ContentBlock, DiscoverResult, Implementation,
        InitializeRequestParams, InitializeResult, ListToolsResult, PaginatedRequestParams,
        ProtocolVersion, ResultType, ServerCapabilities, ServerConfig,
    },
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router, ErrorData, RoleServer, ServerHandler,
};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};

use crate::{
    controllers::notifications::{self, TaskBefore},
    data::notify_hub::NotifyHub,
    extractors::{bearer::McpConnection, current_member::CurrentMember},
    models::{
        claude_update_notices, memberships,
        projects::{self, Progress, ProjectParams, ACCENTS},
        task_assignees,
        task_notes::{self, NoteParams},
        tasks::{self, Refusal, Removed, TaskParams},
    },
    views::{forms::field_errors, links::is_slug, time},
};

const INSTRUCTIONS: &str = "Collab Tool is a small team's workspace. Tasks usually belong to a \
project; a task with no project is a chore. Projects sit in roadmap lanes: now, next or later. \
A project can be marked done in the web app; list_projects shows it as closed, a closed project \
is not current work whatever its lane, and it takes no new tasks. Task statuses are todo, \
progress, blocked and done; priorities are high, medium and low. A \
project's status is free text such as \"Active\" or \"Planned\". People are referred to by \
username; \"me\" means the person you are acting for. Changes notify teammates exactly as they \
would in the web app.";
/// Tools that change something. Each takes `organisation`.
const WRITE_TOOLS: [&str; 6] = [
    "create_project",
    "create_task",
    "update_task",
    "add_task_note",
    "delete_task",
    "delete_project",
];
/// The brake on bulk creates, per connection per minute. A guard against a
/// run in the wrong organisation, not a security control.
const PROJECTS_PER_MINUTE: u64 = 5;
const TASKS_PER_MINUTE: u64 = 60;
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
    #[schemars(
        required,
        description = "Slug of the organisation this change is for. It must be the connected one, named in this tool's description."
    )]
    pub organisation: Option<String>,
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
    #[schemars(
        required,
        description = "Slug of the organisation this change is for. It must be the connected one, named in this tool's description."
    )]
    pub organisation: Option<String>,
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
    #[schemars(
        required,
        description = "Slug of the organisation this change is for. It must be the connected one, named in this tool's description."
    )]
    pub organisation: Option<String>,
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
    #[schemars(
        required,
        description = "Slug of the organisation this change is for. It must be the connected one, named in this tool's description."
    )]
    pub organisation: Option<String>,
    #[schemars(description = "The task's id.")]
    pub task_id: i64,
    #[schemars(
        description = "The note, up to 2,000 characters. @username mentions notify people; @everyone does not notify anyone from here."
    )]
    pub body: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeleteTask {
    #[schemars(
        required,
        description = "Slug of the organisation this change is for. It must be the connected one, named in this tool's description."
    )]
    pub organisation: Option<String>,
    #[schemars(description = "The task's id.")]
    pub task_id: i64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeleteProject {
    #[schemars(
        required,
        description = "Slug of the organisation this change is for. It must be the connected one, named in this tool's description."
    )]
    pub organisation: Option<String>,
    #[schemars(description = "The project's id.")]
    pub project_id: i64,
}

/// Why a tool call did not go through.
enum Fail {
    /// Something the caller can fix; returned as a tool error.
    Refused(String),
    /// No such item in the connected organisation; a tool error naming it.
    NotFound,
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
            ModelError::EntityNotFound => Self::NotFound,
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

/// Turns a read tool's outcome into its result. Every success names the
/// connected organisation, and carries the update notice while it is due.
fn finish(parts: &Parts, outcome: Outcome) -> Result<CallToolResult, ErrorData> {
    result(parts, outcome, true)
}

fn result(parts: &Parts, outcome: Outcome, notice: bool) -> Result<CallToolResult, ErrorData> {
    let member = parts.extensions.get::<CurrentMember>();
    match outcome {
        Ok(mut value) => {
            if let (Some(member), Some(object)) = (member, value.as_object_mut()) {
                object.insert(
                    "organisation".into(),
                    json!({ "name": clean_name(&member.org.name), "slug": member.org.slug }),
                );
                if notice
                    && parts
                        .extensions
                        .get::<McpConnection>()
                        .is_some_and(|c| c.update_notice)
                {
                    object.insert("notice".into(), json!(update_notice(member)));
                }
            }
            Ok(CallToolResult::success(vec![ContentBlock::text(
                value.to_string(),
            )]))
        }
        Err(Fail::Refused(message)) => Ok(CallToolResult::error(vec![ContentBlock::text(message)])),
        Err(Fail::NotFound) => {
            let place = member.map_or_else(|| "this organisation".to_string(), org_label);
            Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "Not found in {place}."
            ))]))
        }
        Err(Fail::Broken(err)) => {
            tracing::error!(error = %err, "MCP tool failed");
            Err(ErrorData::internal_error("Something went wrong.", None))
        }
    }
}

/// Unicode format characters (category Cf): invisible, and able to make
/// text read differently from what it is.
fn is_format(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{0600}'..='\u{0605}' | '\u{061C}' | '\u{06DD}' | '\u{070F}'
        | '\u{0890}'..='\u{0891}' | '\u{08E2}' | '\u{180E}' | '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}'
        | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}' | '\u{110BD}' | '\u{110CD}'
        | '\u{13430}'..='\u{1343F}' | '\u{1BCA0}'..='\u{1BCA3}' | '\u{1D173}'..='\u{1D17A}'
        | '\u{E0001}' | '\u{E0020}'..='\u{E007F}')
}

/// An organisation name as it may reach the model: one line, no invisible
/// characters, no double quotes (it is shown inside them), at most 80
/// characters. Owners choose names, so they are treated like any input.
fn clean_name(name: &str) -> String {
    let spaced: String = name
        .chars()
        .filter(|c| !is_format(*c))
        .map(|c| match c {
            '"' => '\'',
            c if c.is_control() || c.is_whitespace() => ' ',
            c => c,
        })
        .collect();
    spaced
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(80)
        .collect()
}

/// The connected organisation as every message names it: `"Name" (slug)`.
fn org_label(member: &CurrentMember) -> String {
    format!("\"{}\" ({})", clean_name(&member.org.name), member.org.slug)
}

/// The server's instructions for this connection: which organisation it
/// reaches, then the fixed text.
fn instructions(member: &CurrentMember) -> String {
    let (org, slug) = (org_label(member), &member.org.slug);
    format!(
        "Connected to {org} as {}. Every tool reads and writes only there. Write \
tools need `organisation: \"{slug}\"`. If the user means another organisation, stop and tell \
them this connection can't reach it; they can connect again and pick that organisation. \
{INSTRUCTIONS}",
        member.username
    )
}

/// The member for a protocol request (not a tool call). Never missing on
/// `/mcp`; if it is, the request fails rather than answering without an org.
fn context_member(context: &RequestContext<RoleServer>) -> Result<CurrentMember, ErrorData> {
    context
        .extensions
        .get::<Parts>()
        .and_then(|parts| parts.extensions.get::<CurrentMember>())
        .cloned()
        .ok_or_else(|| {
            tracing::error!("no member on an /mcp request");
            ErrorData::internal_error("Something went wrong.", None)
        })
}

fn member(parts: &Parts) -> Result<CurrentMember, Fail> {
    parts
        .extensions
        .get::<CurrentMember>()
        .cloned()
        .ok_or_else(|| Fail::Broken("no member on an /mcp request".into()))
}

/// The connection behind the request. Never missing on `/mcp`; if it is,
/// creates and deletes fail rather than treat the call as the web app's.
fn connection(parts: &Parts) -> Result<McpConnection, Fail> {
    parts
        .extensions
        .get::<McpConnection>()
        .copied()
        .ok_or_else(|| Fail::Broken("no connection on an /mcp request".into()))
}

fn a_minute_ago() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() - chrono::Duration::seconds(60)
}

/// The brake's refusal, naming the organisation so the run can be checked.
fn paused(member: &CurrentMember, limit: u64, what: &str) -> Fail {
    Fail::Refused(format!(
        "Paused: this connection has made {limit} {what} in {} in the last minute. Confirm \
with the user that this is the right organisation and that they want more, then try again in a \
minute.",
        org_label(member)
    ))
}

/// What a connection made before this release is told until it reloads its
/// tools.
fn update_notice(member: &CurrentMember) -> String {
    format!(
        "Tell the user once: the Collab connector was updated. Each write now names its \
organisation (this connection is {}). Restart Claude, or reconnect the connector, to load \
the new tools.",
        org_label(member)
    )
}

/// Refuses a write whose `organisation` isn't the connected one. Only ever
/// refuses: the organisation always comes from the token. No database call.
fn check_org(member: &CurrentMember, given: Option<&str>) -> Result<(), Fail> {
    let (org, slug) = (org_label(member), &member.org.slug);
    let given = given
        .map(|g| g.trim().to_ascii_lowercase())
        .unwrap_or_default();
    if given.is_empty() {
        return Err(Fail::Refused(format!(
            "Collab was updated. Tell the user: writes now name their organisation. This \
connection is {org}. Retry with organisation: \"{slug}\", and restart Claude to \
refresh its tools."
        )));
    }
    if &given == slug {
        return Ok(());
    }
    // Echo only slug-shaped input, so no free text reaches the model.
    let other = if is_slug(&given) {
        given
    } else {
        "a different organisation".to_string()
    };
    Err(Fail::Refused(format!(
        "This connection is {org}, not {other}. Nothing was changed. To work in \
{other}, the user needs a connection for that organisation: connect again and pick it."
    )))
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
async fn show_task(
    ctx: &AppContext,
    member: &CurrentMember,
    team: &Team,
    task: &tasks::Model,
) -> Outcome {
    let names = team.names();
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
        description = "List every project with its lane, status, owner, progress (done/total tasks), and whether it is closed (marked done).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_projects(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        finish(&parts, self.list_projects_for(&parts).await)
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
        self.finish_write(&parts, self.create_project_for(&parts, input).await)
            .await
    }

    #[tool(
        description = "List the active members of the team: username and role.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_members(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        finish(&parts, self.list_members_for(&parts).await)
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
        finish(&parts, self.list_tasks_for(&parts, input).await)
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
        finish(&parts, self.get_task_for(&parts, input).await)
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
        self.finish_write(&parts, self.create_task_for(&parts, input).await)
            .await
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
        self.finish_write(&parts, self.update_task_for(&parts, input).await)
            .await
    }

    #[tool(
        description = "Add a note to a task. @username mentions and the task's assignees are notified; @everyone does not notify anyone from here.",
        annotations(destructive_hint = false, open_world_hint = false)
    )]
    async fn add_task_note(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<AddTaskNote>,
    ) -> Result<CallToolResult, ErrorData> {
        self.finish_write(&parts, self.add_task_note_for(&parts, input).await)
            .await
    }

    #[tool(
        description = "Delete a task this connection created in the last 24 hours, to undo a mistake. Its notes and assignees go with it. Anything else must be deleted in Collab Tool.",
        annotations(destructive_hint = true, open_world_hint = false)
    )]
    async fn delete_task(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<DeleteTask>,
    ) -> Result<CallToolResult, ErrorData> {
        self.finish_write(&parts, self.delete_task_for(&parts, input).await)
            .await
    }

    #[tool(
        description = "Delete a project this connection created in the last 24 hours, with its tasks, to undo a mistake. Refused if anyone else has added tasks or notes to it.",
        annotations(destructive_hint = true, open_world_hint = false)
    )]
    async fn delete_project(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(input): Parameters<DeleteProject>,
    ) -> Result<CallToolResult, ErrorData> {
        self.finish_write(&parts, self.delete_project_for(&parts, input).await)
            .await
    }
}

impl CollabServer {
    /// The member and connection for a write, once its `organisation` is the
    /// connected one.
    fn writer(
        parts: &Parts,
        organisation: Option<&str>,
    ) -> Result<(CurrentMember, McpConnection), Fail> {
        let member = member(parts)?;
        let connection = connection(parts)?;
        check_org(&member, organisation)?;
        Ok((member, connection))
    }

    /// A write tool's result, without the update notice. A write that went
    /// through named its organisation, so this connection has the new tools:
    /// that ends the notice. A refused or invalid write leaves it.
    async fn finish_write(
        &self,
        parts: &Parts,
        outcome: Outcome,
    ) -> Result<CallToolResult, ErrorData> {
        if let (Ok(_), Some(connection)) = (&outcome, parts.extensions.get::<McpConnection>()) {
            if connection.update_notice {
                // The change is saved; failing to record it only keeps the
                // notice a little longer.
                if let Err(err) =
                    claude_update_notices::Model::written(&self.ctx.db, connection.grant_id).await
                {
                    tracing::error!(error = %err, "could not end the update notice");
                }
            }
        }
        result(parts, outcome, false)
    }

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
                    "closed": p.is_closed(),
                    "closed_at": p.closed_at.map(|at| time::local(at, member.tz()).to_rfc3339()),
                })
            })
            .collect();
        Ok(json!({ "projects": projects }))
    }

    async fn create_project_for(&self, parts: &Parts, input: CreateProject) -> Outcome {
        let (member, connection) = Self::writer(parts, input.organisation.as_deref())?;
        let db = &self.ctx.db;
        let recent =
            projects::Model::count_made_by(db, connection.grant_id, a_minute_ago()).await?;
        if recent >= PROJECTS_PER_MINUTE {
            return Err(paused(&member, PROJECTS_PER_MINUTE, "projects"));
        }
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
        let project =
            projects::Model::create(db, member.org.id, &params, Some(connection.grant_id)).await?;
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
        let team = Team::load(&self.ctx, &member).await?;
        let mut shown = show_task(&self.ctx, &member, &team, &task).await?;
        let names = team.names();
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
        let (member, connection) = Self::writer(parts, input.organisation.as_deref())?;
        let recent =
            tasks::Model::count_made_by(&self.ctx.db, connection.grant_id, a_minute_ago()).await?;
        if recent >= TASKS_PER_MINUTE {
            return Err(paused(&member, TASKS_PER_MINUTE, "tasks"));
        }
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
        let task = tasks::Model::create(
            &self.ctx.db,
            member.org.id,
            &params,
            Some(connection.grant_id),
        )
        .await?;
        notifications::task_saved(&self.ctx, &member, None, &task).await?;
        show_task(&self.ctx, &member, &team, &task).await
    }

    async fn update_task_for(&self, parts: &Parts, input: UpdateTask) -> Outcome {
        let (member, _) = Self::writer(parts, input.organisation.as_deref())?;
        let db = &self.ctx.db;
        let task = tasks::Model::find_in_org(db, member.org.id, input.task_id).await?;
        let current = task_assignees::Model::for_task(db, member.org.id, task.id).await?;
        let team = Team::load(&self.ctx, &member).await?;
        let assignee_ids = match &input.assignees {
            Some(names) => team.ids(&member, names)?,
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
        show_task(&self.ctx, &member, &team, &task).await
    }

    async fn add_task_note_for(&self, parts: &Parts, input: AddTaskNote) -> Outcome {
        let (member, _) = Self::writer(parts, input.organisation.as_deref())?;
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

    async fn delete_task_for(&self, parts: &Parts, input: DeleteTask) -> Outcome {
        let (member, connection) = Self::writer(parts, input.organisation.as_deref())?;
        let removed = tasks::Model::remove_made_by(
            &self.ctx.db,
            member.org.id,
            input.task_id,
            connection.grant_id,
            member.user.id,
        )
        .await?
        .map_err(|refusal| {
            Fail::Refused(
                match refusal {
                    Refusal::NotMadeHere => "Only tasks this connection created in the last 24 hours can be deleted from Claude. Ask the user to delete it in Collab Tool.",
                    Refusal::OthersTasks | Refusal::OthersContent => "Someone else has added notes or reactions to this task, so it can't be deleted from Claude.",
                }
                .into(),
            )
        })?;
        self.after_delete(&member, &connection, "task", &removed);
        Ok(json!({ "deleted": { "id": removed.id, "title": removed.name } }))
    }

    async fn delete_project_for(&self, parts: &Parts, input: DeleteProject) -> Outcome {
        let (member, connection) = Self::writer(parts, input.organisation.as_deref())?;
        let removed = projects::Model::remove_made_by(
            &self.ctx.db,
            member.org.id,
            input.project_id,
            connection.grant_id,
            member.user.id,
        )
        .await?
        .map_err(|refusal| {
            Fail::Refused(
                match refusal {
                    Refusal::NotMadeHere => "Only projects this connection created in the last 24 hours can be deleted from Claude. Ask the user to delete it in Collab Tool.",
                    Refusal::OthersTasks => "Someone else has tasks in this project, so it can't be deleted from Claude. Ask the user to delete it in Collab Tool.",
                    Refusal::OthersContent => "Someone else has added notes or reactions to tasks in this project, so it can't be deleted from Claude. Ask the user to delete it in Collab Tool.",
                }
                .into(),
            )
        })?;
        self.after_delete(&member, &connection, "project", &removed);
        Ok(json!({
            "deleted": { "id": removed.id, "name": removed.name, "tasks": removed.tasks }
        }))
    }

    /// The log is the only record once the rows are gone. The bells of people
    /// whose notifications went with it refresh.
    fn after_delete(
        &self,
        member: &CurrentMember,
        connection: &McpConnection,
        what: &str,
        removed: &Removed,
    ) {
        tracing::info!(
            user_id = member.user.id,
            org_id = member.org.id,
            what,
            id = removed.id,
            tasks = removed.tasks,
            acting = member.acting,
            grant_id = %connection.grant_id,
            "Claude deleted a {what}"
        );
        if let Some(hub) = self.ctx.shared_store.get::<NotifyHub>() {
            hub.publish(member.org.id, &removed.notified);
        }
    }
}

#[tool_handler]
impl ServerHandler for CollabServer {
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        let member = context_member(&context)?;
        context.peer.set_peer_info(request.clone());
        let mut result = self.negotiate_initialize(&request)?;
        result.instructions = Some(instructions(&member));
        Ok(result)
    }

    async fn discover(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<DiscoverResult, ErrorData> {
        let member = context_member(&context)?;
        // `from_server_info` keeps rmcp's private cache scope: the text differs
        // per connection.
        let mut result = DiscoverResult::from_server_info(
            self.supported_protocol_versions().into_owned(),
            self.get_info(),
        );
        result.instructions = Some(instructions(&member));
        Ok(result)
    }

    /// The generated list, with the connected org named in each write tool.
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let member = context_member(&context)?;
        let supports_cache_hints = context
            .protocol_version()
            .is_some_and(|version| version >= ProtocolVersion::V_2026_07_28);
        let mut tools = self.tool_router.list_all();
        for tool in &mut tools {
            if WRITE_TOOLS.contains(&tool.name.as_ref()) {
                let description = tool.description.as_deref().unwrap_or_default();
                tool.description = Some(
                    format!(
                        "{description} Connected to {}: pass organisation \"{}\".",
                        org_label(&member),
                        member.org.slug
                    )
                    .into(),
                );
            }
        }
        // The descriptions differ per connection, so no shared cache may
        // hold them.
        Ok(ListToolsResult {
            result_type: Some(ResultType::COMPLETE),
            tools,
            meta: None,
            next_cursor: None,
            ttl_ms: supports_cache_hints.then_some(0),
            cache_scope: supports_cache_hints.then_some(CacheScope::Private),
        })
    }

    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("Collab Tool", env!("CARGO_PKG_VERSION"))
                    .with_title("Collab Tool"),
            )
            .with_instructions(INSTRUCTIONS)
    }
}

#[cfg(test)]
mod tests {
    use axum::http::Request;
    use loco_rs::testing::prelude::*;
    use sea_orm::{EntityTrait, PaginatorTrait};
    use serial_test::serial;

    use super::{CollabServer, CreateTask, CurrentMember, DeleteTask, Fail};
    use crate::{
        app::App,
        models::{
            memberships,
            organisations::{self, SignupParams},
            tasks::{self, TaskParams},
        },
    };

    /// The bearer gate always adds the connection. If it ever didn't, creates
    /// and deletes must fail, never act as the web app (a NULL grant).
    #[tokio::test]
    #[serial]
    async fn no_connection_means_no_delete_and_no_create() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = boot.app_context;
        let (user, org) = organisations::Model::sign_up(
            &ctx.db,
            &SignupParams {
                organisation_name: "Acme".to_string(),
                name: "alice".to_string(),
                email: "alice@example.com".to_string(),
                password: "correct-horse".to_string(),
            },
        )
        .await
        .unwrap();
        let membership = memberships::Model::active_in(&ctx.db, org.id, user.id)
            .await
            .unwrap()
            .unwrap();
        let (mut parts, ()) = Request::new(()).into_parts();
        parts.extensions.insert(CurrentMember::from_membership(
            user,
            org.clone(),
            membership,
        ));
        let params = |title: &str| TaskParams {
            title: title.to_string(),
            project_id: String::new(),
            assignee_ids: vec![],
            priority: "medium".to_string(),
            due_on: String::new(),
            status: String::new(),
        };
        let web = tasks::Model::create(&ctx.db, org.id, &params("Web"), None)
            .await
            .unwrap();
        let server = CollabServer::new(ctx.clone());

        let deleted = server
            .delete_task_for(
                &parts,
                DeleteTask {
                    organisation: Some(org.slug.clone()),
                    task_id: web.id,
                },
            )
            .await;
        assert!(matches!(deleted, Err(Fail::Broken(_))));
        let created = server
            .create_task_for(
                &parts,
                CreateTask {
                    organisation: Some(org.slug.clone()),
                    title: "From Claude".to_string(),
                    project_id: None,
                    status: None,
                    priority: None,
                    due_on: None,
                    assignees: None,
                },
            )
            .await;
        assert!(matches!(created, Err(Fail::Broken(_))));
        assert_eq!(tasks::Entity::find().count(&ctx.db).await.unwrap(), 1);
        assert!(tasks::Entity::find_by_id(web.id)
            .one(&ctx.db)
            .await
            .unwrap()
            .is_some());
    }
}
