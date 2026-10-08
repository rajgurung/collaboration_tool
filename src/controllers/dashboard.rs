use std::collections::HashMap;

use loco_rs::prelude::*;

use crate::{
    controllers::chat,
    extractors::current_member::CurrentMember,
    models::{guide, meetings, memberships, projects, task_assignees, tasks},
    views::layout::avatar_color,
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
            let project = t
                .project_id
                .and_then(|id| project_names.get(&id).copied())
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
            let accent = all_projects
                .iter()
                .find(|p| Some(p.id) == t.project_id)
                .map_or("#9a968d", |p| p.accent.as_str());
            serde_json::json!({ "id": t.id, "title": t.title, "status": t.status, "meta": meta, "accent": accent })
        })
        .collect();

    let blocked = all_tasks.iter().filter(|t| t.status == "blocked").count();
    let week_end = chrono::Utc::now().date_naive() + chrono::Days::new(7);
    let due_this_week = all_tasks
        .iter()
        .filter(|t| t.status != "done" && t.due_on.is_some_and(|d| d <= week_end))
        .count();

    // The "now" lane: progress from finished tasks, stuck work, and who is on it.
    let roadmap_now: Vec<serde_json::Value> = all_projects
        .iter()
        .filter(|p| p.lane == "now")
        .map(|p| {
            let theirs: Vec<&tasks::Model> = all_tasks
                .iter()
                .filter(|t| t.project_id == Some(p.id))
                .collect();
            let stuck = theirs.iter().filter(|t| t.status == "blocked").count();
            let mut people: Vec<i64> = Vec::new();
            for id in theirs.iter().filter_map(|t| assignees.get(&t.id)).flatten() {
                if !people.contains(id) {
                    people.push(*id);
                }
            }
            let people: Vec<serde_json::Value> = people
                .iter()
                .filter_map(|id| names.get(id))
                .take(4)
                .map(|n| serde_json::json!({ "name": n, "color": avatar_color(n) }))
                .collect();
            serde_json::json!({
                "name": p.name, "progress": projects::Progress::of(p.id, &all_tasks),
                "accent": p.accent, "status": p.status,
                "blocked": stuck, "people": people,
            })
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

    // The "Getting started" guide until it's closed, and the welcome pop-up once.
    let membership = memberships::Model::active_in(&ctx.db, org_id, me).await?;
    let guide = match &membership {
        Some(m) if m.guide_dismissed_at.is_none() => {
            let steps = guide::steps(&ctx.db, org_id, me, member.can_manage()).await?;
            let done = steps.iter().filter(|s| s.done).count();
            let pct = (done * 100).checked_div(steps.len()).unwrap_or(0);
            Some(
                serde_json::json!({ "steps": steps, "done": done, "total": steps.len(), "pct": pct }),
            )
        }
        _ => None,
    };
    let welcome = membership.as_ref().is_some_and(|m| m.welcomed_at.is_none());

    format::render().view(
        &v,
        "dashboard/index.html",
        member.page(
            "dashboard",
            data!({
                "my_open": my_open,
                "blocked": blocked,
                "due_this_week": due_this_week,
                "unread": unread,
                "my_tasks": my_tasks,
                "roadmap_now": roadmap_now,
                "recent_chats": recent,
                "decision": decision,
                "guide": guide,
                "welcome": welcome,
                "today": chrono::Utc::now().format("%A %-d %B").to_string(),
            }),
        ),
    )
}

pub fn routes() -> Routes {
    Routes::new().add("/dashboard", get(index))
}
