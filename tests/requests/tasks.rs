use collab::{
    app::App,
    models::{memberships, organisations, projects, task_assignees, task_notes, tasks, users},
};
use loco_rs::{app::AppContext, testing::prelude::*};
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
use serial_test::serial;

use super::prepare_data::sign_up;

pub(super) async fn project_in(ctx: &AppContext, slug: &str, name: &str) -> projects::Model {
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
            accent: "#ffb454".to_string(),
            owner_id: String::new(),
            summary: String::new(),
        },
    )
    .await
    .unwrap()
}

/// A task form; an empty `assignee` leaves the field out, as an unticked form does.
pub(super) fn task_form(title: &str, project_id: i64, assignee: &str) -> serde_json::Value {
    let mut form = serde_json::json!({
        "title": title, "project_id": project_id.to_string(),
        "priority": "high", "due_on": "2026-11-03",
    });
    if !assignee.is_empty() {
        form["assignee_ids"] = assignee.into();
    }
    form
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

        // It shows under "Mine" (alice is assigned) in the To do group, with its due date.
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
        assert_eq!(
            task_assignees::Model::for_task(&ctx.db, task.organisation_id, task.id)
                .await
                .unwrap(),
            vec![alice.id]
        );
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
                    .get(&format!("/tasks?view=list&filter={filter}"))
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
        let checked = compact
            .split(r#"aria-checked="true""#)
            .nth(1)
            .expect("one status is checked");
        assert!(
            checked.contains(r#""status": "blocked""#),
            "the sheet shows the new status"
        );
    })
    .await;
}

/// Joins `email` to Acme and has the owner approve them. Returns their cookie and user id.
pub(super) async fn approved_member(
    request: &loco_rs::TestServer,
    ctx: &AppContext,
    owner: &(axum::http::HeaderName, axum::http::HeaderValue),
    name: &str,
    email: &str,
) -> ((axum::http::HeaderName, axum::http::HeaderValue), i64) {
    let cookie = super::prepare_data::join(request, "acme", name, email).await;
    let user = users::Model::find_by_email(&ctx.db, email).await.unwrap();
    let membership = memberships::Entity::find()
        .filter(memberships::Column::UserId.eq(user.id))
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    request
        .post(&format!("/members/{}/approve", membership.id))
        .add_header(owner.0.clone(), owner.1.clone())
        .await;
    (cookie, user.id)
}

pub(super) fn form_body(fields: &[(&str, String)]) -> axum::body::Bytes {
    fields
        .iter()
        .map(|(k, v)| format!("{k}={}", v.replace(' ', "+")))
        .collect::<Vec<_>>()
        .join("&")
        .into()
}

#[tokio::test]
#[serial]
async fn editing_saves_every_field_and_several_assignees() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let (_bob, bob_id) =
            approved_member(&request, &ctx, &owner, "bob", "bob@example.com").await;
        let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();
        let project = project_in(&ctx, "acme", "Launch").await;
        let other = project_in(&ctx, "acme", "Mobile").await;
        request
            .post("/tasks")
            .add_header(owner.0.clone(), owner.1.clone())
            .form(&task_form("Draft email", project.id, &alice.id.to_string()))
            .await;
        let task = tasks::Entity::find().one(&ctx.db).await.unwrap().unwrap();

        let edit = request
            .get(&format!("/tasks/{}/edit", task.id))
            .add_header(owner.0.clone(), owner.1.clone())
            .await;
        assert_eq!(edit.status_code(), 200);
        assert!(edit.text().contains("Draft email") && edit.text().contains("Save changes"));

        let res = request
            .post(&format!("/tasks/{}", task.id))
            .add_header(owner.0.clone(), owner.1.clone())
            .add_header("HX-Request", "true")
            .content_type("application/x-www-form-urlencoded")
            .bytes(form_body(&[
                ("title", "Draft the launch email".into()),
                ("project_id", other.id.to_string()),
                ("priority", "low".into()),
                ("due_on", String::new()),
                ("status", "progress".into()),
                ("assignee_ids", alice.id.to_string()),
                ("assignee_ids", bob_id.to_string()),
            ]))
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let trigger = res
            .headers()
            .get("HX-Trigger")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(trigger.contains("Task saved") && trigger.contains("tasks-changed"));
        let sheet = res.text();
        assert!(
            sheet.contains("Draft the launch email")
                && sheet.contains("alice")
                && sheet.contains("bob")
        );

        let task = tasks::Entity::find_by_id(task.id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(task.title, "Draft the launch email");
        assert_eq!(task.project_id, other.id);
        assert_eq!(task.priority, "low");
        assert_eq!(task.status, "progress");
        assert_eq!(task.due_on, None);
        assert_eq!(
            task_assignees::Model::for_task(&ctx.db, task.organisation_id, task.id)
                .await
                .unwrap(),
            vec![alice.id, bob_id]
        );

        // Unticking alice leaves bob alone on it.
        request
            .post(&format!("/tasks/{}", task.id))
            .add_header(owner.0.clone(), owner.1.clone())
            .content_type("application/x-www-form-urlencoded")
            .bytes(form_body(&[
                ("title", "Draft the launch email".into()),
                ("project_id", other.id.to_string()),
                ("priority", "low".into()),
                ("assignee_ids", bob_id.to_string()),
            ]))
            .await;
        assert_eq!(
            task_assignees::Model::for_task(&ctx.db, task.organisation_id, task.id)
                .await
                .unwrap(),
            vec![bob_id]
        );
        let task = tasks::Entity::find_by_id(task.id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            task.status, "progress",
            "a form without a status keeps the current one"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn editing_rejects_people_outside_the_team() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        super::prepare_data::join(&request, "acme", "pat", "pat@example.com").await;
        let pending = users::Model::find_by_email(&ctx.db, "pat@example.com")
            .await
            .unwrap();
        let project = project_in(&ctx, "acme", "Launch").await;
        request
            .post("/tasks")
            .add_header(owner.0.clone(), owner.1.clone())
            .form(&task_form("Draft email", project.id, ""))
            .await;
        let task = tasks::Entity::find().one(&ctx.db).await.unwrap().unwrap();

        let res = request
            .post(&format!("/tasks/{}", task.id))
            .add_header(owner.0.clone(), owner.1.clone())
            .add_header("HX-Request", "true")
            .content_type("application/x-www-form-urlencoded")
            .bytes(form_body(&[
                ("title", "Draft email".into()),
                ("project_id", project.id.to_string()),
                ("priority", "high".into()),
                ("assignee_ids", pending.id.to_string()),
            ]))
            .await;
        assert_eq!(res.status_code(), 422);
        assert!(res.text().contains("Choose people from the team."));
        assert!(
            task_assignees::Model::for_task(&ctx.db, task.organisation_id, task.id)
                .await
                .unwrap()
                .is_empty()
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn mine_means_any_task_i_am_assigned_to() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let (bob, bob_id) = approved_member(&request, &ctx, &owner, "bob", "bob@example.com").await;
        let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();
        let project = project_in(&ctx, "acme", "Launch").await;
        request
            .post("/tasks")
            .add_header(owner.0.clone(), owner.1.clone())
            .content_type("application/x-www-form-urlencoded")
            .bytes(form_body(&[
                ("title", "Shared work".into()),
                ("project_id", project.id.to_string()),
                ("priority", "high".into()),
                ("assignee_ids", alice.id.to_string()),
                ("assignee_ids", bob_id.to_string()),
            ]))
            .await;
        request
            .post("/tasks")
            .add_header(owner.0.clone(), owner.1.clone())
            .form(&task_form("Alice alone", project.id, &alice.id.to_string()))
            .await;

        let bobs_list = request
            .get("/tasks/list?filter=mine")
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(bobs_list.contains("Shared work") && !bobs_list.contains("Alice alone"));
        let bobs_board = request
            .get("/tasks/board?scope=mine")
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(bobs_board.contains("Shared work") && !bobs_board.contains("Alice alone"));
        let bobs_lanes = request
            .get("/tasks/board?scope=mine&group=person")
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(
            bobs_lanes.contains(&format!(r#"data-lane="person-{bob_id}""#))
                && !bobs_lanes.contains(&format!(r#"data-lane="person-{}""#, alice.id)),
            "a shared task shows only in my own lane"
        );
        let everyone_by_person = request
            .get("/tasks/board?scope=all&group=person")
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(everyone_by_person.contains(&format!(r#"data-lane="person-{}""#, alice.id)));
        let everyone = request
            .get("/tasks/board?scope=all")
            .add_header(bob.0, bob.1)
            .await
            .text();
        assert!(everyone.contains("Shared work") && everyone.contains("Alice alone"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn deleting_a_task_takes_its_notes_and_assignees() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();
        let project = project_in(&ctx, "acme", "Launch").await;
        request
            .post("/tasks")
            .add_header(owner.0.clone(), owner.1.clone())
            .form(&task_form("Throwaway", project.id, &alice.id.to_string()))
            .await;
        let task = tasks::Entity::find().one(&ctx.db).await.unwrap().unwrap();
        request
            .post(&format!("/tasks/{}/notes", task.id))
            .add_header(owner.0.clone(), owner.1.clone())
            .form(&serde_json::json!({ "body": "A note" }))
            .await;

        let res = request
            .post(&format!("/tasks/{}/delete", task.id))
            .add_header(owner.0.clone(), owner.1.clone())
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 200);
        let trigger = res
            .headers()
            .get("HX-Trigger")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(trigger.contains("close-dialogs") && trigger.contains("Task deleted"));
        assert_eq!(tasks::Entity::find().count(&ctx.db).await.unwrap(), 0);
        assert_eq!(task_notes::Entity::find().count(&ctx.db).await.unwrap(), 0);
        assert_eq!(
            task_assignees::Entity::find().count(&ctx.db).await.unwrap(),
            0
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_board_groups_by_project_person_or_nothing() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();
        let launch = project_in(&ctx, "acme", "Launch").await;
        let mobile = project_in(&ctx, "acme", "Mobile").await;
        project_in(&ctx, "acme", "Empty project").await;
        request
            .post("/tasks")
            .add_header(owner.0.clone(), owner.1.clone())
            .form(&task_form("Launch work", launch.id, &alice.id.to_string()))
            .await;
        request
            .post("/tasks")
            .add_header(owner.0.clone(), owner.1.clone())
            .form(&task_form("Nobody's work", mobile.id, ""))
            .await;

        let board = |group: &'static str| {
            let request = &request;
            let cookie = owner.clone();
            async move {
                request
                    .get(&format!("/tasks/board?group={group}"))
                    .add_header(cookie.0, cookie.1)
                    .await
                    .text()
            }
        };
        let by_project = board("project").await;
        assert!(
            by_project.contains(r#"aria-label="Launch""#)
                && by_project.contains(r#"aria-label="Mobile""#)
        );
        assert!(
            !by_project.contains("Empty project"),
            "projects without tasks get no lane"
        );
        for column in ["To do", "In progress", "Blocked", "Done"] {
            assert!(by_project.contains(column), "{column} column");
        }
        let by_person = board("person").await;
        assert!(
            by_person.contains(r#"aria-label="alice""#)
                && by_person.contains(r#"aria-label="Unassigned""#)
        );
        let flat = board("none").await;
        assert!(
            flat.contains(r#"aria-label="All tasks""#)
                && flat.contains("Launch work")
                && flat.contains("s work")
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn adding_from_a_column_starts_in_that_column() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let project = project_in(&ctx, "acme", "Launch").await;
        let form = request
            .get(&format!("/tasks/new?status=blocked&project_id={}", project.id))
            .add_header(owner.0.clone(), owner.1.clone())
            .await
            .text();
        let compact: String = form.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(compact.contains(r#"value="blocked" checked"#), "blocked is preselected");
        assert!(compact.contains(&format!(r#"value="{}" selected"#, project.id)), "project is preselected");

        request
            .post("/tasks")
            .add_header(owner.0.clone(), owner.1.clone())
            .form(&serde_json::json!({
                "title": "Stuck already", "project_id": project.id.to_string(), "priority": "high", "status": "blocked",
            }))
            .await;
        let task = tasks::Entity::find().one(&ctx.db).await.unwrap().unwrap();
        assert_eq!(task.status, "blocked");
    })
    .await;
}
