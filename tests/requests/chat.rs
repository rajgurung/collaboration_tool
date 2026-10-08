use collab::{
    app::App,
    data::chat_hub::{ChatEvent, ChatHub},
    models::{
        conversation_members,
        conversations::{self, GroupParams},
        memberships, messages, organisations, users,
    },
};
use loco_rs::{app::AppContext, testing::prelude::*, TestServer};
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
use serial_test::serial;

use super::prepare_data::{join, sign_up};

type Cookie = (axum::http::HeaderName, axum::http::HeaderValue);

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
        assert!(res.text().contains("msg msg-own"));

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
            format!("/chat/{}", group.id)
        );

        // Bob is not in the group: he cannot see it, read it, or post to it.
        let page = request
            .get(&format!("/chat/{}", group.id))
            .add_header(bob.0.clone(), bob.1.clone())
            .await;
        assert_eq!(page.status_code(), 404);
        let list = request
            .get("/chat")
            .add_header(bob.0.clone(), bob.1.clone())
            .await;
        assert!(!list.text().contains("leadership"));
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
        assert!(page.text().contains("Direct message"));

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

#[tokio::test]
#[serial]
async fn websocket_checks_run_before_the_upgrade() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let path = format!("/chat/{}/ws", general.id);

        // Anonymous: sent to login.
        assert_eq!(request.get(&path).await.status_code(), 303);

        // Someone pending in Acme: waiting page.
        let carol = join(&request, "acme", "carol", "carol@example.com").await;
        assert_eq!(
            request
                .get(&path)
                .add_header(carol.0, carol.1)
                .await
                .status_code(),
            403
        );

        // Another organisation's member: the conversation does not exist for them.
        let gina = sign_up(&request, "Globex", "gina", "gina@example.com").await;
        assert_eq!(
            request
                .get(&path)
                .add_header(gina.0, gina.1)
                .await
                .status_code(),
            404
        );

        // Cross-site page trying to open a socket with Alice's cookie.
        let evil = request
            .get(&path)
            .add_header(alice.0.clone(), alice.1.clone())
            .add_header("Origin", "https://evil.example")
            .await;
        assert_eq!(evil.status_code(), 403);

        // Alice passes every check; without upgrade headers the request is just not a WebSocket.
        let plain = request
            .get(&path)
            .add_header(alice.0, alice.1)
            .add_header("Origin", "http://localhost:5150")
            .await;
        assert_eq!(plain.status_code(), 400);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn http_sends_are_published_and_the_feed_reloads() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let hub = ctx.shared_store.get::<ChatHub>().unwrap();
        let mut events = hub.subscribe();

        request
            .post(&format!("/chat/{}/messages", general.id))
            .add_header(alice.0, alice.1)
            .form(&serde_json::json!({ "body": "Standup in 5" }))
            .await;
        let ChatEvent::Message {
            conversation_id,
            message,
            ..
        } = events.try_recv().expect("the message is broadcast")
        else {
            panic!("expected a message event");
        };
        assert_eq!(conversation_id, general.id);
        assert_eq!(message.body, "Standup in 5");
        assert_eq!(message.author, "alice");

        let feed = request
            .get(&format!("/chat/{}/feed", general.id))
            .add_header(bob.0, bob.1)
            .await;
        assert_eq!(feed.status_code(), 200);
        assert!(feed.text().contains("Standup in 5"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn unread_counts_follow_reading() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        // Bob has opened general once, so only newer messages count as unread.
        request
            .get(&format!("/chat/{}", general.id))
            .add_header(bob.0.clone(), bob.1.clone())
            .await;

        for body in ["First", "Second"] {
            request
                .post(&format!("/chat/{}/messages", general.id))
                .add_header(alice.0.clone(), alice.1.clone())
                .form(&serde_json::json!({ "body": body }))
                .await;
        }

        let badge = request
            .get("/chat/unread?style=tab")
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(
            badge.contains(r#"class="tab-badge""#) && badge.contains(">2<"),
            "{badge}"
        );
        let mine = request
            .get("/chat/unread")
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert_eq!(mine, "", "your own messages are never unread");

        let list = request
            .get("/chat")
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(
            list.contains(r#"aria-label="2 unread""#),
            "the list shows the count"
        );

        // Opening the conversation clears it.
        request
            .get(&format!("/chat/{}", general.id))
            .add_header(bob.0.clone(), bob.1.clone())
            .await;
        let badge = request
            .get("/chat/unread")
            .add_header(bob.0, bob.1)
            .await
            .text();
        assert_eq!(badge, "");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn legacy_chat_links_redirect() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let res = request
            .get(&format!("/chat?c={}", general.id))
            .add_header(alice.0, alice.1)
            .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(
            res.headers().get("location").unwrap().to_str().unwrap(),
            format!("/chat/{}", general.id)
        );
    })
    .await;
}

async fn approve(ctx: &AppContext, email: &str) -> users::Model {
    let user = users::Model::find_by_email(&ctx.db, email).await.unwrap();
    let membership = memberships::Model::find_for_user(&ctx.db, user.id)
        .await
        .unwrap()
        .unwrap();
    membership.approve(&ctx.db, user.id).await.unwrap();
    user
}

async fn user_id(ctx: &AppContext, email: &str) -> i64 {
    users::Model::find_by_email(&ctx.db, email)
        .await
        .unwrap()
        .id
}

async fn acme_id(ctx: &AppContext) -> i64 {
    organisations::Model::find_by_slug(&ctx.db, "acme")
        .await
        .unwrap()
        .id
}

/// Alice's group with bob and carol; returns the group and carol's cookie.
async fn group_of_three(request: &TestServer, ctx: &AppContext) -> (conversations::Model, Cookie) {
    let carol = join(request, "acme", "carol", "carol@example.com").await;
    let carol_id = approve(ctx, "carol@example.com").await.id;
    let group = conversations::Model::create_group(
        &ctx.db,
        acme_id(ctx).await,
        user_id(ctx, "alice@example.com").await,
        &GroupParams {
            name: "launch".to_string(),
            member_ids: vec![user_id(ctx, "bob@example.com").await, carol_id],
        },
    )
    .await
    .unwrap();
    (group, carol)
}

async fn post(
    request: &TestServer,
    ctx: &AppContext,
    who: &Cookie,
    conversation_id: i64,
    body: &str,
) -> messages::Model {
    request
        .post(&format!("/chat/{conversation_id}/messages"))
        .add_header(who.0.clone(), who.1.clone())
        .form(&serde_json::json!({ "body": body }))
        .await;
    messages::Entity::find()
        .filter(messages::Column::Body.eq(body))
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap()
}

async fn open(request: &TestServer, who: &Cookie, conversation_id: i64) -> String {
    request
        .get(&format!("/chat/{conversation_id}"))
        .add_header(who.0.clone(), who.1.clone())
        .await
        .text()
}

/// The receipt slot for one message as rendered: it ends its meta row.
fn receipt_slot(html: &str, message_id: i64) -> &str {
    let start = html
        .find(&format!(r#"id="receipt-{message_id}""#))
        .expect("the message has a receipt slot");
    let end = start + html[start..].find("</div>").unwrap();
    &html[start..end]
}

#[tokio::test]
#[serial]
async fn dms_show_read_once_the_other_person_opens_them() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let dm = conversations::Model::start_dm(
            &ctx.db,
            acme_id(&ctx).await,
            user_id(&ctx, "alice@example.com").await,
            user_id(&ctx, "bob@example.com").await,
        )
        .await
        .unwrap();
        let hello = post(&request, &ctx, &alice, dm.id, "Got a minute?").await;

        let page = open(&request, &alice, dm.id).await;
        assert!(
            !receipt_slot(&page, hello.id).contains("Read"),
            "bob has not opened it"
        );

        let bobs = open(&request, &bob, dm.id).await;
        assert!(
            !bobs.contains("msg-receipt"),
            "only the sender sees receipts"
        );

        let page = open(&request, &alice, dm.id).await;
        let slot = receipt_slot(&page, hello.id);
        assert!(
            slot.contains("</svg>Read</span>"),
            "DMs keep the word: {slot}"
        );
        assert!(!slot.contains("avatar"), "{slot}");

        // A message sent after bob last read is not read yet.
        let later = post(&request, &ctx, &alice, dm.id, "Never mind").await;
        let page = open(&request, &alice, dm.id).await;
        assert!(!receipt_slot(&page, later.id).contains("Read"));
        assert!(receipt_slot(&page, hello.id).contains("Read"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn groups_count_who_has_read() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let (group, carol) = group_of_three(&request, &ctx).await;
        let message = post(&request, &ctx, &alice, group.id, "Launch is Friday").await;

        open(&request, &bob, group.id).await;
        let page = open(&request, &alice, group.id).await;
        let slot = receipt_slot(&page, message.id);
        assert!(slot.contains(r#"aria-label="Read by bob""#), "{slot}");
        assert!(slot.contains("avatar avatar-2xs"), "{slot}");
        assert!(!slot.contains("receipt-more"), "{slot}");
        assert!(!page.contains("msg-receipt-all"));

        let carols = open(&request, &carol, group.id).await;
        assert!(
            !carols.contains("msg-receipt"),
            "only the sender sees receipts"
        );
        let page = open(&request, &alice, group.id).await;
        let slot = receipt_slot(&page, message.id);
        assert!(slot.contains(r#"aria-label="Read by everyone""#), "{slot}");
        assert_eq!(slot.matches("avatar avatar-2xs").count(), 2, "{slot}");
        assert!(
            slot.contains(r#"id="receipt-names-"#)
                && slot.contains("<span>bob</span>")
                && slot.contains("<span>carol</span>"),
            "the reader list to hover or tap open: {slot}"
        );
        assert!(page.contains("msg-receipt msg-receipt-all"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn general_shows_receipts_like_groups() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        // Dave is still pending. Even with a read mark in General he never counts.
        join(&request, "acme", "dave", "dave@example.com").await;
        let dave_id = user_id(&ctx, "dave@example.com").await;
        conversation_members::Model::add(&ctx.db, &general, dave_id)
            .await
            .unwrap();
        let message = post(&request, &ctx, &alice, general.id, "Morning all").await;
        conversation_members::Model::mark_read(
            &ctx.db,
            general.organisation_id,
            general.id,
            dave_id,
        )
        .await
        .unwrap();

        let page = open(&request, &alice, general.id).await;
        let slot = receipt_slot(&page, message.id);
        assert!(!slot.contains("avatar"), "dave is pending: {slot}");

        open(&request, &bob, general.id).await;
        let page = open(&request, &alice, general.id).await;
        let slot = receipt_slot(&page, message.id);
        assert!(slot.contains(r#"aria-label="Read by everyone""#), "{slot}");
        assert_eq!(slot.matches("avatar avatar-2xs").count(), 1, "{slot}");
        assert!(slot.contains("<span>bob</span>"), "{slot}");
        assert!(!slot.contains("dave"), "{slot}");
        assert!(page.contains("msg-receipt msg-receipt-all"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn reading_publishes_receipts_for_the_authors() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let (group, _carol) = group_of_three(&request, &ctx).await;
        let general = general(&ctx).await;
        let alice_id = user_id(&ctx, "alice@example.com").await;
        let hub = ctx.shared_store.get::<ChatHub>().unwrap();
        let mut events = hub.subscribe();

        let message = post(&request, &ctx, &alice, group.id, "Launch is Friday").await;
        post(&request, &ctx, &alice, general.id, "Morning all").await;
        while events.try_recv().is_ok() {}

        // Alice opening her own conversation reads nothing new from others.
        open(&request, &alice, group.id).await;
        assert!(events.try_recv().is_err());

        open(&request, &bob, group.id).await;
        let ChatEvent::Read {
            conversation_id,
            receipts,
        } = events.try_recv().expect("bob's read is published")
        else {
            panic!("expected a read event");
        };
        assert_eq!(conversation_id, group.id);
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].message_id, message.id);
        assert_eq!(receipts[0].author_id, alice_id);
        assert_eq!(receipts[0].receipt.as_ref().unwrap().text, "Read by bob");

        // Nothing new since: nothing published.
        request
            .get(&format!("/chat/{}/feed", group.id))
            .add_header(bob.0.clone(), bob.1.clone())
            .await;
        assert!(events.try_recv().is_err());

        // General works the same way.
        open(&request, &bob, general.id).await;
        let ChatEvent::Read {
            conversation_id,
            receipts,
        } = events
            .try_recv()
            .expect("bob's read of General is published")
        else {
            panic!("expected a read event");
        };
        assert_eq!(conversation_id, general.id);
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].author_id, alice_id);
        // Carol is in General too and has not read it.
        assert_eq!(receipts[0].receipt.as_ref().unwrap().text, "Read by bob");
    })
    .await;
}
