use collab::{
    app::App,
    models::{conversations, notifications, organisations, tasks, users},
};
use loco_rs::{app::AppContext, testing::prelude::*};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serial_test::serial;

use super::{
    prepare_data::sign_up,
    tasks::{approved_member, form_body, project_in, task_form},
};

/// (kind, body) of someone's notifications, oldest first.
async fn inbox(ctx: &AppContext, user_id: i64) -> Vec<(String, String)> {
    notifications::Entity::find()
        .filter(notifications::Column::UserId.eq(user_id))
        .order_by_asc(notifications::Column::Id)
        .all(&ctx.db)
        .await
        .unwrap()
        .into_iter()
        .map(|n| (n.kind, n.body))
        .collect()
}

async fn user_id(ctx: &AppContext, email: &str) -> i64 {
    users::Model::find_by_email(&ctx.db, email)
        .await
        .unwrap()
        .id
}

#[tokio::test]
#[serial]
async fn chat_mentions_notify_people_who_can_see_the_conversation() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let (bob, bob_id) = approved_member(&request, &ctx, &alice, "bob", "bob@example.com").await;
        let (_, carol_id) =
            approved_member(&request, &ctx, &alice, "carol", "carol@example.com").await;
        let alice_id = user_id(&ctx, "alice@example.com").await;
        let org = organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap();
        let general = conversations::Model::find_general(&ctx.db, org.id)
            .await
            .unwrap();

        let res = request
            .post(&format!("/chat/{}/messages", general.id))
            .add_header(alice.0.clone(), alice.1.clone())
            .add_header("HX-Request", "true")
            .form(&serde_json::json!({ "body": "Can @bob and @Carol look? cc @alice @nobody" }))
            .await;
        assert!(
            res.text().contains(r#"<span class="mention">@bob</span>"#),
            "mentions are highlighted"
        );
        assert_eq!(
            inbox(&ctx, bob_id).await,
            vec![(
                "mention".to_string(),
                "mentioned you in #general\nCan @bob and @Carol look? cc @alice @nobody"
                    .to_string()
            )]
        );
        assert_eq!(inbox(&ctx, carol_id).await.len(), 1);
        assert!(inbox(&ctx, alice_id).await.is_empty(), "never yourself");

        // A group bob is not in: mentioning him there tells him nothing.
        request
            .post("/chat/groups")
            .add_header(alice.0.clone(), alice.1.clone())
            .form(&serde_json::json!({ "name": "leads" }))
            .await;
        let group = conversations::Entity::find()
            .filter(conversations::Column::Kind.eq("group"))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        request
            .post(&format!("/chat/{}/messages", group.id))
            .add_header(alice.0.clone(), alice.1.clone())
            .form(&serde_json::json!({ "body": "secret plan for @bob" }))
            .await;
        assert_eq!(inbox(&ctx, bob_id).await.len(), 1);

        let page = request
            .get("/notifications")
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(page.contains("mentioned you in #general"));
        let badge = request
            .get("/notifications/unread")
            .add_header(bob.0, bob.1)
            .await
            .text();
        assert!(badge.contains(">1<"), "{badge}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn assignments_and_notes_notify_the_right_people() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let (_, bob_id) = approved_member(&request, &ctx, &alice, "bob", "bob@example.com").await;
        let (carol, carol_id) =
            approved_member(&request, &ctx, &alice, "carol", "carol@example.com").await;
        let alice_id = user_id(&ctx, "alice@example.com").await;
        let project = project_in(&ctx, "acme", "Launch").await;

        request
            .post("/tasks")
            .add_header(alice.0.clone(), alice.1.clone())
            .content_type("application/x-www-form-urlencoded")
            .bytes(form_body(&[
                ("title", "Write the FAQ".into()),
                ("project_id", project.id.to_string()),
                ("priority", "high".into()),
                ("assignee_ids", alice_id.to_string()),
                ("assignee_ids", bob_id.to_string()),
            ]))
            .await;
        let task = tasks::Entity::find().one(&ctx.db).await.unwrap().unwrap();
        assert_eq!(
            inbox(&ctx, bob_id).await,
            vec![(
                "assigned".to_string(),
                "assigned you to “Write the FAQ”".to_string()
            )]
        );
        assert!(inbox(&ctx, alice_id).await.is_empty());

        // Adding carol tells carol only; bob was already on it.
        request
            .post(&format!("/tasks/{}", task.id))
            .add_header(alice.0.clone(), alice.1.clone())
            .content_type("application/x-www-form-urlencoded")
            .bytes(form_body(&[
                ("title", "Write the FAQ".into()),
                ("project_id", project.id.to_string()),
                ("priority", "high".into()),
                ("status", "todo".into()),
                ("assignee_ids", alice_id.to_string()),
                ("assignee_ids", bob_id.to_string()),
                ("assignee_ids", carol_id.to_string()),
            ]))
            .await;
        assert_eq!(inbox(&ctx, bob_id).await.len(), 1);
        assert_eq!(inbox(&ctx, carol_id).await.len(), 1);

        // Carol's note mentions alice: alice gets the mention, bob the note, carol nothing new.
        request
            .post(&format!("/tasks/{}/notes", task.id))
            .add_header(carol.0.clone(), carol.1.clone())
            .form(&serde_json::json!({ "body": "Draft is up, @alice please review" }))
            .await;
        assert_eq!(
            inbox(&ctx, alice_id).await,
            vec![(
                "mention".to_string(),
                "mentioned you on “Write the FAQ”\nDraft is up, @alice please review".to_string()
            )]
        );
        assert_eq!(inbox(&ctx, bob_id).await[1].0, "note");
        assert_eq!(inbox(&ctx, carol_id).await.len(), 1);

        let sheet = request
            .get(&format!("/tasks/{}", task.id))
            .add_header(carol.0, carol.1)
            .await
            .text();
        assert!(sheet.contains(r#"<span class="mention">@alice</span>"#));
        assert!(
            sheet.contains(r#"data-mentions="alice,bob""#),
            "picker offers the team"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn project_owners_hear_about_new_and_blocked_tasks() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let (_, bob_id) = approved_member(&request, &ctx, &alice, "bob", "bob@example.com").await;
        request
            .post("/roadmap/projects")
            .add_header(alice.0.clone(), alice.1.clone())
            .form(&serde_json::json!({
                "name": "Launch", "lane": "now", "status": "In progress", "progress": "40",
                "accent": "#72e5b4", "owner_id": bob_id.to_string(), "summary": "",
            }))
            .await;
        assert_eq!(
            inbox(&ctx, bob_id).await,
            vec![("owner".to_string(), "made you owner of Launch".to_string())]
        );
        let project = collab::models::projects::Entity::find()
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();

        request
            .post("/tasks")
            .add_header(alice.0.clone(), alice.1.clone())
            .form(&task_form("Book the venue", project.id, ""))
            .await;
        let task = tasks::Entity::find().one(&ctx.db).await.unwrap().unwrap();
        for _ in 0..2 {
            request
                .post(&format!("/tasks/{}/status", task.id))
                .add_header(alice.0.clone(), alice.1.clone())
                .form(&serde_json::json!({ "status": "blocked" }))
                .await;
        }
        let bodies: Vec<String> = inbox(&ctx, bob_id).await.into_iter().map(|n| n.1).collect();
        assert_eq!(
            bodies,
            vec![
                "made you owner of Launch",
                "added “Book the venue” to Launch",
                "marked “Book the venue” as blocked in Launch",
            ],
            "blocked twice is one notification"
        );

        // An owner who is also assigned gets one notification, not two.
        request
            .post("/tasks")
            .add_header(alice.0.clone(), alice.1.clone())
            .form(&task_form("Print badges", project.id, &bob_id.to_string()))
            .await;
        let last = inbox(&ctx, bob_id).await;
        assert_eq!(last.len(), 4);
        assert_eq!(last[3].0, "assigned");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn opening_marks_read_and_stays_private() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let (bob, bob_id) = approved_member(&request, &ctx, &alice, "bob", "bob@example.com").await;
        let gina = sign_up(&request, "Globex", "gina", "gina@example.com").await;
        let project = project_in(&ctx, "acme", "Launch").await;
        for title in ["One", "Two"] {
            request
                .post("/tasks")
                .add_header(alice.0.clone(), alice.1.clone())
                .form(&task_form(title, project.id, &bob_id.to_string()))
                .await;
        }
        let first = notifications::Entity::find()
            .order_by_asc(notifications::Column::Id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        let task = tasks::Entity::find()
            .order_by_asc(tasks::Column::Id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();

        // Not yours, or another organisation's: not found, and still unread.
        for cookie in [&alice, &gina] {
            let res = request
                .post(&format!("/notifications/{}/open", first.id))
                .add_header(cookie.0.clone(), cookie.1.clone())
                .await;
            assert_eq!(res.status_code(), 404);
            let page = request
                .get("/notifications")
                .add_header(cookie.0.clone(), cookie.1.clone())
                .await
                .text();
            assert!(!page.contains("assigned you"));
        }

        let res = request
            .post(&format!("/notifications/{}/open", first.id))
            .add_header(bob.0.clone(), bob.1.clone())
            .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(
            res.headers().get("location").unwrap(),
            &format!("/tasks?open={}", task.id)
        );
        let opened = request
            .get(&format!("/tasks?open={}", task.id))
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(opened.contains(&format!(
            r##"hx-get="/tasks/{}" hx-target="#task-sheet-body" hx-trigger="load""##,
            task.id
        )));

        let badge = |cookie: (axum::http::HeaderName, axum::http::HeaderValue)| {
            let request = &request;
            async move {
                request
                    .get("/notifications/unread")
                    .add_header(cookie.0, cookie.1)
                    .await
                    .text()
            }
        };
        assert!(badge(bob.clone()).await.contains(">1<"));
        request
            .post("/notifications/read")
            .add_header(bob.0.clone(), bob.1.clone())
            .await;
        assert_eq!(badge(bob).await, "");
    })
    .await;
}
