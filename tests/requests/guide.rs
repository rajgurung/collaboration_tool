use collab::{
    app::App,
    models::{conversations, organisations},
};
use loco_rs::testing::prelude::*;
use serial_test::serial;

use super::{
    prepare_data::{sign_up_member, AsMember, TestMember},
    tasks::{approved_member, project_in, task_form},
};

fn compact(html: &str) -> String {
    html.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[tokio::test]
#[serial]
async fn the_owners_checklist_ticks_itself() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let home = |cookie: TestMember| {
            let request = &request;
            async move {
                compact(
                    &request
                        .get(&cookie.at("/dashboard"))
                        .add_header(cookie.0.clone(), cookie.1.clone())
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
            .post(&alice.at("/tasks"))
            .as_member(&alice)
            .form(&task_form("Book the venue", project.id, ""))
            .await;
        let org = organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap();
        let general = conversations::Model::find_general(&ctx.db, org.id)
            .await
            .unwrap();
        request
            .post(&alice.at(&format!("/chat/{}/messages", general.id)))
            .as_member(&alice)
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
        let alice = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let (bob, _) = approved_member(&request, &ctx, &alice, "bob", "bob@example.com").await;
        let page = compact(
            &request
                .get(&bob.at("/dashboard"))
                .add_header(bob.0.clone(), bob.1.clone())
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
        let alice = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let (bob, _) = approved_member(&request, &ctx, &alice, "bob", "bob@example.com").await;

        let res = request
            .post(&alice.at("/guide/dismiss"))
            .as_member(&alice)
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
            .get(&alice.at("/dashboard"))
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(!home.contains(r#"id="guide""#), "stays closed");
        let bobs = request
            .get(&bob.at("/dashboard"))
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(bobs.contains(r#"id="guide""#), "closing is per person");

        let res = request
            .post(&alice.at("/guide/show"))
            .as_member(&alice)
            .await;
        assert_eq!(res.status_code(), 303);
        let home = request
            .get(&alice.at("/dashboard"))
            .add_header(alice.0.clone(), alice.1.clone())
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
        let alice = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let first = request
            .get(&alice.at("/dashboard"))
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(first.contains("data-welcome") && first.contains("Welcome to Acme"));

        let res = request
            .post(&alice.at("/guide/welcomed"))
            .as_member(&alice)
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 200);
        let again = request
            .get(&alice.at("/dashboard"))
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(!again.contains("data-welcome"));
    })
    .await;
}
