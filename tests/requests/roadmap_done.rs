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
use serial_test::serial;

use super::{
    prepare_data::{sign_up_member, TestMember},
    tasks::project_in,
};

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
