use collab::{
    app::App,
    models::{organisations, projects, task_notes, tasks, users},
};
use loco_rs::{app::AppContext, testing::prelude::*};
use sea_orm::{EntityTrait, PaginatorTrait};
use serial_test::serial;

use super::prepare_data::sign_up;

async fn project_in(ctx: &AppContext, slug: &str, name: &str) -> projects::Model {
    let org = organisations::Model::find_by_slug(&ctx.db, slug)
        .await
        .unwrap();
    projects::Model::create(
        &ctx.db,
        org.id,
        &projects::ProjectParams {
            name: name.to_string(),
            lane: "now".to_string(),
            status: "Active".to_string(),
            progress: 0,
            accent: "#ffb454".to_string(),
            owner_id: String::new(),
            summary: String::new(),
        },
    )
    .await
    .unwrap()
}

fn task_form(title: &str, project_id: i64, owner_id: &str) -> serde_json::Value {
    serde_json::json!({
        "title": title, "project_id": project_id.to_string(), "owner_id": owner_id,
        "priority": "high", "due_on": "2026-11-03",
    })
}

#[tokio::test]
#[serial]
async fn creating_a_task_puts_it_in_to_do() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let project = project_in(&ctx, "acme", "Launch").await;
        let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();

        let res = request
            .post("/tasks")
            .add_header(owner.0.clone(), owner.1.clone())
            .add_header("HX-Request", "true")
            .form(&task_form(
                "Write the launch email",
                project.id,
                &alice.id.to_string(),
            ))
            .await;
        assert_eq!(res.status_code(), 200);
        let trigger = res
            .headers()
            .get("HX-Trigger")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            trigger.contains("tasks-changed") && trigger.contains("Task added"),
            "{trigger}"
        );

        // It shows under "Mine" (alice owns it) in the To do group, with its due date.
        let body = request
            .get("/tasks")
            .add_header(owner.0, owner.1)
            .await
            .text();
        assert!(body.contains("Write the launch email"));
        assert!(body.contains("To do · 1"));
        assert!(body.contains("Due Tue 3 Nov"));

        let task = tasks::Entity::find().one(&ctx.db).await.unwrap().unwrap();
        assert_eq!(task.status, "todo");
        assert_eq!(task.priority, "high");
        assert_eq!(task.owner_id, Some(alice.id));
        assert_eq!(task.project_id, project.id);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn tasks_need_a_title_and_a_project_from_this_org() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        sign_up(&request, "Globex", "gina", "gina@example.com").await;
        let foreign = project_in(&ctx, "globex", "Their project").await;

        let res = request
            .post("/tasks")
            .add_header(owner.0.clone(), owner.1.clone())
            .add_header("HX-Request", "true")
            .form(&task_form("", foreign.id, ""))
            .await;
        assert_eq!(res.status_code(), 422);
        assert_eq!(res.headers().get("HX-Retarget").unwrap(), "#task-form");
        assert!(res.text().contains("Describe the task"));

        let res = request
            .post("/tasks")
            .add_header(owner.0, owner.1)
            .add_header("HX-Request", "true")
            .form(&task_form("Sneaky", foreign.id, ""))
            .await;
        assert_eq!(res.status_code(), 422);
        assert!(res.text().contains("Choose a project."));
        assert_eq!(tasks::Entity::find().count(&ctx.db).await.unwrap(), 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn status_changes_move_the_card() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let project = project_in(&ctx, "acme", "Launch").await;
        request
            .post("/tasks")
            .add_header(owner.0.clone(), owner.1.clone())
            .form(&task_form("Ship it", project.id, ""))
            .await;
        let task = tasks::Entity::find().one(&ctx.db).await.unwrap().unwrap();

        let res = request
            .post(&format!("/tasks/{}/status", task.id))
            .add_header(owner.0.clone(), owner.1.clone())
            .add_header("HX-Request", "true")
            .form(&serde_json::json!({ "status": "done", "q": "" }))
            .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(
            tasks::Entity::find_by_id(task.id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap()
                .status,
            "done"
        );

        let bad = request
            .post(&format!("/tasks/{}/status", task.id))
            .add_header(owner.0, owner.1)
            .form(&serde_json::json!({ "status": "archived" }))
            .await;
        assert_eq!(bad.status_code(), 400);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn notes_are_posted_and_counted() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let project = project_in(&ctx, "acme", "Launch").await;
        request
            .post("/tasks")
            .add_header(owner.0.clone(), owner.1.clone())
            .form(&task_form("Ship it", project.id, ""))
            .await;
        let task = tasks::Entity::find().one(&ctx.db).await.unwrap().unwrap();
        let path = format!("/tasks/{}/notes", task.id);

        let empty = request
            .post(&path)
            .add_header(owner.0.clone(), owner.1.clone())
            .add_header("HX-Request", "true")
            .form(&serde_json::json!({ "body": "   " }))
            .await;
        assert_eq!(empty.status_code(), 422);
        assert_eq!(empty.headers().get("HX-Retarget").unwrap(), "#note-form");

        let res = request
            .post(&path)
            .add_header(owner.0.clone(), owner.1.clone())
            .add_header("HX-Request", "true")
            .form(&serde_json::json!({ "body": "Blocked on <copy> review" }))
            .await;
        assert_eq!(res.status_code(), 200);
        assert!(
            res.text().contains("Blocked on &lt;copy&gt; review"),
            "notes are escaped"
        );
        assert!(res
            .headers()
            .get("HX-Trigger")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("tasks-changed"));
        assert_eq!(task_notes::Entity::find().count(&ctx.db).await.unwrap(), 1);

        let sheet = request
            .get(&format!("/tasks/{}", task.id))
            .add_header(owner.0.clone(), owner.1.clone())
            .await;
        assert!(sheet.text().contains("alice"));
        let list = request
            .get("/tasks/list?filter=all")
            .add_header(owner.0, owner.1)
            .await;
        assert!(list.text().contains("Ship it"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn search_filters_by_title_owner_and_project() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let launch = project_in(&ctx, "acme", "Launch").await;
        let pricing = project_in(&ctx, "acme", "Pricing").await;
        let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();
        for (title, project, who) in [
            ("Email copy", launch.id, ""),
            ("Spreadsheet", pricing.id, alice.id.to_string().as_str()),
        ] {
            request
                .post("/tasks")
                .add_header(owner.0.clone(), owner.1.clone())
                .form(&task_form(title, project, who))
                .await;
        }

        for (q, shown, hidden) in [
            ("email", "Email copy", "Spreadsheet"),
            ("PRICING", "Spreadsheet", "Email copy"),
            ("alice", "Spreadsheet", "Email copy"),
        ] {
            let page = request
                .get(&format!("/tasks?filter=all&q={q}"))
                .add_header(owner.0.clone(), owner.1.clone())
                .await;
            let body = page.text();
            assert!(body.contains(shown), "{q}");
            assert!(!body.contains(hidden), "{q}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn other_orgs_tasks_are_out_of_reach() {
    request::<App, _, _>(|request, ctx| async move {
        let acme = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let globex = sign_up(&request, "Globex", "gina", "gina@example.com").await;
        let project = project_in(&ctx, "globex", "Secret").await;
        request
            .post("/tasks")
            .add_header(globex.0, globex.1)
            .form(&task_form("Secret task", project.id, ""))
            .await;
        let task = tasks::Entity::find().one(&ctx.db).await.unwrap().unwrap();

        assert!(!request
            .get("/tasks")
            .add_header(acme.0.clone(), acme.1.clone())
            .await
            .text()
            .contains("Secret task"));
        assert_eq!(
            request
                .get(&format!("/tasks/{}", task.id))
                .add_header(acme.0.clone(), acme.1.clone())
                .await
                .status_code(),
            404
        );
        let status = request
            .post(&format!("/tasks/{}/status", task.id))
            .add_header(acme.0.clone(), acme.1.clone())
            .form(&serde_json::json!({ "status": "done" }))
            .await;
        assert_eq!(status.status_code(), 404);
        let note = request
            .post(&format!("/tasks/{}/notes", task.id))
            .add_header(acme.0, acme.1)
            .form(&serde_json::json!({ "body": "hi" }))
            .await;
        assert_eq!(note.status_code(), 404);
        assert_eq!(
            tasks::Entity::find_by_id(task.id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap()
                .status,
            "todo"
        );
        assert_eq!(task_notes::Entity::find().count(&ctx.db).await.unwrap(), 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn filters_show_the_right_tasks() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let project = project_in(&ctx, "acme", "Launch").await;
        let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();
        let mine = alice.id.to_string();
        for (title, who) in [
            ("Mine open", mine.as_str()),
            ("Mine stuck", mine.as_str()),
            ("Mine finished", mine.as_str()),
            ("Unowned", ""),
        ] {
            request
                .post("/tasks")
                .add_header(owner.0.clone(), owner.1.clone())
                .form(&task_form(title, project.id, who))
                .await;
        }
        for (title, status) in [("Mine stuck", "blocked"), ("Mine finished", "done")] {
            let task = tasks::Entity::find()
                .all(&ctx.db)
                .await
                .unwrap()
                .into_iter()
                .find(|t| t.title == title)
                .unwrap();
            request
                .post(&format!("/tasks/{}/status", task.id))
                .add_header(owner.0.clone(), owner.1.clone())
                .form(&serde_json::json!({ "status": status }))
                .await;
        }

        let page = |filter: &'static str| {
            let request = &request;
            let cookie = owner.clone();
            async move {
                request
                    .get(&format!("/tasks?filter={filter}"))
                    .add_header(cookie.0, cookie.1)
                    .await
                    .text()
            }
        };
        let mine_page = page("mine").await;
        assert!(mine_page.contains("Mine open") && mine_page.contains("Mine stuck"));
        assert!(!mine_page.contains("Mine finished") && !mine_page.contains("Unowned"));
        let blocked = page("blocked").await;
        assert!(blocked.contains("Mine stuck") && !blocked.contains("Mine open"));
        let done = page("done").await;
        assert!(done.contains("Mine finished") && !done.contains("Mine open"));
        let all = page("all").await;
        assert!(all.contains("Unowned") && !all.contains("Mine finished"));
        assert!(
            all.contains("Mine 2")
                && all.contains("All 3")
                && all.contains("Blocked 1")
                && all.contains("Done 1")
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn changing_status_from_the_sheet_refreshes_it() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let project = project_in(&ctx, "acme", "Launch").await;
        request
            .post("/tasks")
            .add_header(owner.0.clone(), owner.1.clone())
            .form(&task_form("Ship it", project.id, ""))
            .await;
        let task = tasks::Entity::find().one(&ctx.db).await.unwrap().unwrap();

        let res = request
            .post(&format!("/tasks/{}/status", task.id))
            .add_header(owner.0, owner.1)
            .add_header("HX-Request", "true")
            .form(&serde_json::json!({ "status": "blocked" }))
            .await;
        assert_eq!(res.status_code(), 200);
        assert!(res
            .headers()
            .get("HX-Trigger")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("tasks-changed"));
        let compact: String = res.text().split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            compact.contains(r#"aria-checked="true" class="status-blocked""#),
            "the sheet shows the new status"
        );
    })
    .await;
}
