use std::collections::HashMap;

use loco_rs::prelude::*;

use crate::{
    extractors::current_member::CurrentMember,
    models::{memberships, projects, tasks},
    views::layout::avatar_color,
};

/// How many open tasks the overview lists.
const OPEN_ACTIONS: usize = 4;

#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let org_id = member.org.id;
    let team = memberships::Model::team(&ctx.db, org_id).await?;
    let names: HashMap<i64, &str> = team.iter().map(|(id, n)| (*id, n.as_str())).collect();
    let all_tasks = tasks::Model::list_for_org(&ctx.db, org_id).await?;
    let all_projects = projects::Model::list_for_org(&ctx.db, org_id).await?;
    let project_names: HashMap<i64, &str> = all_projects
        .iter()
        .map(|p| (p.id, p.name.as_str()))
        .collect();
    let count = |status: &str| all_tasks.iter().filter(|t| t.status == status).count();

    let people: Vec<serde_json::Value> = team
        .iter()
        .map(|(id, username)| {
            let owned: Vec<&str> = all_tasks
                .iter()
                .filter(|t| t.owner_id == Some(*id))
                .map(|t| t.status.as_str())
                .collect();
            let n = |s: &str| owned.iter().filter(|x| **x == s).count();
            serde_json::json!({
                "username": username,
                "color": avatar_color(username),
                "score": tasks::progress_score(owned.iter().copied()),
                "total": owned.len(),
                "done": n("done"),
                "active": n("progress"),
                "blocked": n("blocked"),
            })
        })
        .collect();
    let members_with_tasks = team
        .iter()
        .filter(|(id, _)| all_tasks.iter().any(|t| t.owner_id == Some(*id)))
        .count();

    let workstreams: Vec<serde_json::Value> = all_projects
        .iter()
        .filter(|p| p.lane == "now")
        .map(|p| {
            let owner = p.owner_id.and_then(|id| names.get(&id).copied());
            serde_json::json!({
                "name": p.name,
                "progress": p.progress,
                "accent": p.accent,
                "owner": owner,
                "owner_color": owner.map_or("#e8dfce", avatar_color),
            })
        })
        .collect();

    let actions: Vec<serde_json::Value> = all_tasks
        .iter()
        .filter(|t| t.status != "done")
        .take(OPEN_ACTIONS)
        .map(|t| {
            let owner = t.owner_id.and_then(|id| names.get(&id).copied());
            serde_json::json!({
                "title": t.title,
                "color": tasks::STATUSES.iter().find(|(k, _, _)| *k == t.status).map_or("#9ca3af", |(_, _, c)| *c),
                "project": project_names.get(&t.project_id).copied().unwrap_or_default(),
                "due": t.due_on.map_or_else(|| "No date".to_string(), |d| d.format("%d %b").to_string()),
                "owner": owner,
                "owner_color": owner.map_or("#e8dfce", avatar_color),
            })
        })
        .collect();

    format::render().view(
        &v,
        "dashboard/index.html",
        member.page(
            "dashboard",
            data!({
                "completion": tasks::completion(all_tasks.iter().map(|t| t.status.as_str())),
                "in_progress": count("progress"),
                "blocked": count("blocked"),
                "members_with_tasks": members_with_tasks,
                "team_size": team.len(),
                "people": people,
                "workstreams": workstreams,
                "actions": actions,
            }),
        ),
    )
}

pub fn routes() -> Routes {
    Routes::new().add("/dashboard", get(index))
}
