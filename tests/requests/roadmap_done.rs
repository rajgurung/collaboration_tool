//! Done projects: a closed project leaves its lane everywhere, closing and
//! reopening, the refusals around it, the notices, and the Home donut.
use collab::{
    app::App,
    models::{
        memberships, notifications, organisations, projects,
        tasks::{self, TaskParams},
        users,
    },
};
use loco_rs::{app::AppContext, prelude::TenantActiveModelExt, testing::prelude::*, TestServer};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, IntoActiveModel, PaginatorTrait,
    QueryFilter,
};
use serde_json::json;
use serial_test::serial;

use super::{
    admin::super_admin_cookie,
    mcp::{rpc, token_for, tool},
    prepare_data::{approve_as_owner, join, sign_up_member, AsMember, TestMember},
    tasks::{project_in, task_form},
};

const REFUSAL: &str = "Website is done. Reopen the project first.";
const ORG: &str = "juniper-tea-co";

/// Someone who joined Juniper Tea Co and was approved, with `role`.
/// Returns them and their user id.
async fn teammate(
    request: &TestServer,
    ctx: &AppContext,
    name: &str,
    role: &str,
) -> (TestMember, i64) {
    let email = format!("{name}@example.com");
    let member = join(request, ctx, ORG, name, &email).await;
    let user = users::Model::find_by_email(&ctx.db, &email).await.unwrap();
    let pending = memberships::Entity::find()
        .filter(memberships::Column::UserId.eq(user.id))
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    let approved = approve_as_owner(ctx, &pending).await;
    if role != memberships::role::MEMBER {
        let mut m = approved.into_active_model();
        m.role = ActiveValue::Set(role.to_string());
        m.update(&ctx.db).await.unwrap();
    }
    (member, user.id)
}

/// Makes `user_id` the project's owner.
async fn owned_by(ctx: &AppContext, project: &projects::Model, user_id: i64) -> projects::Model {
    let mut p = project.clone().into_active_model();
    p.owner_id = ActiveValue::Set(Some(user_id));
    p.update(&ctx.db).await.unwrap()
}

async fn find(ctx: &AppContext, project: &projects::Model) -> projects::Model {
    projects::Entity::find_by_id(project.id)
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap()
}

/// Posts a close or reopen (`action`) as HTMX. Returns the status and the toast.
async fn act(
    request: &TestServer,
    member: &TestMember,
    project: &projects::Model,
    action: &str,
) -> (u16, String) {
    let res = request
        .post(&member.at(&format!("/roadmap/projects/{}/{action}", project.id)))
        .as_member(member)
        .add_header("HX-Request", "true")
        .await;
    let toast = res
        .headers()
        .get("HX-Trigger")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    (res.status_code().as_u16(), toast)
}

/// The Done column on the roadmap page.
fn done_column(html: &str) -> &str {
    html.split(r#"data-lane="done""#).nth(1).unwrap_or_default()
}

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

#[tokio::test]
#[serial]
async fn owners_admins_and_the_project_owner_can_close() {
    request::<App, _, _>(|request, ctx| async move {
        let ana = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let (maya, _) = teammate(&request, &ctx, "maya", memberships::role::ADMIN).await;
        let (tom, tom_id) = teammate(&request, &ctx, "tom", memberships::role::MEMBER).await;
        let website = project_in(&ctx, ORG, "Website").await;
        let spring = project_in(&ctx, ORG, "Spring tea range").await;
        let wholesale = project_in(&ctx, ORG, "Wholesale catalogue").await;
        let wholesale = owned_by(&ctx, &wholesale, tom_id).await;

        for (who, project) in [(&ana, &website), (&maya, &spring), (&tom, &wholesale)] {
            let (status, toast) = act(&request, who, project, "close").await;
            assert_eq!(status, 200, "{}", project.name);
            assert!(
                toast.contains(&format!("{} done.", project.name)),
                "{toast}"
            );
            assert!(toast.contains("roadmap-changed") && toast.contains("close-dialogs"));
            assert!(find(&ctx, project).await.is_closed());

            let (status, toast) = act(&request, who, project, "reopen").await;
            assert_eq!(status, 200);
            assert!(toast.contains("is back in Now."), "{toast}");
            assert!(!find(&ctx, project).await.is_closed());
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_member_who_does_not_own_it_cannot_close_or_reopen() {
    request::<App, _, _>(|request, ctx| async move {
        let ana = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let (tom, tom_id) = teammate(&request, &ctx, "tom", memberships::role::MEMBER).await;
        let website = project_in(&ctx, ORG, "Website").await;
        let task = task_in(&ctx, ana.org_id, Some(website.id), "Checkout", "todo", &[]).await;
        let wholesale = project_in(&ctx, ORG, "Wholesale catalogue").await;
        owned_by(&ctx, &wholesale, tom_id).await;

        let (status, _) = act(&request, &tom, &website, "close").await;
        assert_eq!(status, 403);
        assert!(!find(&ctx, &website).await.is_closed());
        assert_eq!(reload(&ctx, &task).await, task);

        close(&ctx, &ana, &website).await;
        let (status, _) = act(&request, &tom, &website, "reopen").await;
        assert_eq!(status, 403);
        assert!(find(&ctx, &website).await.is_closed());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_box_and_reopen_show_only_to_those_allowed() {
    request::<App, _, _>(|request, ctx| async move {
        let ana = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let (maya, _) = teammate(&request, &ctx, "maya", memberships::role::ADMIN).await;
        let (tom, tom_id) = teammate(&request, &ctx, "tom", memberships::role::MEMBER).await;
        let (leo, _) = teammate(&request, &ctx, "leo", memberships::role::MEMBER).await;
        let website = project_in(&ctx, ORG, "Website").await;
        let website = owned_by(&ctx, &website, tom_id).await;
        let shop = project_in(&ctx, ORG, "Shop till upgrade").await;
        let shop = owned_by(&ctx, &shop, tom_id).await;
        close(&ctx, &ana, &shop).await;

        for (who, allowed) in [(&ana, true), (&maya, true), (&tom, true), (&leo, false)] {
            let sheet = get(
                &request,
                who,
                &format!("/roadmap/projects/{}/edit", website.id),
            )
            .await;
            assert_eq!(sheet.contains("Mark project done"), allowed, "{sheet}");
            assert_eq!(sheet.contains("data-close-confirm"), allowed);
            let roadmap = get(&request, who, "/roadmap").await;
            assert_eq!(done_column(&roadmap).contains("data-reopen"), allowed);
            assert!(done_column(&roadmap).contains("Shop till upgrade"));
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_super_admin_acting_cannot_close_or_reopen() {
    request::<App, _, _>(|request, ctx| async move {
        let ana = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let website = project_in(&ctx, ORG, "Website").await;
        let spring = project_in(&ctx, ORG, "Spring tea range").await;
        close(&ctx, &ana, &spring).await;
        let admin = super_admin_cookie(&request, &ctx).await;
        let cookie = format!("{}; acting_org={}", admin.1.to_str().unwrap(), ana.org_id);

        for (project, action) in [(&website, "close"), (&spring, "reopen")] {
            let res = request
                .post(&ana.at(&format!("/roadmap/projects/{}/{action}", project.id)))
                .add_header("cookie", cookie.clone())
                .add_header("x-org-id", ana.org_id.to_string())
                .add_header("HX-Request", "true")
                .await;
            assert_eq!(res.status_code(), 403, "{}", res.text());
            assert!(res.text().contains("Leave admin mode"), "{}", res.text());
        }
        assert!(!find(&ctx, &website).await.is_closed());
        assert!(find(&ctx, &spring).await.is_closed());

        // Nor do they see the box or Reopen.
        let res = request
            .get(&ana.at(&format!("/roadmap/projects/{}/edit", website.id)))
            .add_header("cookie", cookie.clone())
            .await;
        assert_eq!(res.status_code(), 200);
        assert!(!res.text().contains("Mark project done"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn pending_members_are_sent_away() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let pending = join(&request, &ctx, ORG, "leo", "leo@example.com").await;
        let website = project_in(&ctx, ORG, "Website").await;
        let (status, _) = act(&request, &pending, &website, "close").await;
        assert_ne!(status, 200);
        assert!(!find(&ctx, &website).await.is_closed());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_toast_reports_the_real_count() {
    request::<App, _, _>(|request, ctx| async move {
        let ana = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let website = project_in(&ctx, ORG, "Website").await;
        let pid = Some(website.id);
        let first = task_in(&ctx, ana.org_id, pid, "Returns page", "todo", &[]).await;
        task_in(&ctx, ana.org_id, pid, "Checkout", "progress", &[]).await;
        task_in(&ctx, ana.org_id, pid, "Payments", "progress", &[]).await;
        task_in(&ctx, ana.org_id, pid, "Couriers", "blocked", &[]).await;

        let sheet = get(
            &request,
            &ana,
            &format!("/roadmap/projects/{}/edit", website.id),
        )
        .await;
        assert!(sheet.contains("4 open tasks will be marked done. 1 is blocked."));
        assert!(sheet.contains("Its 4 open tasks are marked done with it."));
        assert!(sheet.contains("No one else gets a notice."));

        // Someone finishes one while the confirm is open.
        first.set_status(&ctx.db, "done").await.unwrap();
        let (status, toast) = act(&request, &ana, &website, "close").await;
        assert_eq!(status, 200);
        assert!(toast.contains("Website done. 3 tasks closed."), "{toast}");

        let (_, toast) = act(&request, &ana, &website, "close").await;
        assert!(toast.contains("Website is already done."), "{toast}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn reopen_toast_counts_restored_tasks() {
    request::<App, _, _>(|request, ctx| async move {
        let ana = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let website = project_in(&ctx, ORG, "Website").await;
        let pid = Some(website.id);
        for (title, status) in [
            ("A", "todo"),
            ("B", "progress"),
            ("C", "blocked"),
            ("D", "done"),
        ] {
            task_in(&ctx, ana.org_id, pid, title, status, &[]).await;
        }
        act(&request, &ana, &website, "close").await;
        let (_, toast) = act(&request, &ana, &website, "reopen").await;
        assert!(
            toast.contains("Website is back in Now. 3 tasks reopened."),
            "{toast}"
        );
        let (_, toast) = act(&request, &ana, &website, "reopen").await;
        assert!(toast.contains("Website is already open."), "{toast}");

        // Without HTMX, a redirect to the lane.
        let res = request
            .post(&ana.at(&format!("/roadmap/projects/{}/close", website.id)))
            .as_member(&ana)
            .await;
        assert_eq!(res.status_code(), 303);
        assert!(res.headers()["location"]
            .to_str()
            .unwrap()
            .ends_with("/roadmap?lane=done"));
        let empty = project_in(&ctx, ORG, "Empty").await;
        let (_, toast) = act(&request, &ana, &empty, "close").await;
        assert!(toast.contains(r#""Empty done.""#), "{toast}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_closed_project_leaves_now_and_joins_done() {
    request::<App, _, _>(|request, ctx| async move {
        let ana = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let website = project_in(&ctx, ORG, "Website").await;
        let pid = Some(website.id);
        task_in(&ctx, ana.org_id, pid, "Logo", "done", &[]).await;
        let back = task_in(&ctx, ana.org_id, pid, "Checkout", "todo", &[]).await;
        act(&request, &ana, &website, "close").await;

        let page = get(&request, &ana, "/roadmap?lane=done").await;
        assert!(page.contains(r#"Now<span class="seg-count">0</span>"#));
        assert!(page.contains(r#"Done<span class="seg-count">1</span>"#));
        assert!(page.contains(r#"data-lane="done" data-current"#));
        let done = done_column(&page);
        assert!(done.contains("Website") && done.contains("is-closed"));
        assert!(done.contains("2 tasks done"));
        assert!(done.contains("Marked done by ana"));
        assert!(!done.contains("Edit Website"), "no pencil on a done card");

        // A task set back shows as open on the card.
        back.set_status(&ctx.db, "progress").await.unwrap();
        let page = get(&request, &ana, "/roadmap/list?lane=done").await;
        assert!(done_column(&page).contains("1 open task"));
        assert!(done_column(&page).contains("1 task done"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn done_shows_the_newest_five_then_all() {
    request::<App, _, _>(|request, ctx| async move {
        let ana = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        for n in 1..=14 {
            let p = project_in(&ctx, ORG, &format!("Finished {n:02}")).await;
            close(&ctx, &ana, &p).await;
        }
        let page = get(&request, &ana, "/roadmap?lane=done").await;
        let done = done_column(&page);
        assert_eq!(done.matches("is-closed").count(), 5);
        assert!(done.contains("Show all 14"));
        let newest = done.find("Finished 14").unwrap();
        assert!(newest < done.find("Finished 13").unwrap(), "newest first");
        assert!(!done.contains("Finished 09"));

        let all = get(&request, &ana, "/roadmap/list?lane=done&all=1").await;
        let done = done_column(&all);
        assert_eq!(done.matches("is-closed").count(), 14);
        assert!(done.contains("Show newest 5"));
        assert!(
            all.contains("/roadmap/list?lane=done&amp;all=1")
                || all.contains("/roadmap/list?lane=done&all=1")
        );
    })
    .await;
}

/// A sheet opened before the close: the edit and the save are refused with
/// a toast, never an `HX-Redirect`, which would read as a stale tab.
#[tokio::test]
#[serial]
async fn editing_a_closed_project_is_refused() {
    request::<App, _, _>(|request, ctx| async move {
        let ana = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let website = project_in(&ctx, ORG, "Website").await;
        close(&ctx, &ana, &website).await;
        let before = find(&ctx, &website).await;

        let edit = request
            .get(&ana.at(&format!("/roadmap/projects/{}/edit", website.id)))
            .as_member(&ana)
            .add_header("HX-Request", "true")
            .await;
        let save = request
            .post(&ana.at(&format!("/roadmap/projects/{}", website.id)))
            .as_member(&ana)
            .add_header("HX-Request", "true")
            .form(&json!({
                "name": "Website 2", "lane": "later", "status": "Active",
                "accent": "#6cb6ff", "owner_id": "", "summary": "",
            }))
            .await;
        for res in [edit, save] {
            assert_eq!(res.status_code(), 409);
            assert!(res.headers().get("HX-Redirect").is_none());
            let trigger = res.headers()["HX-Trigger"].to_str().unwrap().to_string();
            assert!(
                trigger.contains(REFUSAL) && trigger.contains(r#""kind":"error""#),
                "{trigger}"
            );
        }
        assert_eq!(find(&ctx, &website).await, before);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn org_a_cannot_close_or_reopen_org_b() {
    request::<App, _, _>(|request, ctx| async move {
        let ana = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        sign_up_member(&request, "Globex", "gina", "gina@example.com").await;
        let theirs = project_in(&ctx, "globex", "Secret plan").await;
        let globex = organisations::Model::find_by_slug(&ctx.db, "globex")
            .await
            .unwrap();
        let task = task_in(&ctx, globex.id, Some(theirs.id), "Secret task", "todo", &[]).await;

        for action in ["close", "reopen"] {
            let (status, _) = act(&request, &ana, &theirs, action).await;
            assert_eq!(status, 404, "{action}");
        }
        assert!(!find(&ctx, &theirs).await.is_closed());
        assert_eq!(reload(&ctx, &task).await, task);
    })
    .await;
}

/// Tom owns a project in Juniper Tea Co and is a member of Globex. That gives
/// him nothing in Globex.
#[tokio::test]
#[serial]
async fn owning_a_project_in_a_does_not_let_you_close_bs() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let (tom, tom_id) = teammate(&request, &ctx, "tom", memberships::role::MEMBER).await;
        let wholesale = project_in(&ctx, ORG, "Wholesale catalogue").await;
        owned_by(&ctx, &wholesale, tom_id).await;
        sign_up_member(&request, "Globex", "gina", "gina@example.com").await;
        let globex = organisations::Model::find_by_slug(&ctx.db, "globex")
            .await
            .unwrap();
        memberships::ActiveModel {
            user_id: ActiveValue::Set(tom_id),
            username: ActiveValue::Set("tom".to_string()),
            role: ActiveValue::Set(memberships::role::MEMBER.to_string()),
            status: ActiveValue::Set(memberships::status::ACTIVE.to_string()),
            ..Default::default()
        }
        .set_tenant(globex.id)
        .unwrap()
        .insert(&ctx.db)
        .await
        .unwrap();
        let tom_in_globex = TestMember::of(&ctx, tom.cookie.clone(), globex.id).await;
        let theirs = project_in(&ctx, "globex", "Secret plan").await;

        let (status, _) = act(&request, &tom_in_globex, &theirs, "close").await;
        assert_eq!(status, 403);
        // His own project, asked for at Globex's address, isn't there.
        let (status, _) = act(&request, &tom_in_globex, &wholesale, "close").await;
        assert_eq!(status, 404);
        assert!(!find(&ctx, &theirs).await.is_closed());
        assert!(!find(&ctx, &wholesale).await.is_closed());
    })
    .await;
}

async fn notices(ctx: &AppContext) -> Vec<notifications::Model> {
    notifications::Entity::find().all(&ctx.db).await.unwrap()
}

async fn ana_id(ctx: &AppContext) -> i64 {
    users::Model::find_by_email(&ctx.db, "ana@example.com")
        .await
        .unwrap()
        .id
}

/// Website, owned by Tom. Maya is on two open tasks, Tom on one, Ana (who
/// closes it) on one, and Leo on one already done. Starts with no notices.
/// Returns Ana and the ids of Maya, Tom and Leo.
async fn website_with_people(
    request: &TestServer,
    ctx: &AppContext,
) -> (TestMember, i64, i64, i64, projects::Model) {
    let ana = sign_up_member(request, "Juniper Tea Co", "ana", "ana@example.com").await;
    let (_, maya) = teammate(request, ctx, "maya", memberships::role::MEMBER).await;
    let (_, tom) = teammate(request, ctx, "tom", memberships::role::MEMBER).await;
    let (_, leo) = teammate(request, ctx, "leo", memberships::role::MEMBER).await;
    let website = project_in(ctx, ORG, "Website").await;
    let website = owned_by(ctx, &website, tom).await;
    let pid = Some(website.id);
    let ana_user = ana_id(ctx).await;
    task_in(ctx, ana.org_id, pid, "Returns page", "todo", &[maya]).await;
    task_in(
        ctx,
        ana.org_id,
        pid,
        "Checkout",
        "blocked",
        &[maya, ana_user],
    )
    .await;
    task_in(ctx, ana.org_id, pid, "Couriers", "progress", &[tom]).await;
    task_in(ctx, ana.org_id, pid, "Logo", "done", &[leo]).await;
    notifications::Entity::delete_many()
        .exec(&ctx.db)
        .await
        .unwrap();
    (ana, maya, tom, leo, website)
}

#[tokio::test]
#[serial]
async fn maya_with_two_tasks_gets_one_notice() {
    request::<App, _, _>(|request, ctx| async move {
        let (ana, maya, tom, leo, website) = website_with_people(&request, &ctx).await;
        let (status, _) = act(&request, &ana, &website, "close").await;
        assert_eq!(status, 200);

        let all = notices(&ctx).await;
        let mayas: Vec<_> = all.iter().filter(|n| n.user_id == maya).collect();
        assert_eq!(mayas.len(), 1);
        assert_eq!(mayas[0].kind, "closed");
        assert_eq!(mayas[0].body, "marked Website done\n2 of your tasks closed");
        assert_eq!(mayas[0].link, "/roadmap?lane=done");
        // Tom is an assignee and the owner: only the assignee notice.
        let toms: Vec<_> = all.iter().filter(|n| n.user_id == tom).collect();
        assert_eq!(toms.len(), 1);
        assert_eq!(toms[0].body, "marked Website done\n1 of your tasks closed");
        // Leo's task was already done, and Ana closed it.
        let ana_user = ana_id(&ctx).await;
        assert!(all
            .iter()
            .all(|n| n.user_id != leo && n.user_id != ana_user));
        assert!(all.iter().all(|n| n.kind == "closed"), "no task notices");
        assert_eq!(all.len(), 2);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_owner_gets_the_total() {
    request::<App, _, _>(|request, ctx| async move {
        let ana = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let (_, maya) = teammate(&request, &ctx, "maya", memberships::role::MEMBER).await;
        let (_, tom) = teammate(&request, &ctx, "tom", memberships::role::MEMBER).await;
        let website = owned_by(&ctx, &project_in(&ctx, ORG, "Website").await, tom).await;
        let empty = owned_by(&ctx, &project_in(&ctx, ORG, "Empty").await, tom).await;
        for title in ["A", "B", "C", "D"] {
            task_in(&ctx, ana.org_id, Some(website.id), title, "todo", &[maya]).await;
        }
        notifications::Entity::delete_many()
            .exec(&ctx.db)
            .await
            .unwrap();

        act(&request, &ana, &website, "close").await;
        act(&request, &ana, &empty, "close").await;
        let toms: Vec<String> = notices(&ctx)
            .await
            .into_iter()
            .filter(|n| n.user_id == tom)
            .map(|n| n.body)
            .collect();
        assert_eq!(
            toms,
            ["marked Website done\n4 tasks closed", "marked Empty done"],
            "with nothing open, the owner still hears"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn removed_members_get_nothing() {
    request::<App, _, _>(|request, ctx| async move {
        let (ana, maya, tom, _, website) = website_with_people(&request, &ctx).await;
        for user in [maya, tom] {
            let mut m = memberships::Entity::find()
                .filter(memberships::Column::UserId.eq(user))
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap()
                .into_active_model();
            m.status = ActiveValue::Set(memberships::status::REMOVED.to_string());
            m.update(&ctx.db).await.unwrap();
        }
        act(&request, &ana, &website, "close").await;
        assert!(notices(&ctx).await.is_empty());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_second_close_or_reopen_sends_nothing() {
    request::<App, _, _>(|request, ctx| async move {
        let (ana, _, _, _, website) = website_with_people(&request, &ctx).await;
        act(&request, &ana, &website, "close").await;
        notifications::Entity::delete_many()
            .exec(&ctx.db)
            .await
            .unwrap();
        act(&request, &ana, &website, "close").await;
        assert!(notices(&ctx).await.is_empty(), "already closed");
        act(&request, &ana, &website, "reopen").await;
        notifications::Entity::delete_many()
            .exec(&ctx.db)
            .await
            .unwrap();
        let (status, _) = act(&request, &ana, &website, "reopen").await;
        assert_eq!(status, 200);
        assert!(notices(&ctx).await.is_empty(), "already open");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_reopen_sends_one_notice_per_person() {
    request::<App, _, _>(|request, ctx| async move {
        let (ana, maya, tom, leo, website) = website_with_people(&request, &ctx).await;
        act(&request, &ana, &website, "close").await;
        notifications::Entity::delete_many()
            .exec(&ctx.db)
            .await
            .unwrap();
        let (status, _) = act(&request, &ana, &website, "reopen").await;
        assert_eq!(status, 200);
        let all = notices(&ctx).await;
        let bodies = |user: i64| -> Vec<String> {
            all.iter()
                .filter(|n| n.user_id == user)
                .map(|n| n.body.clone())
                .collect()
        };
        assert_eq!(bodies(maya), ["reopened Website\n2 of your tasks reopened"]);
        assert_eq!(bodies(tom), ["reopened Website\n1 of your tasks reopened"]);
        assert!(bodies(leo).is_empty(), "Logo was done before the close");
        assert!(all.iter().all(|n| n.kind == "reopened"));
        assert!(
            all.iter().all(|n| n.link == "/roadmap?lane=now"),
            "back in its lane"
        );
        let ana_user = ana_id(&ctx).await;
        assert!(
            all.iter().all(|n| n.user_id != ana_user),
            "the reopener is not told"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_bell_shows_the_reopened_icon() {
    request::<App, _, _>(|request, ctx| async move {
        let (ana, _, _, _, website) = website_with_people(&request, &ctx).await;
        act(&request, &ana, &website, "close").await;
        act(&request, &ana, &website, "reopen").await;
        let cookie = super::prepare_data::login(
            &request,
            "maya@example.com",
            super::prepare_data::USER_PASSWORD,
        )
        .await;
        let maya = TestMember::of(&ctx, cookie, ana.org_id).await;
        let bell = get(&request, &maya, "/notifications").await;
        assert!(bell.contains("notice-reopened"), "{bell}");
        assert!(bell.contains("reopened Website"));
        assert!(bell.contains("2 of your tasks reopened"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_bell_shows_the_closed_icon() {
    request::<App, _, _>(|request, ctx| async move {
        let (ana, _, _, _, website) = website_with_people(&request, &ctx).await;
        act(&request, &ana, &website, "close").await;
        let cookie = super::prepare_data::login(
            &request,
            "maya@example.com",
            super::prepare_data::USER_PASSWORD,
        )
        .await;
        let maya = TestMember::of(&ctx, cookie, ana.org_id).await;
        let bell = get(&request, &maya, "/notifications").await;
        assert!(bell.contains("notice-closed"), "{bell}");
        assert!(bell.contains("marked Website done"));
        assert!(bell.contains("2 of your tasks closed"));
    })
    .await;
}

/// Closing Juniper's Website tells only Juniper's team.
#[tokio::test]
#[serial]
async fn notices_stay_in_the_org() {
    request::<App, _, _>(|request, ctx| async move {
        let (ana, _, _, _, website) = website_with_people(&request, &ctx).await;
        sign_up_member(&request, "Globex", "gina", "gina@example.com").await;
        let gina = users::Model::find_by_email(&ctx.db, "gina@example.com")
            .await
            .unwrap();
        act(&request, &ana, &website, "close").await;
        let all = notices(&ctx).await;
        assert!(!all.is_empty());
        assert!(all.iter().all(|n| n.organisation_id == ana.org_id));
        assert!(all.iter().all(|n| n.user_id != gina.id));
    })
    .await;
}

/// Adds `n` tasks with `status` to a project (or a chore with `None`).
async fn tasks_in(ctx: &AppContext, org_id: i64, project_id: Option<i64>, status: &str, n: usize) {
    for i in 0..n {
        task_in(
            ctx,
            org_id,
            project_id,
            &format!("{status} {i}"),
            status,
            &[],
        )
        .await;
    }
}

#[tokio::test]
#[serial]
async fn home_shows_the_donut_for_now_projects() {
    request::<App, _, _>(|request, ctx| async move {
        let ana = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let org = ana.org_id;
        let website = project_in(&ctx, ORG, "Website").await;
        let spring = project_in(&ctx, ORG, "Spring tea range").await;
        let wholesale = project_in(&ctx, ORG, "Wholesale catalogue").await;
        tasks_in(&ctx, org, Some(website.id), "done", 11).await;
        tasks_in(&ctx, org, Some(spring.id), "done", 10).await;
        tasks_in(&ctx, org, Some(wholesale.id), "done", 10).await;
        tasks_in(&ctx, org, Some(website.id), "progress", 9).await;
        tasks_in(&ctx, org, Some(spring.id), "blocked", 3).await;
        tasks_in(&ctx, org, Some(wholesale.id), "todo", 7).await;
        // None of these count: a chore, a closed project, a project in Next.
        tasks_in(&ctx, org, None, "todo", 4).await;
        let shop = project_in(&ctx, ORG, "Shop till upgrade").await;
        tasks_in(&ctx, org, Some(shop.id), "progress", 5).await;
        close(&ctx, &ana, &shop).await;
        let next = projects::Model::create(
            &ctx.db,
            org,
            &projects::ProjectParams {
                name: "Ship to Europe".to_string(),
                lane: "next".to_string(),
                status: "Idea".to_string(),
                accent: "#c9d76a".to_string(),
                owner_id: String::new(),
                summary: String::new(),
            },
            None,
        )
        .await
        .unwrap();
        tasks_in(&ctx, org, Some(next.id), "blocked", 2).await;

        let home = get(&request, &ana, "/dashboard").await;
        assert!(home.contains(
            r#"aria-label="Now: 62% done. 31 done, 9 in progress, 3 blocked, 7 to do.""#
        ));
        assert!(home.contains(">62%</text>"));
        assert!(home.contains("31 of 50 tasks done"));
        assert!(home.contains("In 3 projects in Now"));
        assert_eq!(home.matches("donut-arc").count(), 4);
        assert!(home.contains(r#"stroke-dasharray="192.78 314.16""#));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn no_donut_when_now_has_no_tasks() {
    request::<App, _, _>(|request, ctx| async move {
        let ana = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let home = get(&request, &ana, "/dashboard").await;
        assert!(
            home.contains(r#"Nothing in the "now" lane."#)
                || home.contains("Nothing in the &quot;now&quot; lane.")
        );
        assert!(!home.contains("donut-card"));

        project_in(&ctx, ORG, "Tea tasting evenings").await;
        let home = get(&request, &ana, "/dashboard").await;
        assert!(home.contains("Tea tasting evenings"));
        assert!(home.contains("No tasks yet"));
        assert!(!home.contains("donut-card"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn all_done_shows_100() {
    request::<App, _, _>(|request, ctx| async move {
        let ana = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let website = project_in(&ctx, ORG, "Website").await;
        tasks_in(&ctx, ana.org_id, Some(website.id), "done", 19).await;
        let home = get(&request, &ana, "/dashboard").await;
        assert!(home.contains(">100%</text>"));
        assert_eq!(home.matches("donut-arc").count(), 1);
        assert!(home.contains("In 1 project in Now"));
        assert_eq!(home.matches(r#"class="zero""#).count(), 3);
    })
    .await;
}

/// "What's done on the roadmap?": closed projects say so, in this
/// organisation only, and the instructions explain it.
#[tokio::test]
#[serial]
async fn mcp_list_projects_shows_closed() {
    request::<App, _, _>(|request, ctx| async move {
        let ana = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let gina = sign_up_member(&request, "Globex", "gina", "gina@example.com").await;
        let website = project_in(&ctx, ORG, "Website").await;
        project_in(&ctx, ORG, "Spring tea range").await;
        let secret = project_in(&ctx, "globex", "Secret plan").await;
        close(&ctx, &ana, &website).await;
        close(&ctx, &gina, &secret).await;
        let token = token_for(&ctx, "ana@example.com").await;

        let (error, list) = tool(&request, &token, "list_projects", json!({})).await;
        assert!(!error, "{list}");
        let projects = list["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 2, "only this organisation's: {list}");
        let by_name = |name: &str| projects.iter().find(|p| p["name"] == name).unwrap();
        let done = by_name("Website");
        assert_eq!(done["closed"], true);
        assert_eq!(done["lane"], "now", "the lane as stored");
        let at = done["closed_at"].as_str().unwrap();
        assert!(chrono::DateTime::parse_from_rfc3339(at).is_ok(), "{at}");
        let open = by_name("Spring tea range");
        assert_eq!(open["closed"], false);
        assert!(open["closed_at"].is_null());

        let init = rpc(
            &request,
            &token,
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" }
            }),
        )
        .await
        .json();
        let instructions = init["result"]["instructions"].as_str().unwrap();
        assert!(instructions.contains("list_projects shows it as closed"));
        assert!(instructions.contains("is not current work"));
    })
    .await;
}

/// Posts a drag onto `lane` as HTMX. Returns the status and the HX-Trigger.
async fn drag(
    request: &TestServer,
    member: &TestMember,
    project: &projects::Model,
    lane: &str,
) -> (u16, String) {
    let res = request
        .post(&member.at(&format!("/roadmap/projects/{}/lane", project.id)))
        .as_member(member)
        .add_header("HX-Request", "true")
        .form(&[("lane", lane)])
        .await;
    let trigger = res
        .headers()
        .get("HX-Trigger")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    (res.status_code().as_u16(), trigger)
}

#[tokio::test]
#[serial]
async fn dragging_a_card_moves_it_to_that_lane() {
    request::<App, _, _>(|request, ctx| async move {
        let ana = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let (tom_member, _) = teammate(&request, &ctx, "tom", memberships::role::MEMBER).await;
        let website = project_in(&ctx, ORG, "Website").await;

        // Any member can move it, as with the edit sheet.
        let (status, trigger) = drag(&request, &tom_member, &website, "next").await;
        assert_eq!(status, 200);
        assert!(trigger.contains("Website moved to Next."), "{trigger}");
        assert!(trigger.contains("roadmap-changed"));
        assert!(find(&ctx, &website).await.in_lane("next"));

        // The same lane again changes nothing and says nothing.
        let (status, trigger) = drag(&request, &ana, &website, "next").await;
        assert_eq!(status, 200);
        assert!(!trigger.contains("toast"), "{trigger}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn done_and_unknown_lanes_are_refused() {
    request::<App, _, _>(|request, ctx| async move {
        let ana = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let website = project_in(&ctx, ORG, "Website").await;
        for lane in ["done", "someday", ""] {
            let (status, _) = drag(&request, &ana, &website, lane).await;
            assert_eq!(status, 400, "{lane}");
        }
        let website = find(&ctx, &website).await;
        assert!(website.in_lane("now"), "unmoved");
        assert!(!website.is_closed(), "a drag never closes a project");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_closed_project_cannot_be_dragged() {
    request::<App, _, _>(|request, ctx| async move {
        let ana = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let website = project_in(&ctx, ORG, "Website").await;
        close(&ctx, &ana, &website).await;
        let (status, trigger) = drag(&request, &ana, &website, "later").await;
        assert_eq!(status, 409);
        assert!(trigger.contains("Website is done. Reopen the project first."));
        assert_eq!(find(&ctx, &website).await.lane, "now", "keeps its lane");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn cards_carry_what_the_drag_needs() {
    request::<App, _, _>(|request, ctx| async move {
        let ana = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        let (tom_member, _) = teammate(&request, &ctx, "tom", memberships::role::MEMBER).await;
        let website = project_in(&ctx, ORG, "Website").await;
        let card = format!(r#"data-project-id="{}" data-card-lane="now""#, website.id);

        let page = get(&request, &ana, "/roadmap").await;
        assert!(page.contains(&card), "{page}");
        assert!(page.contains(r#"data-drop-lane="done""#));
        assert!(
            page.contains("data-can-close"),
            "an owner may drop it on Done"
        );

        let page = get(&request, &tom_member, "/roadmap").await;
        assert!(page.contains(&card));
        assert!(
            !page.contains("data-can-close"),
            "a member who doesn't own it may not"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn org_a_cannot_drag_org_b_projects() {
    request::<App, _, _>(|request, ctx| async move {
        let ana = sign_up_member(&request, "Juniper Tea Co", "ana", "ana@example.com").await;
        sign_up_member(&request, "Globex", "gina", "gina@example.com").await;
        let theirs = project_in(&ctx, "globex", "Secret plan").await;
        let (status, _) = drag(&request, &ana, &theirs, "later").await;
        assert_eq!(status, 404);
        assert!(find(&ctx, &theirs).await.in_lane("now"));
    })
    .await;
}
