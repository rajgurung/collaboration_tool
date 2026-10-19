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

use super::prepare_data::{join, sign_up_member, AsMember};

pub(super) use super::prepare_data::TestMember;

/// Signs up Acme (alice) and gets bob approved; returns both cookies.
pub(super) async fn acme_with_bob(
    request: &TestServer,
    ctx: &AppContext,
) -> (TestMember, TestMember) {
    let alice = sign_up_member(request, "Acme", "alice", "alice@example.com").await;
    let bob = join(request, ctx, "acme", "bob", "bob@example.com").await;
    let bob_user = users::Model::find_by_email(&ctx.db, "bob@example.com")
        .await
        .unwrap();
    let membership = memberships::Model::default_for_user(&ctx.db, bob_user.id)
        .await
        .unwrap()
        .unwrap();
    membership.approve(&ctx.db, bob_user.id).await.unwrap();
    (alice, bob)
}

pub(super) async fn general(ctx: &AppContext) -> conversations::Model {
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
            .as_member(&alice)
            .add_header("HX-Request", "true")
            .form(&serde_json::json!({ "body": "Hello <team>" }))
            .await;
        assert_eq!(res.status_code(), 200);
        assert!(res.text().contains("Hello &lt;team&gt;"));
        assert!(res.text().contains("msg msg-own"));

        let page = request
            .get("/chat")
            .add_header(bob.0.clone(), bob.1.clone())
            .await;
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
                .as_member(&alice)
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
            .as_member(&alice)
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
            .as_member(&bob)
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
        sign_up_member(&request, "Globex", "gina", "gina@example.com").await;
        let gina = users::Model::find_by_email(&ctx.db, "gina@example.com")
            .await
            .unwrap();
        let res = request
            .post("/chat/groups")
            .as_member(&alice)
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
            .as_member(&alice)
            .form(&serde_json::json!({ "user_id": bob_user.id }))
            .await;
        let second = request
            .post("/chat/dms")
            .as_member(&bob)
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
        let page = request
            .get(&location)
            .add_header(bob.0.clone(), bob.1.clone())
            .await;
        assert!(page.text().contains("Direct message"));

        let to_self = request
            .post("/chat/dms")
            .as_member(&alice)
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
        let globex = sign_up_member(&request, "Globex", "gina", "gina@example.com").await;
        let acme_general = general(&ctx).await;
        let alice_user = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();

        let post = request
            .post(&format!("/chat/{}/messages", acme_general.id))
            .as_member(&globex)
            .form(&serde_json::json!({ "body": "hi from outside" }))
            .await;
        assert_eq!(post.status_code(), 404);
        let dm = request
            .post("/chat/dms")
            .as_member(&globex)
            .form(&serde_json::json!({ "user_id": alice_user.id }))
            .await;
        assert_eq!(dm.status_code(), 400);
        let page = request
            .get("/chat")
            .add_header(alice.0.clone(), alice.1.clone())
            .await;
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
        let carol = join(&request, &ctx, "acme", "carol", "carol@example.com").await;
        assert_eq!(
            request
                .get(&path)
                .add_header(carol.0.clone(), carol.1.clone())
                .await
                .status_code(),
            403
        );

        // Another organisation's member: the conversation does not exist for them.
        let gina = sign_up_member(&request, "Globex", "gina", "gina@example.com").await;
        assert_eq!(
            request
                .get(&path)
                .add_header(gina.0.clone(), gina.1.clone())
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

        // A thread socket checks the thread too: it must be a top-level
        // message in this conversation.
        let parent = post(&request, &ctx, &alice, general.id, "Top").await;
        let gina_thread = request
            .get(&format!("{path}?thread={}", parent.id))
            .add_header(gina.0.clone(), gina.1.clone())
            .await;
        assert_eq!(gina_thread.status_code(), 404);
        for (thread, status) in [(parent.id.to_string(), 400), ("987654".to_string(), 404)] {
            let res = request
                .get(&format!("{path}?thread={thread}"))
                .add_header(alice.0.clone(), alice.1.clone())
                .add_header("Origin", "http://localhost:5150")
                .await;
            assert_eq!(res.status_code(), status, "thread {thread}");
        }

        // Alice passes every check; without upgrade headers the request is just not a WebSocket.
        let plain = request
            .get(&path)
            .add_header(alice.0.clone(), alice.1.clone())
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
            .as_member(&alice)
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
            .add_header(bob.0.clone(), bob.1.clone())
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
                .as_member(&alice)
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
            .add_header(bob.0.clone(), bob.1.clone())
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
            .add_header(alice.0.clone(), alice.1.clone())
            .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(
            res.headers().get("location").unwrap().to_str().unwrap(),
            format!("/chat/{}", general.id)
        );
    })
    .await;
}

pub(super) async fn approve(ctx: &AppContext, email: &str) -> users::Model {
    let user = users::Model::find_by_email(&ctx.db, email).await.unwrap();
    let membership = memberships::Model::default_for_user(&ctx.db, user.id)
        .await
        .unwrap()
        .unwrap();
    membership.approve(&ctx.db, user.id).await.unwrap();
    user
}

pub(super) async fn user_id(ctx: &AppContext, email: &str) -> i64 {
    users::Model::find_by_email(&ctx.db, email)
        .await
        .unwrap()
        .id
}

pub(super) async fn acme_id(ctx: &AppContext) -> i64 {
    organisations::Model::find_by_slug(&ctx.db, "acme")
        .await
        .unwrap()
        .id
}

/// Alice's group with bob and carol; returns the group and carol's cookie.
async fn group_of_three(
    request: &TestServer,
    ctx: &AppContext,
) -> (conversations::Model, TestMember) {
    let carol = join(request, ctx, "acme", "carol", "carol@example.com").await;
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

pub(super) async fn post(
    request: &TestServer,
    ctx: &AppContext,
    who: &TestMember,
    conversation_id: i64,
    body: &str,
) -> messages::Model {
    request
        .post(&format!("/chat/{conversation_id}/messages"))
        .as_member(who)
        .form(&serde_json::json!({ "body": body }))
        .await;
    messages::Entity::find()
        .filter(messages::Column::Body.eq(body))
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap()
}

async fn open(request: &TestServer, who: &TestMember, conversation_id: i64) -> String {
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

/// The tick inside one message's bubble.
fn tick_slot(html: &str, message_id: i64) -> &str {
    let id = html
        .find(&format!(r#"id="tick-{message_id}""#))
        .expect("the message has a tick");
    let start = html[..id].rfind("<span").unwrap();
    let end = id + html[id..].find("</span></span>").unwrap() + "</span></span>".len();
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
        let tick = tick_slot(&page, hello.id);
        assert!(tick.starts_with(r#"<span class="tick" "#), "{tick}");
        assert!(
            tick.contains("not read yet"),
            "bob has not opened it: {tick}"
        );

        let page = open(&request, &alice, dm.id).await;
        let slot = receipt_slot(&page, hello.id);
        assert!(!slot.contains("Read"), "nothing under it yet: {slot}");

        let bobs = open(&request, &bob, dm.id).await;
        assert!(
            !bobs.contains("msg-receipt") && !bobs.contains(r#"id="tick-"#),
            "in a DM only the sender sees receipts"
        );

        let page = open(&request, &alice, dm.id).await;
        let tick = tick_slot(&page, hello.id);
        assert!(
            tick.starts_with(r#"<span class="tick tick-read""#),
            "{tick}"
        );
        assert!(tick.contains(", read</span>"), "{tick}");
        let slot = receipt_slot(&page, hello.id);
        assert!(!slot.contains("avatar"), "a DM shows no faces: {slot}");
        assert!(
            slot.contains(r#"<span class="msg-receipt-dm">"#) && slot.contains("Read</span>"),
            "a DM says Read under the sender's message: {slot}"
        );

        // Bob's reply: alice, who did not write it, sees no receipt on it.
        let reply = post(&request, &ctx, &bob, dm.id, "Sure").await;
        let page = open(&request, &alice, dm.id).await;
        assert!(!page.contains(&format!(r#"id="receipt-{}""#, reply.id)));
        assert!(!page.contains(&format!(r#"id="tick-{}""#, reply.id)));

        // A message sent after bob last read is not read yet.
        let later = post(&request, &ctx, &alice, dm.id, "Never mind").await;
        let page = open(&request, &alice, dm.id).await;
        assert!(!tick_slot(&page, later.id).contains("tick-read"));
        assert!(tick_slot(&page, hello.id).contains("tick-read"));
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
        assert!(slot.contains("Read by 1"), "{slot}");
        assert!(!slot.contains("seen-more"), "{slot}");
        assert!(
            !tick_slot(&page, message.id).contains("tick-read"),
            "carol has not read it"
        );
        assert!(
            tick_slot(&page, message.id).contains(", Read by bob</span>"),
            "the tick says who has read it so far"
        );

        // Everyone in a group sees who read every message, their own face
        // included, but the tick stays on the sender's page.
        let carols = open(&request, &carol, group.id).await;
        let slot = receipt_slot(&carols, message.id);
        assert!(
            slot.contains(r#"aria-label="Read by bob, carol""#),
            "{slot}"
        );
        assert!(!carols.contains(r#"id="tick-"#), "{carols}");
        assert!(!carols.contains("stamp-pad-tick"), "{carols}");
        let page = open(&request, &alice, group.id).await;
        let slot = receipt_slot(&page, message.id);
        assert!(
            slot.contains(r#"aria-label="Read by bob, carol""#),
            "{slot}"
        );
        assert_eq!(slot.matches("avatar avatar-2xs").count(), 2, "{slot}");
        assert!(
            slot.contains(r#"id="receipt-names-"#)
                && slot.contains("<span>bob</span>")
                && slot.contains("<span>carol</span>"),
            "the reader list to hover or tap open: {slot}"
        );
        assert!(!slot.contains("everyone"), "{slot}");
        assert!(tick_slot(&page, message.id).contains("tick-read"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn late_joiners_and_pending_people_do_not_block_the_read_tick() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let (group, carol) = group_of_three(&request, &ctx).await;
        // Erin is pending, with an old read mark in the group.
        join(&request, &ctx, "acme", "erin", "erin@example.com").await;
        let erin_id = user_id(&ctx, "erin@example.com").await;
        conversation_members::Model::add(&ctx.db, &group, erin_id)
            .await
            .unwrap();
        conversation_members::Model::mark_read(&ctx.db, group.organisation_id, group.id, erin_id)
            .await
            .unwrap();

        let message = post(&request, &ctx, &alice, group.id, "Launch is Friday").await;
        open(&request, &bob, group.id).await;
        open(&request, &carol, group.id).await;
        // Dave joins the group after the message and has not read it.
        join(&request, &ctx, "acme", "dave", "dave@example.com").await;
        let dave_id = approve(&ctx, "dave@example.com").await.id;
        conversation_members::Model::add(&ctx.db, &group, dave_id)
            .await
            .unwrap();

        let page = open(&request, &alice, group.id).await;
        let tick = tick_slot(&page, message.id);
        assert!(
            tick.starts_with(r#"<span class="tick tick-read""#),
            "{tick}"
        );
        assert!(tick.contains("read by everyone"), "{tick}");
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
        join(&request, &ctx, "acme", "dave", "dave@example.com").await;
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

        let bobs = open(&request, &bob, general.id).await;
        let slot = receipt_slot(&bobs, message.id);
        assert!(
            slot.contains(r#"aria-label="Read by bob""#),
            "bob sees his own face: {slot}"
        );
        let page = open(&request, &alice, general.id).await;
        let slot = receipt_slot(&page, message.id);
        assert!(slot.contains(r#"aria-label="Read by bob""#), "{slot}");
        assert_eq!(slot.matches("avatar avatar-2xs").count(), 1, "{slot}");
        assert!(slot.contains("<span>bob</span>"), "{slot}");
        assert!(!slot.contains("dave"), "{slot}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn reading_publishes_receipts() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let (group, _carol) = group_of_three(&request, &ctx).await;
        let general = general(&ctx).await;
        let alice_id = user_id(&ctx, "alice@example.com").await;
        // Dave is pending but has a read mark in General: he must never count.
        join(&request, &ctx, "acme", "dave", "dave@example.com").await;
        let dave_id = user_id(&ctx, "dave@example.com").await;
        conversation_members::Model::add(&ctx.db, &general, dave_id)
            .await
            .unwrap();
        let hub = ctx.shared_store.get::<ChatHub>().unwrap();
        let mut events = hub.subscribe();

        let message = post(&request, &ctx, &alice, group.id, "Launch is Friday").await;
        post(&request, &ctx, &alice, general.id, "Morning all").await;
        conversation_members::Model::mark_read(
            &ctx.db,
            general.organisation_id,
            general.id,
            dave_id,
        )
        .await
        .unwrap();
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
        assert!(!receipts[0].dm, "every member's socket takes it");
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
        // Carol is in General too and has not read it; dave is pending.
        let receipt = receipts[0].receipt.as_ref().unwrap();
        assert_eq!(receipt.text, "Read by bob");
        assert!(receipt.readers.iter().all(|p| p.id != dave_id));

        // A DM's receipt is marked so only its author's socket takes it.
        let dm = conversations::Model::start_dm(
            &ctx.db,
            acme_id(&ctx).await,
            alice_id,
            user_id(&ctx, "bob@example.com").await,
        )
        .await
        .unwrap();
        post(&request, &ctx, &alice, dm.id, "Got a minute?").await;
        while events.try_recv().is_ok() {}
        open(&request, &bob, dm.id).await;
        let ChatEvent::Read { receipts, .. } = events
            .try_recv()
            .expect("bob's read of the DM is published")
        else {
            panic!("expected a read event");
        };
        assert!(receipts[0].dm);
        assert_eq!(receipts[0].author_id, alice_id);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn people_approved_later_count_once_they_read_older_messages() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let alice_id = user_id(&ctx, "alice@example.com").await;
        let message = post(&request, &ctx, &alice, general.id, "Morning all").await;
        open(&request, &bob, general.id).await;

        let carol = join(&request, &ctx, "acme", "carol", "carol@example.com").await;
        approve(&ctx, "carol@example.com").await;
        let page = open(&request, &alice, general.id).await;
        let slot = receipt_slot(&page, message.id);
        assert!(slot.contains(r#"aria-label="Read by bob""#), "{slot}");

        // Carol's first open covers the older message too, and tells alice.
        let hub = ctx.shared_store.get::<ChatHub>().unwrap();
        let mut events = hub.subscribe();
        open(&request, &carol, general.id).await;
        let ChatEvent::Read {
            conversation_id,
            receipts,
        } = events.try_recv().expect("carol's first read is published")
        else {
            panic!("expected a read event");
        };
        assert_eq!(conversation_id, general.id);
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].message_id, message.id);
        assert_eq!(receipts[0].author_id, alice_id);
        assert_eq!(
            receipts[0].receipt.as_ref().unwrap().text,
            "Read by bob, carol"
        );

        let page = open(&request, &alice, general.id).await;
        let slot = receipt_slot(&page, message.id);
        assert!(
            slot.contains(r#"aria-label="Read by bob, carol""#),
            "{slot}"
        );
        assert_eq!(slot.matches("avatar avatar-2xs").count(), 2, "{slot}");
    })
    .await;
}

/// Real sockets: in General a member who did not write the message gets the
/// faces, with no tick; in a DM only the sender's socket gets "Read".
#[tokio::test]
#[serial]
async fn read_receipts_reach_the_right_sockets() {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

    let (ctx, addr, cookie, alice, mut general_socket) =
        super::live_sign_out::open_general_socket().await;
    let joined = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .post(format!("http://{addr}/join/acme"))
        .form(&[
            ("name", "bob"),
            ("email", "bob@example.com"),
            ("password", super::prepare_data::USER_PASSWORD),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(joined.status(), 303);
    let bob = approve(&ctx, "bob@example.com").await;
    let org_id = acme_id(&ctx).await;
    let general = general(&ctx).await;
    let say = |conversation: conversations::Model, user_id: i64, body: &'static str| {
        let db = ctx.db.clone();
        async move {
            messages::Model::create(
                &db,
                &conversation,
                user_id,
                &messages::MessageParams {
                    body: body.to_string(),
                },
            )
            .await
            .unwrap()
        }
    };

    // Bob's message in General, read by alice: her socket shows her face on it.
    let bobs = say(general.clone(), bob.id, "Morning all").await;
    collab::controllers::chat_ws::mark_read(&ctx, org_id, general.id, &general.kind, alice.id)
        .await
        .unwrap();
    let next = tokio::time::timeout(super::live_sign_out::WAIT, general_socket.next())
        .await
        .unwrap();
    let Some(Ok(Message::Text(html))) = next else {
        panic!("expected the receipt, got {next:?}");
    };
    assert!(
        html.contains(&format!(r#"id="receipt-{}" hx-swap-oob="true""#, bobs.id)),
        "{html}"
    );
    assert!(html.contains(r#"aria-label="Read by alice""#), "{html}");
    assert!(!html.contains("tick-"), "alice did not write it: {html}");

    // A DM socket for alice.
    let dm = conversations::Model::start_dm(&ctx.db, org_id, alice.id, bob.id)
        .await
        .unwrap();
    let mut request = format!("ws://{addr}/chat/{}/ws", dm.id)
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    let (mut dm_socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();

    // Bob's DM, read by alice: not for her socket. Then hers, read by bob: it is.
    let from_bob = say(dm.clone(), bob.id, "Got a minute?").await;
    collab::controllers::chat_ws::mark_read(&ctx, org_id, dm.id, &dm.kind, alice.id)
        .await
        .unwrap();
    let from_alice = say(dm.clone(), alice.id, "Sure").await;
    collab::controllers::chat_ws::mark_read(&ctx, org_id, dm.id, &dm.kind, bob.id)
        .await
        .unwrap();
    let next = tokio::time::timeout(super::live_sign_out::WAIT, dm_socket.next())
        .await
        .unwrap();
    let Some(Ok(Message::Text(html))) = next else {
        panic!("expected the receipt, got {next:?}");
    };
    assert!(
        !html.contains(&format!("receipt-{}", from_bob.id)),
        "bob's DM is not alice's to see: {html}"
    );
    assert!(
        html.contains(&format!(
            r#"id="receipt-{}" hx-swap-oob="true""#,
            from_alice.id
        )),
        "{html}"
    );
    assert!(
        html.contains("msg-receipt-dm") && html.contains("Read</span>"),
        "{html}"
    );
    assert!(
        html.contains(&format!(
            r#"id="tick-{}" hx-swap-oob="true""#,
            from_alice.id
        )),
        "{html}"
    );
}
