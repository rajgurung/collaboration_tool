use collab::{app::App, models::saved_views};
use loco_rs::testing::prelude::*;
use sea_orm::{EntityTrait, PaginatorTrait};
use serial_test::serial;

use super::{
    prepare_data::sign_up,
    tasks::{approved_member, project_in, task_form},
};

type Cookie = (axum::http::HeaderName, axum::http::HeaderValue);

/// Saves a view from a form like the page's, returning the status and redirect.
async fn save(
    request: &loco_rs::TestServer,
    cookie: &Cookie,
    form: &[(&str, &str)],
) -> (u16, String) {
    let body: String = form
        .iter()
        .map(|(k, v)| format!("{k}={}", v.replace(' ', "+")))
        .collect::<Vec<_>>()
        .join("&");
    let res = request
        .post("/tasks/views")
        .add_header(cookie.0.clone(), cookie.1.clone())
        .content_type("application/x-www-form-urlencoded")
        .bytes(body.into())
        .await;
    let location = res
        .headers()
        .get("location")
        .map(|l| l.to_str().unwrap().to_string())
        .unwrap_or_default();
    (res.status_code().as_u16(), location)
}

#[tokio::test]
#[serial]
async fn a_saved_view_brings_its_setup_back() {
    request::<App, _, _>(|request, _ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let res = save(
            &request,
            &alice,
            &[
                ("name", "My lanes"),
                ("scope", "mine"),
                ("group", "person"),
                ("view", "board"),
                ("q", ""),
            ],
        )
        .await;
        assert_eq!(
            res,
            (
                303,
                "/tasks?view=board&scope=mine&group=person&filter=mine".to_string()
            )
        );

        let page = request
            .get("/tasks?view=board&scope=mine&group=person&filter=mine")
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(
            page.contains(r#"class="view-chip view-chip-on""#),
            "the matching view is lit"
        );
        let other = request
            .get("/tasks?scope=all")
            .add_header(alice.0, alice.1)
            .await
            .text();
        assert!(other.contains("My lanes") && !other.contains("view-chip-on"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_default_view_opens_tasks() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        for (name, default) in [("First", "on"), ("Mine by person", "on")] {
            save(
                &request,
                &alice,
                &[
                    ("name", name),
                    ("is_default", default),
                    ("scope", "mine"),
                    ("group", "person"),
                    ("view", "list"),
                ],
            )
            .await;
        }
        let defaults = saved_views::Entity::find()
            .all(&ctx.db)
            .await
            .unwrap()
            .into_iter()
            .filter(|v| v.is_default)
            .count();
        assert_eq!(defaults, 1, "only one default at a time");

        let plain = request
            .get("/tasks")
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(plain.contains(r#"id="task-table""#), "opens as a list");
        assert!(plain.contains("Group by"));

        let chosen = request
            .get("/tasks?scope=all&group=project")
            .add_header(alice.0, alice.1)
            .await
            .text();
        assert!(
            chosen.contains(r#"id="task-board""#),
            "an explicit setup wins over the default"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn views_are_named_limited_and_private() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let (bob, _) = approved_member(&request, &ctx, &alice, "bob", "bob@example.com").await;

        let res = request
            .post("/tasks/views")
            .add_header(alice.0.clone(), alice.1.clone())
            .add_header("HX-Request", "true")
            .form(&serde_json::json!({ "name": "  ", "scope": "all" }))
            .await;
        assert_eq!(res.status_code(), 422);
        assert!(res.text().contains("Give the view a short name"));

        for i in 0..10 {
            save(
                &request,
                &alice,
                &[("name", &format!("View {i}")), ("scope", "all")],
            )
            .await;
        }
        let res = request
            .post("/tasks/views")
            .add_header(alice.0.clone(), alice.1.clone())
            .add_header("HX-Request", "true")
            .form(&serde_json::json!({ "name": "One too many", "scope": "all" }))
            .await;
        assert_eq!(res.status_code(), 422);
        assert!(res.text().contains("up to 10 views"));
        assert_eq!(
            saved_views::Entity::find().count(&ctx.db).await.unwrap(),
            10
        );

        // Bob sees none of them and can't touch them.
        let view = saved_views::Entity::find()
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        let bobs = request
            .get("/tasks")
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(!bobs.contains("View 0"));
        for path in [
            format!("/tasks/views/{}", view.id),
            format!("/tasks/views/{}/delete", view.id),
        ] {
            let res = request
                .post(&path)
                .add_header(bob.0.clone(), bob.1.clone())
                .form(&serde_json::json!({ "name": "hijacked" }))
                .await;
            assert_eq!(res.status_code(), 404, "{path}");
        }
        assert_eq!(
            saved_views::Entity::find().count(&ctx.db).await.unwrap(),
            10
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn views_can_be_renamed_updated_and_deleted() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        save(&request, &alice, &[("name", "Mine"), ("scope", "mine"), ("group", "project")]).await;
        let view = saved_views::Entity::find().one(&ctx.db).await.unwrap().unwrap();

        let edit = request
            .get(&format!("/tasks/views/{}/edit?scope=all&group=none&view=list", view.id))
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(edit.contains("Update to what") && edit.contains("Delete view"));

        request
            .post(&format!("/tasks/views/{}", view.id))
            .add_header(alice.0.clone(), alice.1.clone())
            .form(&serde_json::json!({ "name": "Everything", "is_default": "on" }))
            .await;
        request
            .post(&format!("/tasks/views/{}/setup", view.id))
            .add_header(alice.0.clone(), alice.1.clone())
            .form(&serde_json::json!({ "scope": "all", "group": "none", "view": "list", "q": "venue" }))
            .await;
        let saved = saved_views::Entity::find().one(&ctx.db).await.unwrap().unwrap();
        assert_eq!(saved.name, "Everything");
        assert!(saved.is_default);
        assert_eq!(
            (saved.scope.as_str(), saved.lanes.as_str(), saved.layout.as_str(), saved.q.as_str()),
            ("all", "none", "list", "venue")
        );

        let res = request
            .post(&format!("/tasks/views/{}/delete", view.id))
            .add_header(alice.0, alice.1)
            .form(&serde_json::json!({ "scope": "all" }))
            .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(saved_views::Entity::find().count(&ctx.db).await.unwrap(), 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_desktop_list_shows_lanes_as_rows() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let launch = project_in(&ctx, "acme", "Launch").await;
        request
            .post("/tasks")
            .add_header(alice.0.clone(), alice.1.clone())
            .form(&task_form("Book the venue", launch.id, ""))
            .await;
        let list = request
            .get("/tasks/board?view=list&scope=all&group=project")
            .add_header(alice.0, alice.1)
            .await
            .text();
        assert!(list.contains(r#"id="task-table""#));
        assert!(list.contains(r#"class="task-row""#) && list.contains("Book the venue"));
        assert!(
            list.contains(r#"aria-label="Launch""#),
            "grouped like the board"
        );
        assert!(
            list.contains("view=list&amp;scope=mine") || list.contains("view=list&scope=mine"),
            "filters keep the list"
        );
    })
    .await;
}
