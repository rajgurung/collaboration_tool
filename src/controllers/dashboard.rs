use std::collections::HashMap;

use loco_rs::prelude::*;

use crate::{
    controllers::chat,
    extractors::current_member::CurrentMember,
    models::{meetings, memberships, projects, task_assignees, tasks},
};

/// How many of your open tasks Home lists.
const MY_TASKS: usize = 4;
/// How many conversations Home lists.
const RECENT_CHATS: usize = 2;

#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let org_id = member.org.id;
    let me = member.user.id;
    let names: HashMap<i64, String> = memberships::Model::team(&ctx.db, org_id)
        .await?
        .into_iter()
        .collect();
    let all_tasks = tasks::Model::list_for_org(&ctx.db, org_id).await?;
    let assignees = task_assignees::Model::by_task(&ctx.db, org_id).await?;
    let all_projects = projects::Model::list_for_org(&ctx.db, org_id).await?;
    let project_names: HashMap<i64, &str> = all_projects
        .iter()
        .map(|p| (p.id, p.name.as_str()))
        .collect();

    // Your open work, blocked first.
    let rank = |s: &str| match s {
        "blocked" => 0,
        "progress" => 1,
        _ => 2,
    };
    let mut mine: Vec<&tasks::Model> = all_tasks
        .iter()
        .filter(|t| t.status != "done" && assignees.get(&t.id).is_some_and(|ids| ids.contains(&me)))
        .collect();
    mine.sort_by_key(|t| rank(&t.status));
    let my_open = mine.len();
    let my_tasks: Vec<serde_json::Value> = mine
        .iter()
        .take(MY_TASKS)
        .map(|t| {
            let project = project_names
                .get(&t.project_id)
                .copied()
                .unwrap_or_default();
            let lead = match (t.status.as_str(), t.due_on) {
                ("blocked", _) => "Blocked".to_string(),
                (_, Some(d)) => format!("Due {}", d.format("%a %-d %b")),
                _ => String::new(),
            };
            let meta = [lead.as_str(), project]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(" · ");
            serde_json::json!({ "id": t.id, "title": t.title, "status": t.status, "meta": meta })
        })
        .collect();

    let blocked = all_tasks.iter().filter(|t| t.status == "blocked").count();

    // The "now" lane, with how much of each project is stuck.
    let roadmap_now: Vec<serde_json::Value> = all_projects
        .iter()
        .filter(|p| p.lane == "now")
        .map(|p| {
            let stuck = all_tasks.iter().filter(|t| t.project_id == p.id && t.status == "blocked").count();
            serde_json::json!({ "name": p.name, "progress": p.progress, "accent": p.accent, "blocked": stuck })
        })
        .collect();

    let chats = chat::list_items(&ctx, &member, &names).await?;
    let unread: u64 = chats.iter().map(|c| c.unread).sum();
    let recent: Vec<&chat::ListItem> = chats
        .iter()
        .filter(|c| c.last.is_some())
        .take(RECENT_CHATS)
        .collect();

    let decision = meetings::Model::list_for_org(&ctx.db, org_id)
        .await?
        .into_iter()
        .find(|(m, _)| !m.decisions.trim().is_empty())
        .map(|(m, _)| {
            serde_json::json!({
                "title": m.title,
                "held_on": m.held_on.format("%a %-d %b").to_string(),
                "decisions": m.decisions,
            })
        });

    format::render().view(
        &v,
        "dashboard/index.html",
        member.page(
            "dashboard",
            data!({
                "my_open": my_open,
                "blocked": blocked,
                "unread": unread,
                "my_tasks": my_tasks,
                "roadmap_now": roadmap_now,
                "recent_chats": recent,
                "decision": decision,
                "today": chrono::Utc::now().format("%A %-d %B").to_string(),
            }),
        ),
    )
}

pub fn routes() -> Routes {
    Routes::new().add("/dashboard", get(index))
}
