use collab::{
    app::App,
    models::{conversations, memberships, messages, organisations, users},
};
use loco_rs::{app::AppContext, testing::prelude::*, TestServer};
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
use serial_test::serial;

use super::prepare_data::{join, sign_up};

/// Signs up Acme (alice) and gets bob approved; returns both cookies.
async fn acme_with_bob(
    request: &TestServer,
    ctx: &AppContext,
) -> (
    (axum::http::HeaderName, axum::http::HeaderValue),
    (axum::http::HeaderName, axum::http::HeaderValue),
) {
    let alice = sign_up(request, "Acme", "alice", "alice@example.com").await;
    let bob = join(request, "acme", "bob", "bob@example.com").await;
    let bob_user = users::Model::find_by_email(&ctx.db, "bob@example.com")
        .await
        .unwrap();
    let membership = memberships::Model::find_for_user(&ctx.db, bob_user.id)
        .await
        .unwrap()
        .unwrap();
    membership.approve(&ctx.db, bob_user.id).await.unwrap();
    (alice, bob)
}

async fn general(ctx: &AppContext) -> conversations::Model {
    let org = organisations::Model::find_by_slug(&ctx.db, "acme")
        .await
        .unwrap();
    conversations::Model::find_general(&ctx.db, org.id)
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn chat_opens_on_general_and_shows_messages() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;

        let res = request
            .post(&format!("/chat/{}/messages", general.id))
            .add_header(alice.0.clone(), alice.1.clone())
            .add_header("HX-Request", "true")
            .form(&serde_json::json!({ "body": "Hello <team>" }))
            .await;
        assert_eq!(res.status_code(), 200);
        assert!(res.text().contains("Hello &lt;team&gt;"));
        assert!(res.text().contains("chat-bubble own"));

        let page = request.get("/chat").add_header(bob.0, bob.1).await;
        assert_eq!(page.status_code(), 200);
        let body = page.text();
        assert!(body.contains("general"));
        assert!(body.contains("Hello &lt;team&gt;"));
        assert!(body.contains("alice"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn empty_and_overlong_messages_are_rejected() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        for body in ["   ".to_string(), "x".repeat(1001)] {
            let res = request
                .post(&format!("/chat/{}/messages", general.id))
                .add_header(alice.0.clone(), alice.1.clone())
                .add_header("HX-Request", "true")
                .form(&serde_json::json!({ "body": body }))
                .await;
            assert_eq!(res.status_code(), 422);
            assert_eq!(res.headers().get("HX-Reswap").unwrap(), "none");
        }
        assert_eq!(messages::Entity::find().count(&ctx.db).await.unwrap(), 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn groups_are_private_to_their_members() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let res = request
            .post("/chat/groups")
            .add_header(alice.0.clone(), alice.1.clone())
            .form(&serde_json::json!({ "name": "#leadership" }))
            .await;
        assert_eq!(res.status_code(), 303);
        let group = conversations::Entity::find()
            .filter(conversations::Column::Kind.eq("group"))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(group.name.as_deref(), Some("leadership"));
        assert_eq!(
            res.headers().get("location").unwrap().to_str().unwrap(),
            format!("/chat?c={}", group.id)
        );

        // Bob is not in the group: he cannot see it, read it, or post to it.
        let page = request
            .get(&format!("/chat?c={}", group.id))
            .add_header(bob.0.clone(), bob.1.clone())
            .await;
        assert!(!page.text().contains("leadership"));
        let post = request
            .post(&format!("/chat/{}/messages", group.id))
            .add_header(bob.0, bob.1)
            .form(&serde_json::json!({ "body": "let me in" }))
            .await;
        assert_eq!(post.status_code(), 404);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn group_members_must_be_on_the_team() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _bob) = acme_with_bob(&request, &ctx).await;
        sign_up(&request, "Globex", "gina", "gina@example.com").await;
        let gina = users::Model::find_by_email(&ctx.db, "gina@example.com")
            .await
            .unwrap();
        let res = request
            .post("/chat/groups")
            .add_header(alice.0, alice.1)
            .add_header("HX-Request", "true")
            .bytes(format!("name=planning&member_ids={}", gina.id).into())
            .content_type("application/x-www-form-urlencoded")
            .await;
        assert_eq!(res.status_code(), 422);
        assert!(res.text().contains("Members must be on the team."));
        let groups = conversations::Entity::find()
            .filter(conversations::Column::Kind.eq("group"))
            .count(&ctx.db)
            .await
            .unwrap();
        assert_eq!(groups, 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn starting_a_dm_twice_reuses_it() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let bob_user = users::Model::find_by_email(&ctx.db, "bob@example.com")
            .await
            .unwrap();
        let alice_user = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();

        let first = request
            .post("/chat/dms")
            .add_header(alice.0.clone(), alice.1.clone())
            .form(&serde_json::json!({ "user_id": bob_user.id }))
            .await;
        let second = request
            .post("/chat/dms")
            .add_header(bob.0.clone(), bob.1.clone())
            .form(&serde_json::json!({ "user_id": alice_user.id }))
            .await;
        assert_eq!(
            first.headers().get("location"),
            second.headers().get("location")
        );
        let dms = conversations::Entity::find()
            .filter(conversations::Column::Kind.eq("dm"))
            .count(&ctx.db)
            .await
            .unwrap();
        assert_eq!(dms, 1);

        // The DM is labelled with the other person's name.
        let location = first
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let page = request.get(&location).add_header(bob.0, bob.1).await;
        assert!(page.text().contains("Private conversation"));

        let to_self = request
            .post("/chat/dms")
            .add_header(alice.0, alice.1)
            .form(&serde_json::json!({ "user_id": alice_user.id }))
            .await;
        assert_eq!(to_self.status_code(), 400);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn chat_does_not_cross_organisations() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _bob) = acme_with_bob(&request, &ctx).await;
        let globex = sign_up(&request, "Globex", "gina", "gina@example.com").await;
        let acme_general = general(&ctx).await;
        let alice_user = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();

        let post = request
            .post(&format!("/chat/{}/messages", acme_general.id))
            .add_header(globex.0.clone(), globex.1.clone())
            .form(&serde_json::json!({ "body": "hi from outside" }))
            .await;
        assert_eq!(post.status_code(), 404);
        let dm = request
            .post("/chat/dms")
            .add_header(globex.0, globex.1)
            .form(&serde_json::json!({ "user_id": alice_user.id }))
            .await;
        assert_eq!(dm.status_code(), 400);
        let page = request.get("/chat").add_header(alice.0, alice.1).await;
        assert!(!page.text().contains("hi from outside"));
    })
    .await;
}
