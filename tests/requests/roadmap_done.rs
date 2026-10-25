//! Done projects: a closed project leaves its lane everywhere, closing and
//! reopening, the refusals around it, the notices, and the Home donut.
use collab::{
    app::App,
    models::{
        projects,
        tasks::{self, TaskParams},
    },
};
use loco_rs::{app::AppContext, testing::prelude::*};
use sea_orm::{EntityTrait, PaginatorTrait};
use serde_json::json;
use serial_test::serial;

use super::{
    mcp::{token_for, tool},
    prepare_data::{sign_up_member, AsMember, TestMember},
    tasks::{project_in, task_form},
};

const REFUSAL: &str = "Website is done. Reopen the project first.";

async fn reload(ctx: &AppContext, t: &tasks::Model) -> tasks::Model {
    tasks::Entity::find_by_id(t.id)
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap()
}

/// Adds a task through the model, as the web form would.
pub(super) async fn task_in(
    ctx: &AppContext,
    org_id: i64,
    project_id: Option<i64>,
    title: &str,
    status: &str,
    assignees: &[i64],
) -> tasks::Model {
    tasks::Model::create(
        &ctx.db,
        org_id,
        &TaskParams {
            title: title.to_string(),
            project_id: project_id.map(|id| id.to_string()).unwrap_or_default(),
            assignee_ids: assignees.to_vec(),
            priority: "medium".to_string(),
            due_on: String::new(),
            status: status.to_string(),
        },
        None,
    )
    .await
    .unwrap()
}

/// Closes a project through the model, as the owner.
async fn close(ctx: &AppContext, owner: &TestMember, project: &projects::Model) {
    let user = owner_id(ctx, owner).await;
    projects::Model::close(&ctx.db, owner.org_id, project.id, user)
        .await
        .unwrap();
}

async fn owner_id(ctx: &AppContext, owner: &TestMember) -> i64 {
    collab::models::memberships::Model::owners(&ctx.db, owner.org_id)
        .await
        .unwrap()
        .first()
        .unwrap()
        .0
        .user_id
}

async fn get(request: &loco_rs::TestServer, member: &TestMember, path: &str) -> String {
    let res = request
        .get(&member.at(path))
        .add_header(member.0.clone(), member.1.clone())
        .await;
    assert_eq!(res.status_code(), 200, "{path}: {}", res.text());
    res.text()
}

/// The `<select name="project_id">` of a task form.
fn picker(html: &str) -> &str {
    let start = html.find(r#"name="project_id""#).unwrap();
    let end = start + html[start..].find("</select>").unwrap();
    &html[start..end]
}

#[tokio::test]
#[serial]
async fn home_now_skips_closed_projects() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let website = project_in(&ctx, "juniper-tea-co", "Website").await;
        project_in(&ctx, "juniper-tea-co", "Spring tea range").await;
        close(&ctx, &owner, &website).await;

        let home = get(&request, &owner, "/dashboard").await;
        assert!(home.contains("Spring tea range"));
        assert!(
            !home.contains("Website"),
            "a closed project is not current work"
        );

        let roadmap = get(&request, &owner, "/roadmap?lane=now").await;
        let now = roadmap.split(r#"data-lane="now""#).nth(1).unwrap();
        let now = now.split("data-lane=").next().unwrap();
        assert!(now.contains("Spring tea range"));
        assert!(!now.contains("Website"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_task_picker_skips_closed_projects() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let website = project_in(&ctx, "juniper-tea-co", "Website").await;
        project_in(&ctx, "juniper-tea-co", "Spring tea range").await;
        let task = task_in(
            &ctx,
            owner.org_id,
            Some(website.id),
            "Returns page",
            "todo",
            &[],
        )
        .await;
        close(&ctx, &owner, &website).await;

        let new = get(&request, &owner, "/tasks/new").await;
        assert!(picker(&new).contains("Spring tea range"));
        assert!(!picker(&new).contains("Website"));

        // A task already in it keeps it, under "Done".
        let edit = get(&request, &owner, &format!("/tasks/{}/edit", task.id)).await;
        let select = picker(&edit);
        let done = select.split(r#"<optgroup label="Done">"#).nth(1).unwrap();
        assert!(done.contains(&format!(
            r#"<option value="{}" selected>Website"#,
            website.id
        )));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn new_task_from_an_old_add_button_starts_blank() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let website = project_in(&ctx, "juniper-tea-co", "Website").await;
        close(&ctx, &owner, &website).await;

        let new = get(
            &request,
            &owner,
            &format!("/tasks/new?status=todo&project_id={}", website.id),
        )
        .await;
        let select = picker(&new);
        assert!(!select.contains("Website"));
        assert!(!select.contains("selected"), "{select}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_board_hides_a_fully_done_closed_project() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let website = project_in(&ctx, "juniper-tea-co", "Website").await;
        let spring = project_in(&ctx, "juniper-tea-co", "Spring tea range").await;
        let returns = task_in(
            &ctx,
            owner.org_id,
            Some(website.id),
            "Returns page",
            "todo",
            &[],
        )
        .await;
        task_in(
            &ctx,
            owner.org_id,
            Some(spring.id),
            "Order tins",
            "todo",
            &[],
        )
        .await;
        close(&ctx, &owner, &website).await;

        let lane = format!(r#"data-lane="project-{}""#, website.id);
        for path in [
            "/tasks/board?group=project",
            "/tasks/board?group=project&view=list",
        ] {
            let board = get(&request, &owner, path).await;
            assert!(board.contains("Order tins"));
            assert!(!board.contains(&lane), "{path}");
        }
        // Still under Person and None.
        let none = get(&request, &owner, "/tasks/board?group=none").await;
        assert!(none.contains("Returns page"));

        // Set back to in progress: the swimlane shows, last, with no add buttons.
        returns.set_status(&ctx.db, "progress").await.unwrap();
        let board = get(&request, &owner, "/tasks/board?group=project").await;
        let spring_at = board
            .find(&format!(r#"data-lane="project-{}""#, spring.id))
            .unwrap();
        let website_at = board.find(&lane).unwrap();
        assert!(
            spring_at < website_at,
            "closed projects come after open ones"
        );
        let swimlane = &board[website_at..];
        let swimlane = swimlane.split("<section").next().unwrap();
        assert!(swimlane.contains("lane-done"), "the Done badge");
        assert!(!swimlane.contains("cell-add"), "{swimlane}");
        assert!(board.contains(&format!("project_id={}", spring.id)));
    })
    .await;
}

/// Every lane read goes through `projects::Model::in_lane`, which also skips
/// closed projects. A `.lane` read or `Column::Lane` anywhere else must be a
/// display use on this list.
#[test]
fn no_lane_compare_outside_projects() {
    let allowed = [
        // Shown or linked as stored, never compared.
        r#""lane": p.lane"#,
        r#""lane": project.lane"#,
        "/roadmap?lane={}\", project.lane",
        // Input: Claude's create_project, and the roadmap's `?lane=`.
        "lane: input.lane",
        "query.lane",
        // A reopen's toast names the lane the project is back in.
        "lane_label(&project.lane)",
    ];
    // `..lane(` is struct update syntax, not a field read.
    let lane_read = regex::Regex::new(r"(^|[^.])\.lane\b|Column::Lane\b").unwrap();
    let mut offenders = Vec::new();
    let mut dirs = vec![std::path::PathBuf::from("src")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.to_string_lossy().replace('\\', "/");
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs")
                || name == "src/models/projects.rs"
                || name.starts_with("src/models/_entities/")
            {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for (n, line) in text.lines().enumerate() {
                if lane_read.is_match(line) && !allowed.iter().any(|a| line.contains(a)) {
                    offenders.push(format!("{name}:{}: {}", n + 1, line.trim()));
                }
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:#?}");
}

/// The re-rendered task form keeps Website selected, under "Done".
fn keeps_website_selected(html: &str, website: &projects::Model) {
    let done = picker(html)
        .split(r#"<optgroup label="Done">"#)
        .nth(1)
        .unwrap_or_else(|| panic!("no Done group: {html}"));
    assert!(done.contains(&format!(
        r#"<option value="{}" selected>Website"#,
        website.id
    )));
}

#[tokio::test]
#[serial]
async fn web_create_into_a_closed_project_is_refused() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let website = project_in(&ctx, "juniper-tea-co", "Website").await;
        close(&ctx, &owner, &website).await;

        // Twice: a second Save of the re-rendered form is refused again.
        for _ in 0..2 {
            let res = request
                .post(&owner.at("/tasks"))
                .as_member(&owner)
                .add_header("HX-Request", "true")
                .form(&task_form("Write the returns page", website.id, ""))
                .await;
            assert_eq!(res.status_code(), 422);
            let body = res.text();
            assert!(body.contains(REFUSAL), "{body}");
            keeps_website_selected(&body, &website);
        }
        assert_eq!(tasks::Entity::find().count(&ctx.db).await.unwrap(), 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn web_move_into_a_closed_project_is_refused() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let website = project_in(&ctx, "juniper-tea-co", "Website").await;
        let spring = project_in(&ctx, "juniper-tea-co", "Spring tea range").await;
        let task = task_in(
            &ctx,
            owner.org_id,
            Some(spring.id),
            "Order tins",
            "todo",
            &[],
        )
        .await;
        close(&ctx, &owner, &website).await;

        let res = request
            .post(&owner.at(&format!("/tasks/{}", task.id)))
            .as_member(&owner)
            .add_header("HX-Request", "true")
            .form(&task_form("Order tins", website.id, ""))
            .await;
        assert_eq!(res.status_code(), 422);
        let body = res.text();
        assert!(body.contains(REFUSAL), "{body}");
        keeps_website_selected(&body, &website);
        assert_eq!(reload(&ctx, &task).await, task, "unchanged");
    })
    .await;
}

/// Allowed, and it forgets the saved status, from the board (a drag) and
/// from the task form.
#[tokio::test]
#[serial]
async fn web_status_change_inside_a_closed_project_is_allowed() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let website = project_in(&ctx, "juniper-tea-co", "Website").await;
        let dragged = task_in(
            &ctx,
            owner.org_id,
            Some(website.id),
            "Checkout",
            "todo",
            &[],
        )
        .await;
        let edited = task_in(
            &ctx,
            owner.org_id,
            Some(website.id),
            "Payments",
            "blocked",
            &[],
        )
        .await;
        close(&ctx, &owner, &website).await;
        assert_eq!(
            reload(&ctx, &dragged).await.status_before_close.as_deref(),
            Some("todo")
        );

        let res = request
            .post(&owner.at(&format!("/tasks/{}/status", dragged.id)))
            .as_member(&owner)
            .add_header("HX-Request", "true")
            .form(&json!({ "status": "progress" }))
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let dragged = reload(&ctx, &dragged).await;
        assert_eq!(dragged.status, "progress");
        assert_eq!(dragged.status_before_close, None);

        let mut form = task_form("Payments", website.id, "");
        form["status"] = "progress".into();
        let res = request
            .post(&owner.at(&format!("/tasks/{}", edited.id)))
            .as_member(&owner)
            .add_header("HX-Request", "true")
            .form(&form)
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let edited = reload(&ctx, &edited).await;
        assert_eq!(edited.status, "progress");
        assert_eq!(edited.status_before_close, None);

        let website = projects::Model::find_in_org(&ctx.db, owner.org_id, website.id)
            .await
            .unwrap();
        assert!(website.is_closed(), "the project stays closed");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn mcp_task_writes_follow_the_same_refusal() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let website = project_in(&ctx, "juniper-tea-co", "Website").await;
        let spring = project_in(&ctx, "juniper-tea-co", "Spring tea range").await;
        let inside = task_in(
            &ctx,
            owner.org_id,
            Some(website.id),
            "Checkout",
            "todo",
            &[],
        )
        .await;
        let outside = task_in(
            &ctx,
            owner.org_id,
            Some(spring.id),
            "Order tins",
            "todo",
            &[],
        )
        .await;
        close(&ctx, &owner, &website).await;
        let token = token_for(&ctx, "ana@example.com").await;
        let org = "juniper-tea-co";

        let (error, text) = tool(
            &request,
            &token,
            "create_task",
            json!({ "organisation": org, "title": "Banner", "project_id": website.id }),
        )
        .await;
        assert!(error);
        assert!(
            text.to_string().contains(&format!("project_id: {REFUSAL}")),
            "{text}"
        );
        assert_eq!(tasks::Entity::find().count(&ctx.db).await.unwrap(), 2);

        let (error, text) = tool(
            &request,
            &token,
            "update_task",
            json!({ "organisation": org, "task_id": outside.id, "project_id": website.id }),
        )
        .await;
        assert!(error);
        assert!(text.to_string().contains(REFUSAL), "{text}");
        assert_eq!(reload(&ctx, &outside).await.project_id, Some(spring.id));

        // A task already in it can still change, and forgets its saved status.
        let (error, text) = tool(
            &request,
            &token,
            "update_task",
            json!({ "organisation": org, "task_id": inside.id, "status": "progress" }),
        )
        .await;
        assert!(!error, "{text}");
        let inside = reload(&ctx, &inside).await;
        assert_eq!(inside.status, "progress");
        assert_eq!(inside.status_before_close, None);
    })
    .await;
}
