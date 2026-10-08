//! One place that tries every tenant route from organisation A against
//! organisation B's data. Each route must answer 404 (or reject the reference),
//! and B's data must be unchanged afterwards.
use collab::{
    app::App,
    models::{
        conversations, meetings, memberships, messages, organisations, projects, task_notes, tasks,
        users,
    },
};
use loco_rs::{app::AppContext, testing::prelude::*, TestServer};
use sea_orm::{EntityTrait, PaginatorTrait};
use serial_test::serial;

use super::prepare_data::{join, sign_up};

const SECRET: &str = "GLOBEX-SECRET";

/// Everything Globex owns, created through the models.
struct Globex {
    owner: users::Model,
    pending: memberships::Model,
    project: projects::Model,
    task: tasks::Model,
    general: conversations::Model,
    group: conversations::Model,
}

async fn build_globex(request: &TestServer, ctx: &AppContext) -> Globex {
    sign_up(request, "Globex", "gina", "gina@example.com").await;
    join(request, "globex", "hank", "hank@example.com").await;
    let org = organisations::Model::find_by_slug(&ctx.db, "globex")
        .await
        .unwrap();
    let owner = users::Model::find_by_email(&ctx.db, "gina@example.com")
        .await
        .unwrap();
    let hank = users::Model::find_by_email(&ctx.db, "hank@example.com")
        .await
        .unwrap();
    let pending = memberships::Model::find_for_user(&ctx.db, hank.id)
        .await
        .unwrap()
        .unwrap();

    let project = projects::Model::create(
        &ctx.db,
        org.id,
        &projects::ProjectParams {
            name: format!("{SECRET} project"),
            lane: "now".to_string(),
            status: "Active".to_string(),
            accent: "#ffb454".to_string(),
            owner_id: owner.id.to_string(),
            summary: SECRET.to_string(),
        },
    )
    .await
    .unwrap();
    let task = tasks::Model::create(
        &ctx.db,
        org.id,
        &tasks::TaskParams {
            title: format!("{SECRET} task"),
            project_id: project.id.to_string(),
            assignee_ids: vec![owner.id],
            priority: "high".to_string(),
            due_on: String::new(),
            status: String::new(),
        },
    )
    .await
    .unwrap();
    task_notes::Model::create(
        &ctx.db,
        &task,
        owner.id,
        &task_notes::NoteParams {
            body: format!("{SECRET} note"),
        },
    )
    .await
    .unwrap();
    meetings::Model::create(
        &ctx.db,
        org.id,
        owner.id,
        &meetings::MeetingParams {
            title: format!("{SECRET} meeting"),
            held_on: "2026-10-01".to_string(),
            summary: SECRET.to_string(),
            decisions: String::new(),
            attendee_ids: vec![owner.id],
        },
    )
    .await
    .unwrap();
    let general = conversations::Model::find_general(&ctx.db, org.id)
        .await
        .unwrap();
    messages::Model::create(
        &ctx.db,
        &general,
        owner.id,
        &messages::MessageParams {
            body: format!("{SECRET} message"),
        },
    )
    .await
    .unwrap();
    let group = conversations::Model::create_group(
        &ctx.db,
        org.id,
        owner.id,
        &conversations::GroupParams {
            name: format!("{SECRET}-group"),
            member_ids: vec![],
        },
    )
    .await
    .unwrap();
    Globex {
        owner,
        pending,
        project,
        task,
        general,
        group,
    }
}

#[tokio::test]
#[serial]
async fn org_a_cannot_reach_org_b() {
    request::<App, _, _>(|request, ctx| async move {
        let globex = build_globex(&request, &ctx).await;
        let a = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let acme = organisations::Model::find_by_slug(&ctx.db, "acme").await.unwrap();
        let acme_project = projects::Model::create(
            &ctx.db,
            acme.id,
            &projects::ProjectParams {
                name: "Acme project".to_string(),
                lane: "now".to_string(),
                status: "Active".to_string(),
                accent: "#ffb454".to_string(),
                owner_id: String::new(),
                summary: String::new(),
            },
        )
        .await
        .unwrap();
        let b_user = globex.owner.id.to_string();

        // 1. Every page Alice can open shows nothing of Globex.
        for page in [
            "/dashboard", "/roadmap", "/tasks", "/tasks/list", "/tasks/board", "/tasks?view=list", "/tasks?group=person", "/more", "/meetings", "/chat", "/members", "/notifications",
            "/tasks/new", "/meetings/new", "/roadmap/projects/new", "/chat/groups/new", "/chat/dms/new",
        ] {
            let res = request.get(page).add_header(a.0.clone(), a.1.clone()).await;
            assert_eq!(res.status_code(), 200, "{page}");
            let body = res.text();
            assert!(!body.contains(SECRET), "{page} leaks Globex data");
            assert!(!body.contains("gina"), "{page} leaks a Globex member");
        }
        let res = request
            .get(&format!("/chat?c={}", globex.general.id))
            .add_header(a.0.clone(), a.1.clone())
            .await;
        assert!(!res.text().contains(SECRET), "/chat?c= leaks Globex messages");

        // 2. Every route that takes a Globex id answers 404.
        let gets = [
            format!("/roadmap/projects/{}/edit", globex.project.id),
            format!("/tasks/{}", globex.task.id),
            format!("/tasks/{}/edit", globex.task.id),
            format!("/chat/{}/feed", globex.general.id),
            format!("/chat/{}/feed", globex.group.id),
            format!("/chat/{}/ws", globex.general.id),
        ];
        for path in &gets {
            let res = request.get(path).add_header(a.0.clone(), a.1.clone()).await;
            assert_eq!(res.status_code(), 404, "GET {path}");
        }
        let posts = [
            (format!("/roadmap/projects/{}", globex.project.id), serde_json::json!({
                "name": "hijacked", "lane": "now", "status": "x", "accent": "#ffb454", "owner_id": "", "summary": ""
            })),
            (format!("/tasks/{}/status", globex.task.id), serde_json::json!({ "status": "done" })),
            (format!("/tasks/{}", globex.task.id), serde_json::json!({ "title": "hijacked", "project_id": "1", "priority": "high" })),
            (format!("/tasks/{}/delete", globex.task.id), serde_json::json!({})),
            (format!("/tasks/{}/notes", globex.task.id), serde_json::json!({ "body": "hijacked" })),
            (format!("/members/{}/approve", globex.pending.id), serde_json::json!({})),
            (format!("/members/{}/reject", globex.pending.id), serde_json::json!({})),
            (format!("/members/{}/role", globex.pending.id), serde_json::json!({ "role": "admin" })),
            (format!("/chat/{}/messages", globex.general.id), serde_json::json!({ "body": "hijacked" })),
            (format!("/chat/{}/messages", globex.group.id), serde_json::json!({ "body": "hijacked" })),
        ];
        for (path, form) in &posts {
            let res = request.post(path).add_header(a.0.clone(), a.1.clone()).form(form).await;
            assert_eq!(res.status_code(), 404, "POST {path}");
        }

        // 3. Globex records cannot be referenced from Acme forms.
        let refs = [
            ("/tasks", format!("title=t&priority=high&project_id={}", globex.project.id)),
            ("/tasks", format!("title=t&priority=high&project_id={}&assignee_ids={b_user}", acme_project.id)),
            ("/roadmap/projects", format!("name=p&lane=now&status=s&accent=%23ffb454&owner_id={b_user}")),
            ("/meetings", format!("title=m&held_on=2026-10-01&summary=s&attendee_ids={b_user}")),
            ("/chat/groups", format!("name=g&member_ids={b_user}")),
        ];
        for (path, body) in &refs {
            let res = request
                .post(path)
                .add_header(a.0.clone(), a.1.clone())
                .bytes(body.clone().into())
                .content_type("application/x-www-form-urlencoded")
                .await;
            assert_eq!(res.status_code(), 422, "POST {path} {body}");
        }
        let dm = request
            .post("/chat/dms")
            .add_header(a.0.clone(), a.1.clone())
            .form(&serde_json::json!({ "user_id": globex.owner.id }))
            .await;
        assert_eq!(dm.status_code(), 400, "DM to a Globex member");

        // 4. Globex is exactly as it was.
        let project = projects::Entity::find_by_id(globex.project.id).one(&ctx.db).await.unwrap().unwrap();
        assert!(project.name.starts_with(SECRET));
        let task = tasks::Entity::find_by_id(globex.task.id).one(&ctx.db).await.unwrap().unwrap();
        assert_eq!(task.status, "todo");
        let pending = memberships::Entity::find_by_id(globex.pending.id).one(&ctx.db).await.unwrap().unwrap();
        assert_eq!((pending.status.as_str(), pending.role.as_str()), ("pending", "member"));
        assert_eq!(task_notes::Entity::find().count(&ctx.db).await.unwrap(), 1);
        assert_eq!(messages::Entity::find().count(&ctx.db).await.unwrap(), 1);
        assert_eq!(meetings::Entity::find().count(&ctx.db).await.unwrap(), 1);
        assert_eq!(tasks::Entity::find().count(&ctx.db).await.unwrap(), 1);
        assert_eq!(projects::Entity::find().count(&ctx.db).await.unwrap(), 2);
        let dms = conversations::Entity::find()
            .all(&ctx.db)
            .await
            .unwrap()
            .into_iter()
            .filter(|c| c.kind != "channel")
            .count();
        assert_eq!(dms, 1, "only Globex's group exists, no new group or DM");
    })
    .await;
}
