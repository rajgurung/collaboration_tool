//! One place that tries every tenant route from organisation A against
//! organisation B's data. Each route must answer 404 (or reject the reference),
//! and B's data must be unchanged afterwards.
use axum_test::multipart::{MultipartForm, Part};
use collab::{
    app::App,
    models::{
        conversations, documents, folders, meetings, memberships, messages, note_reactions,
        organisations, projects, reactions, task_notes, tasks, users,
    },
};
use loco_rs::{app::AppContext, testing::prelude::*, TestServer};
use sea_orm::{EntityTrait, PaginatorTrait};
use serial_test::serial;

use super::prepare_data::{join, sign_up_member, AsMember};

const SECRET: &str = "GLOBEX-SECRET";

/// Everything Globex owns, created through the models.
struct Globex {
    owner: users::Model,
    pending: memberships::Model,
    project: projects::Model,
    task: tasks::Model,
    note: task_notes::Model,
    general: conversations::Model,
    message: messages::Model,
    group: conversations::Model,
    folder: folders::Model,
    file: documents::Model,
}

async fn build_globex(request: &TestServer, ctx: &AppContext) -> Globex {
    sign_up_member(request, "Globex", "gina", "gina@example.com").await;
    join(request, ctx, "globex", "hank", "hank@example.com").await;
    let org = organisations::Model::find_by_slug(&ctx.db, "globex")
        .await
        .unwrap();
    let owner = users::Model::find_by_email(&ctx.db, "gina@example.com")
        .await
        .unwrap();
    let hank = users::Model::find_by_email(&ctx.db, "hank@example.com")
        .await
        .unwrap();
    let pending = memberships::Model::default_for_user(&ctx.db, hank.id)
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
    let note = task_notes::Model::create(
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
    let message = messages::Model::create(
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
    let folder = folders::Model::create(
        &ctx.db,
        org.id,
        owner.id,
        &folders::FolderParams {
            name: format!("{SECRET} folder"),
            note: SECRET.to_string(),
        },
    )
    .await
    .unwrap();
    let file = documents::Model::upload(
        ctx,
        &folder,
        owner.id,
        &format!("{SECRET}.pdf"),
        bytes::Bytes::from_static(super::documents::PDF),
    )
    .await
    .unwrap();
    Globex {
        folder,
        file,
        owner,
        pending,
        project,
        task,
        note,
        general,
        message,
        group,
    }
}

#[tokio::test]
#[serial]
async fn org_a_cannot_reach_org_b() {
    request::<App, _, _>(|request, ctx| async move {
        let globex = build_globex(&request, &ctx).await;
        let a = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
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
            "/tasks/new", "/meetings/new", "/roadmap/projects/new", "/chat/groups/new", "/chat/dms/new", "/documents",
        ] {
            let res = request.get(page).add_header(a.0.clone(), a.1.clone()).await;
            assert_eq!(res.status_code(), 200, "{page}");
            let body = res.text();
            assert!(!body.contains(SECRET), "{page} leaks Globex data");
            // As a name or an email, not a substring: "Argentina" is a time zone.
            for gina in [r#""gina""#, ">gina<", "gina@"] {
                assert!(!body.contains(gina), "{page} leaks a Globex member");
            }
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
            format!("/chat/{}/messages/{}/actions", globex.general.id, globex.message.id),
            format!("/chat/{}/threads/{}", globex.general.id, globex.message.id),
            format!("/chat/{}/threads/{}/feed", globex.general.id, globex.message.id),
            format!("/chat/{}/ws?thread={}", globex.general.id, globex.message.id),
            format!("/documents/folders/{}", globex.folder.id),
            format!("/documents/files/{}", globex.file.id),
            format!("/documents/files/{}?download=1", globex.file.id),
        ];
        for path in &gets {
            let res = request.get(path).add_header(a.0.clone(), a.1.clone()).await;
            assert_eq!(res.status_code(), 404, "GET {path}");
            assert!(!res.text().contains(SECRET), "GET {path} leaks Globex data");
        }
        let posts = [
            (format!("/roadmap/projects/{}", globex.project.id), serde_json::json!({
                "name": "hijacked", "lane": "now", "status": "x", "accent": "#ffb454", "owner_id": "", "summary": ""
            })),
            (format!("/tasks/{}/status", globex.task.id), serde_json::json!({ "status": "done" })),
            (format!("/tasks/{}", globex.task.id), serde_json::json!({ "title": "hijacked", "project_id": "1", "priority": "high" })),
            (format!("/tasks/{}/delete", globex.task.id), serde_json::json!({})),
            (format!("/tasks/{}/notes", globex.task.id), serde_json::json!({ "body": "hijacked" })),
            (format!("/tasks/{}/notes/{}/reactions", globex.task.id, globex.note.id), serde_json::json!({ "emoji": "👍", "on": "1" })),
            (format!("/members/{}/approve", globex.pending.id), serde_json::json!({})),
            (format!("/members/{}/reject", globex.pending.id), serde_json::json!({})),
            (format!("/members/{}/role", globex.pending.id), serde_json::json!({ "role": "admin" })),
            (format!("/members/{}/remove", globex.pending.id), serde_json::json!({})),
            (format!("/documents/folders/{}", globex.folder.id), serde_json::json!({ "name": "hijacked", "note": "" })),
            (format!("/documents/folders/{}/delete", globex.folder.id), serde_json::json!({})),
            (format!("/documents/files/{}/rename", globex.file.id), serde_json::json!({ "name": "hijacked" })),
            (format!("/documents/files/{}/delete", globex.file.id), serde_json::json!({})),
            (format!("/chat/{}/messages", globex.general.id), serde_json::json!({ "body": "hijacked" })),
            (format!("/chat/{}/messages", globex.group.id), serde_json::json!({ "body": "hijacked" })),
            (format!("/chat/{}/messages/{}/reactions", globex.general.id, globex.message.id), serde_json::json!({ "emoji": "👍", "on": "1" })),
            (format!("/chat/{}/threads/{}/replies", globex.general.id, globex.message.id), serde_json::json!({ "body": "hijacked" })),
        ];
        for (path, form) in &posts {
            let res = request.post(path).as_member(&a).form(form).await;
            assert_eq!(res.status_code(), 404, "POST {path}");
        }
        // Alice's own task with Globex's note.
        let acme_task = tasks::Model::create(
            &ctx.db,
            acme.id,
            &tasks::TaskParams {
                title: "Acme task".to_string(),
                project_id: String::new(),
                assignee_ids: vec![],
                priority: "low".to_string(),
                due_on: String::new(),
                status: String::new(),
            },
        )
        .await
        .unwrap();
        let res = request
            .post(&format!("/tasks/{}/notes/{}/reactions", acme_task.id, globex.note.id))
            .as_member(&a)
            .form(&serde_json::json!({ "emoji": "👍", "on": "1" }))
            .await;
        assert_eq!(res.status_code(), 404, "own task, Globex note");
        assert!(!res.text().contains(SECRET), "own task, Globex note");

        let res = request
            .post(&format!("/documents/folders/{}/files", globex.folder.id))
            .as_member(&a)
            .multipart(MultipartForm::new().add_part("file", Part::bytes(super::documents::PDF.to_vec()).file_name("x.pdf")))
            .await;
        assert_eq!(res.status_code(), 404, "upload into a Globex folder");

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
                .as_member(&a)
                .bytes(body.clone().into())
                .content_type("application/x-www-form-urlencoded")
                .await;
            assert_eq!(res.status_code(), 422, "POST {path} {body}");
        }
        let dm = request
            .post("/chat/dms")
            .as_member(&a)
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
        assert_eq!(reactions::Entity::find().count(&ctx.db).await.unwrap(), 0);
        assert_eq!(note_reactions::Entity::find().count(&ctx.db).await.unwrap(), 0);
        assert_eq!(meetings::Entity::find().count(&ctx.db).await.unwrap(), 1);
        assert_eq!(tasks::Entity::find().count(&ctx.db).await.unwrap(), 2, "Globex's and alice's");
        assert_eq!(projects::Entity::find().count(&ctx.db).await.unwrap(), 2);
        let dms = conversations::Entity::find()
            .all(&ctx.db)
            .await
            .unwrap()
            .into_iter()
            .filter(|c| c.kind != "channel")
            .count();
        assert_eq!(dms, 1, "only Globex's group exists, no new group or DM");
        let folder = folders::Entity::find_by_id(globex.folder.id).one(&ctx.db).await.unwrap().unwrap();
        assert!(folder.name.starts_with(SECRET));
        let file = documents::Entity::find_by_id(globex.file.id).one(&ctx.db).await.unwrap().unwrap();
        assert!(file.name.starts_with(SECRET));
        assert_eq!(documents::Entity::find().count(&ctx.db).await.unwrap(), 1);
        assert!(ctx.storage.download::<Vec<u8>>(std::path::Path::new(&file.storage_key)).await.is_ok());
    })
    .await;
}

/// Every Documents route, and removing a member, refuses people who are not
/// approved members of the organisation: logged out, waiting, removed, or a
/// super admin acting inside it.
#[tokio::test]
#[serial]
async fn documents_and_removal_need_your_own_active_membership() {
    request::<App, _, _>(|request, ctx| async move {
        let globex = build_globex(&request, &ctx).await;
        let hank = super::prepare_data::login(
            &request,
            "hank@example.com",
            super::prepare_data::USER_PASSWORD,
        )
        .await;
        // Ivy was a member until gina removed her.
        let ivy_cookie = join(&request, &ctx, "globex", "ivy", "ivy@example.com").await;
        let ivy = users::Model::find_by_email(&ctx.db, "ivy@example.com")
            .await
            .unwrap();
        let ivy_m = memberships::Model::default_for_user(&ctx.db, ivy.id)
            .await
            .unwrap()
            .unwrap();
        let ivy_m = ivy_m.approve(&ctx.db, globex.owner.id).await.unwrap();
        memberships::Model::remove(
            &ctx,
            globex.folder.organisation_id,
            globex.owner.id,
            ivy_m.id,
            &Default::default(),
        )
        .await
        .unwrap();
        let ivy = super::prepare_data::login(
            &request,
            "ivy@example.com",
            super::prepare_data::USER_PASSWORD,
        )
        .await;
        assert_ne!(ivy, ivy_cookie.cookie);
        let admin = super::admin::super_admin_cookie(&request, &ctx).await;
        let acting = format!(
            "{}; acting_org={}",
            admin.1.to_str().unwrap(),
            globex.folder.organisation_id
        );

        let gets = [
            "/documents".to_string(),
            format!("/documents/folders/{}", globex.folder.id),
            format!("/documents/files/{}", globex.file.id),
        ];
        let posts = [
            "/documents/folders".to_string(),
            format!("/documents/folders/{}", globex.folder.id),
            format!("/documents/folders/{}/delete", globex.folder.id),
            format!("/documents/folders/{}/files", globex.folder.id),
            format!("/documents/files/{}/rename", globex.file.id),
            format!("/documents/files/{}/delete", globex.file.id),
            format!("/members/{}/remove", globex.pending.id),
        ];
        let form = serde_json::json!({ "name": "hijacked", "note": "" });
        for path in &gets {
            let res = request.get(path).await;
            assert_eq!(res.status_code(), 303, "logged out GET {path}");
            assert_eq!(res.headers()["location"], "/login");
            for (who, cookie) in [("waiting", &hank), ("removed", &ivy)] {
                let res = request
                    .get(path)
                    .add_header(cookie.0.clone(), cookie.1.clone())
                    .await;
                assert_eq!(res.status_code(), 403, "{who} GET {path}");
                assert!(!res.text().contains(SECRET), "{who} GET {path}");
            }
            let res = request.get(path).add_header("cookie", acting.clone()).await;
            assert_eq!(res.status_code(), 403, "acting GET {path}");
            assert!(!res.text().contains(SECRET), "acting GET {path}");
        }
        // Uploads get a real file, so only the membership check can refuse them.
        let send = |path: &str, cookie: Option<String>| {
            let mut req = request.post(path);
            if let Some(cookie) = cookie {
                req = req.add_header("cookie", cookie);
            }
            if path.ends_with("/files") {
                req.multipart(MultipartForm::new().add_part(
                    "file",
                    Part::bytes(super::documents::PDF.to_vec()).file_name("x.pdf"),
                ))
            } else {
                req.form(&form)
            }
        };
        for path in &posts {
            assert_eq!(
                send(path, None).await.status_code(),
                303,
                "logged out POST {path}"
            );
            for (who, cookie) in [("waiting", &hank), ("removed", &ivy)] {
                let res = send(path, Some(cookie.1.to_str().unwrap().to_string())).await;
                assert_eq!(res.status_code(), 403, "{who} POST {path}");
            }
            let res = send(path, Some(acting.clone()))
                .add_header("x-org-id", globex.folder.organisation_id.to_string())
                .await;
            assert_eq!(res.status_code(), 403, "acting POST {path}");
        }
        assert_eq!(
            request
                .post("/account/sign-out-everywhere")
                .await
                .status_code(),
            303,
            "logged out"
        );

        // Nothing changed.
        let folder = folders::Entity::find_by_id(globex.folder.id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert!(folder.name.starts_with(SECRET));
        let file = documents::Entity::find_by_id(globex.file.id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert!(file.name.starts_with(SECRET));
        let globex_folders = folders::Model::list_for_org(&ctx.db, globex.folder.organisation_id)
            .await
            .unwrap();
        assert_eq!(
            globex_folders.len(),
            3,
            "Company, Logins and the secret one"
        );
        let pending = memberships::Entity::find_by_id(globex.pending.id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pending.status, "pending");
    })
    .await;
}
