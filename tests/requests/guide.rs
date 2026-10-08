use collab::{
    app::App,
    models::{conversations, organisations},
};
use loco_rs::testing::prelude::*;
use serial_test::serial;

use super::{
    prepare_data::sign_up,
    tasks::{approved_member, project_in, task_form},
};

fn compact(html: &str) -> String {
    html.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[tokio::test]
#[serial]
async fn the_owners_checklist_ticks_itself() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let home = |cookie: (axum::http::HeaderName, axum::http::HeaderValue)| {
            let request = &request;
            async move {
                compact(
                    &request
                        .get("/dashboard")
                        .add_header(cookie.0, cookie.1)
                        .await
                        .text(),
                )
            }
        };

        let first = home(alice.clone()).await;
        assert!(first.contains("Getting started"));
        assert!(first.contains(r#"aria-label="0 of 5 done""#));
        assert!(first.contains("Invite your team") && first.contains("Tag a teammate"));

        // Do some of it: a teammate joins, a project, a task, a hello with a tag.
        approved_member(&request, &ctx, &alice, "bob", "bob@example.com").await;
        let project = project_in(&ctx, "acme", "Launch").await;
        request
            .post("/tasks")
            .add_header(alice.0.clone(), alice.1.clone())
            .form(&task_form("Book the venue", project.id, ""))
            .await;
        let org = organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap();
        let general = conversations::Model::find_general(&ctx.db, org.id)
            .await
            .unwrap();
        request
            .post(&format!("/chat/{}/messages", general.id))
            .add_header(alice.0.clone(), alice.1.clone())
            .form(&serde_json::json!({ "body": "Hello team, welcome @bob" }))
            .await;

        let after = home(alice).await;
        assert!(after.contains(r#"aria-label="5 of 5 done""#), "{after}");
        assert!(after.contains("You're all set."));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn people_who_join_get_their_own_steps() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let (bob, _) = approved_member(&request, &ctx, &alice, "bob", "bob@example.com").await;
        let page = compact(
            &request
                .get("/dashboard")
                .add_header(bob.0, bob.1)
                .await
                .text(),
        );
        assert!(page.contains(r#"aria-label="0 of 4 done""#));
        assert!(page.contains("Take on a task") && page.contains("Add a note to a task"));
        assert!(
            !page.contains("Invite your team"),
            "only owners and admins invite"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn closing_the_guide_is_remembered_until_brought_back() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let (bob, _) = approved_member(&request, &ctx, &alice, "bob", "bob@example.com").await;

        let res = request
            .post("/guide/dismiss")
            .add_header(alice.0.clone(), alice.1.clone())
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(res.text(), "", "the card swaps itself out");
        assert!(res
            .headers()
            .get("HX-Trigger")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("Guide closed"));

        let home = request
            .get("/dashboard")
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(!home.contains(r#"id="guide""#), "stays closed");
        let bobs = request
            .get("/dashboard")
            .add_header(bob.0, bob.1)
            .await
            .text();
        assert!(bobs.contains(r#"id="guide""#), "closing is per person");

        let res = request
            .post("/guide/show")
            .add_header(alice.0.clone(), alice.1.clone())
            .await;
        assert_eq!(res.status_code(), 303);
        let home = request
            .get("/dashboard")
            .add_header(alice.0, alice.1)
            .await
            .text();
        assert!(home.contains(r#"id="guide""#), "back on Home");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_welcome_shows_once() {
    request::<App, _, _>(|request, _ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let first = request
            .get("/dashboard")
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(first.contains("data-welcome") && first.contains("Welcome to Acme"));

        let res = request
            .post("/guide/welcomed")
            .add_header(alice.0.clone(), alice.1.clone())
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 200);
        let again = request
            .get("/dashboard")
            .add_header(alice.0, alice.1)
            .await
            .text();
        assert!(!again.contains("data-welcome"));
    })
    .await;
}
