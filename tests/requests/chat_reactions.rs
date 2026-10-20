//! Emoji reactions on chat messages: setting them, the fixed six, who can
//! react or see who reacted, live updates and how the feed shows them.
use collab::{
    app::App,
    data::chat_hub::{ChatEvent, ChatHub},
    models::{
        conversations::{self, GroupParams},
        messages, notifications, reactions,
    },
};
use futures_util::StreamExt;
use loco_rs::{app::AppContext, testing::prelude::*, TestServer};
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
use serial_test::serial;
use tokio_tungstenite::tungstenite::Message;

use super::{
    chat::{acme_id, acme_with_bob, approve, general, post, user_id, TestMember},
    prepare_data::{join, sign_up_member, AsMember},
};

fn react_path(conversation_id: i64, message_id: i64) -> String {
    format!("/chat/{conversation_id}/messages/{message_id}/reactions")
}

fn actions_path(conversation_id: i64, message_id: i64) -> String {
    format!("/chat/{conversation_id}/messages/{message_id}/actions")
}

/// Sets `who`'s reaction through the route, as the page does.
async fn react(
    request: &TestServer,
    who: &TestMember,
    message: &messages::Model,
    emoji: &str,
    on: bool,
) -> axum_test::TestResponse {
    request
        .post(&who.at(&react_path(message.conversation_id, message.id)))
        .as_member(who)
        .add_header("HX-Request", "true")
        .form(&serde_json::json!({ "emoji": emoji, "on": if on { "1" } else { "0" } }))
        .await
}

async fn count(ctx: &AppContext) -> u64 {
    reactions::Entity::find().count(&ctx.db).await.unwrap()
}

/// The chips slot for one message as rendered. It holds no `div`s, so it ends
/// at the first `</div>`.
fn reacts_slot(html: &str, message_id: i64) -> &str {
    let start = html
        .find(&format!(r#"id="reacts-{message_id}""#))
        .expect("the message has a reactions slot");
    let end = start + html[start..].find("</div>").unwrap();
    &html[start..end]
}

#[tokio::test]
#[serial]
async fn setting_a_reaction_is_safe_to_repeat() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let message = post(&request, &ctx, &bob, general.id, "Price list is in").await;

        for _ in 0..2 {
            let res = react(&request, &alice, &message, "👍", true).await;
            assert_eq!(res.status_code(), 200);
            let slot = reacts_slot(&res.text(), message.id).to_string();
            assert!(slot.contains(r#"class="react mine""#), "{slot}");
            assert!(
                slot.contains(r#"<span class="react-count" aria-hidden="true">1</span>"#),
                "{slot}"
            );
            assert!(slot.contains(r#"aria-label="👍 1, you reacted""#), "{slot}");
            assert_eq!(count(&ctx).await, 1);
        }
        for _ in 0..2 {
            let res = react(&request, &alice, &message, "👍", false).await;
            assert_eq!(res.status_code(), 200);
            let html = res.text();
            assert!(
                html.contains(&format!(
                    r#"<div class="reacts" id="reacts-{}" role="group" aria-label="Reactions" aria-live="off" hx-swap-oob="true"></div>"#,
                    message.id
                )),
                "{html}"
            );
            assert_eq!(count(&ctx).await, 0);
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn people_share_a_chip_and_each_sees_their_own() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let message = post(&request, &ctx, &bob, general.id, "Keys on the 24th").await;

        react(&request, &bob, &message, "👍", true).await;
        react(&request, &alice, &message, "✅", true).await;
        let res = react(&request, &alice, &message, "👍", true).await;
        assert_eq!(count(&ctx).await, 3);
        let mine = reacts_slot(&res.text(), message.id).to_string();
        assert!(mine.contains(r#"aria-label="👍 2, you reacted""#), "{mine}");
        assert!(mine.contains(r#"aria-label="✅ 1, you reacted""#), "{mine}");
        // The toolbar marks both as alice's.
        assert!(res.text().contains(r#"title="Remove your 👍""#));
        assert!(res.text().contains(r#"title="React ❤️""#));

        let feed = request
            .get(&bob.at(&format!("/chat/{}/feed", general.id)))
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        let bobs = reacts_slot(&feed, message.id);
        assert!(bobs.contains(r#"aria-label="👍 2, you reacted""#), "{bobs}");
        assert!(bobs.contains(r#"aria-label="✅ 1""#), "{bobs}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn only_the_six_reactions_are_accepted() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let message = post(&request, &ctx, &bob, general.id, "Pick one").await;
        let path = alice.at(&react_path(general.id, message.id));

        let refused = [
            serde_json::json!({ "emoji": "😀", "on": "1" }),
            serde_json::json!({ "emoji": "\u{2764}", "on": "1" }),
            serde_json::json!({ "emoji": "👍👍", "on": "1" }),
            serde_json::json!({ "emoji": "", "on": "1" }),
            serde_json::json!({ "emoji": "<b>x</b>", "on": "1" }),
            serde_json::json!({ "on": "1" }),
            serde_json::json!({ "emoji": "👍", "on": "yes" }),
            serde_json::json!({ "emoji": "👍" }),
        ];
        for form in &refused {
            let res = request
                .post(&alice.to(&path))
                .as_member(&alice)
                .add_header("HX-Request", "true")
                .form(form)
                .await;
            assert_eq!(res.status_code(), 422, "{form}");
            assert_eq!(res.headers()["HX-Reswap"], "none", "{form}");
            assert!(res.headers().contains_key("HX-Trigger"), "{form}");
            assert!(!res.text().contains("<b>"), "{form}");
        }
        assert_eq!(count(&ctx).await, 0);

        for emoji in reactions::EMOJI {
            assert_eq!(
                react(&request, &alice, &message, emoji, true)
                    .await
                    .status_code(),
                200,
                "{emoji}"
            );
        }
        assert_eq!(count(&ctx).await, 6);
        let stored: Vec<String> = reactions::Entity::find()
            .all(&ctx.db)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.emoji)
            .collect();
        assert!(
            stored.contains(&"\u{2764}\u{fe0f}".to_string()),
            "❤️ keeps U+FE0F"
        );
    })
    .await;
}

/// Every reactions route answers only to approved members of the conversation,
/// and only for messages in it.
#[tokio::test]
#[serial]
async fn reactions_need_a_member_of_the_conversation() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let in_general = post(&request, &ctx, &bob, general.id, "Morning all").await;
        // A group of alice and carol, without bob.
        join(&request, &ctx, "acme", "carol", "carol@example.com").await;
        let carol_id = approve(&ctx, "carol@example.com").await.id;
        let group = conversations::Model::create_group(
            &ctx.db,
            acme_id(&ctx).await,
            user_id(&ctx, "alice@example.com").await,
            &GroupParams {
                name: "payroll".to_string(),
                member_ids: vec![carol_id],
            },
        )
        .await
        .unwrap();
        let in_group = post(&request, &ctx, &alice, group.id, "Salaries are in").await;
        // Dave has joined but is not approved yet.
        let dave = join(&request, &ctx, "acme", "dave", "dave@example.com").await;
        let gina = sign_up_member(&request, "Globex", "gina", "gina@example.com").await;

        let cases = [
            (
                "another organisation",
                &gina,
                general.id,
                in_general.id,
                404,
            ),
            ("pending", &dave, general.id, in_general.id, 403),
            ("not in the group", &bob, group.id, in_group.id, 404),
            (
                "message from another conversation",
                &alice,
                general.id,
                in_group.id,
                404,
            ),
        ];
        for (case, who, conversation_id, message_id, status) in cases {
            let res = request
                .post(&who.at(&react_path(conversation_id, message_id)))
                .as_member(who)
                .form(&serde_json::json!({ "emoji": "👍", "on": "1" }))
                .await;
            // A call from a tab with no active org left is sent Home instead.
            let posted = if case == "pending" { 409 } else { status };
            assert_eq!(res.status_code(), posted, "POST reactions: {case}");
            assert!(!res.text().contains("Salaries"), "POST reactions: {case}");
            let res = request
                .get(&who.at(&actions_path(conversation_id, message_id)))
                .add_header(who.0.clone(), who.1.clone())
                .await;
            assert_eq!(res.status_code(), status, "GET actions: {case}");
            assert!(!res.text().contains("Salaries"), "GET actions: {case}");
            assert!(!res.text().contains("Morning all"), "GET actions: {case}");
        }
        assert_eq!(count(&ctx).await, 0);

        // The same routes work for members.
        assert_eq!(
            react(&request, &alice, &in_group, "🎉", true)
                .await
                .status_code(),
            200
        );
        let res = request
            .get(&alice.at(&actions_path(group.id, in_group.id)))
            .add_header(alice.0.clone(), alice.1.clone())
            .await;
        assert_eq!(res.status_code(), 200);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn reactions_go_live_and_notify_nobody() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let message = post(&request, &ctx, &bob, general.id, "Signage proof is in").await;
        let notices = notifications::Entity::find().count(&ctx.db).await.unwrap();
        let mut events = ctx.shared_store.get::<ChatHub>().unwrap().subscribe();

        react(&request, &alice, &message, "👀", true).await;
        let ChatEvent::Reactions {
            conversation_id,
            message_id,
            rows,
            ..
        } = events.try_recv().expect("the reaction is broadcast")
        else {
            panic!("expected a reactions event");
        };
        assert_eq!((conversation_id, message_id), (general.id, message.id));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].emoji, "👀");
        assert_eq!(rows[0].username, "alice");
        assert!(events.try_recv().is_err(), "one event per change");

        // Removing it sends the now empty set.
        react(&request, &alice, &message, "👀", false).await;
        let Ok(ChatEvent::Reactions { rows, .. }) = events.try_recv() else {
            panic!("expected a reactions event");
        };
        assert!(rows.is_empty());

        assert_eq!(
            notifications::Entity::find().count(&ctx.db).await.unwrap(),
            notices,
            "reactions send no notification"
        );
    })
    .await;
}

/// A real socket gets the chips for its own conversation and nothing from
/// any other, even though the hub carries both.
#[tokio::test]
#[serial]
async fn a_socket_only_gets_reactions_for_its_conversation() {
    let (ctx, addr, cookie, alice, mut socket) = super::live_sign_out::open_general_socket().await;
    let hello = messages::Entity::find()
        .filter(messages::Column::Body.eq("hello"))
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    let hub = ctx.shared_store.get::<ChatHub>().unwrap();
    hub.publish(ChatEvent::Reactions {
        conversation_id: -1,
        message_id: 987_654,
        parent_id: None,
        rows: std::sync::Arc::new(vec![collab::controllers::chat::ReactionRow {
            emoji: "👍".to_string(),
            user_id: alice.id,
            username: "OTHER-ORG".to_string(),
        }]),
    });
    let res = reqwest::Client::new()
        .post(format!(
            "http://{addr}/o/acme{}",
            react_path(hello.conversation_id, hello.id)
        ))
        .header("cookie", &cookie)
        .header("x-org-id", hello.organisation_id.to_string())
        .form(&[("emoji", "🎉"), ("on", "1")])
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    let next = tokio::time::timeout(super::live_sign_out::WAIT, socket.next())
        .await
        .unwrap();
    let Some(Ok(Message::Text(html))) = next else {
        panic!("expected the chips, got {next:?}");
    };
    assert!(
        !html.contains("987654") && !html.contains("OTHER-ORG"),
        "{html}"
    );
    assert!(
        html.contains(&format!(
            r#"id="reacts-{}" role="group" aria-label="Reactions" aria-live="off" hx-swap-oob="true""#,
            hello.id
        )),
        "{html}"
    );
    assert!(html.contains(r#"aria-label="🎉 1, you reacted""#), "{html}");
    assert!(
        html.contains(&format!(r#"id="tools-{}""#, hello.id)),
        "{html}"
    );
    // The broadcast carries the socket's org in its links.
    assert!(html.contains(r#"data-url="/o/acme/chat/"#), "{html}");
    assert!(!html.contains(r#"="/chat/"#), "{html}");
}

#[tokio::test]
#[serial]
async fn the_feed_shows_chips_in_the_fixed_order() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let message = post(&request, &ctx, &bob, general.id, "Opening is on").await;
        for emoji in ["✅", "😂", "👍"] {
            react(&request, &alice, &message, emoji, true).await;
        }
        let page = request
            .get(&bob.at(&format!("/chat/{}", general.id)))
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        let feed = request
            .get(&bob.at(&format!("/chat/{}/feed", general.id)))
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        for html in [&page, &feed] {
            let slot = reacts_slot(html, message.id);
            let at = |emoji: &str| slot.find(&format!(r#"data-emoji="{emoji}""#)).unwrap();
            assert!(at("👍") < at("😂") && at("😂") < at("✅"), "{slot}");
            assert!(!slot.contains("mine"), "bob reacted with none of them");
            assert!(
                slot.contains(r#"<span>alice</span>"#),
                "who reacted: {slot}"
            );
        }
        assert!(page.contains(&format!(
            r#"data-actions-url="/o/acme/chat/{}/messages/{}/actions""#,
            general.id, message.id
        )));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_sheet_lists_you_first_then_others_in_order() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        join(&request, &ctx, "acme", "carol", "carol@example.com").await;
        approve(&ctx, "carol@example.com").await;
        let carol = super::prepare_data::login(
            &request,
            "carol@example.com",
            super::prepare_data::USER_PASSWORD,
        )
        .await;
        let carol = TestMember::new(carol, acme_id(&ctx).await, "acme");
        let message = post(&request, &ctx, &bob, general.id, "VAT is due 7 November").await;
        react(&request, &carol, &message, "👍", true).await;
        react(&request, &bob, &message, "👍", true).await;
        react(&request, &alice, &message, "👍", true).await;
        react(&request, &carol, &message, "✅", true).await;

        let sheet = request
            .get(&alice.at(&actions_path(general.id, message.id)))
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(sheet.contains("VAT is due 7 November"));
        assert!(sheet.contains("Reactions · 4"), "{sheet}");
        assert!(
            sheet.contains(r#"<span class="names truncate-1">You, carol, bob</span>"#),
            "{sheet}"
        );
        assert!(
            sheet.contains(r#"<span class="names truncate-1">carol</span>"#),
            "{sheet}"
        );
        assert!(
            sheet.contains(r#"aria-pressed="true" aria-label="Remove your 👍""#),
            "{sheet}"
        );
        assert!(
            sheet.contains(r#"aria-pressed="false" aria-label="React ✅""#),
            "{sheet}"
        );
        assert!(sheet.contains("Reply in thread"), "a top-level message");

        // Nobody has reacted: no "Reactions" list.
        let quiet = post(&request, &ctx, &alice, general.id, "Quiet one").await;
        let sheet = request
            .get(&alice.at(&actions_path(general.id, quiet.id)))
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(sheet.contains("<span>You</span>"), "your own message");
        assert!(!sheet.contains("who-row"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn emoji_in_a_message_round_trip() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        // The family is three people joined by zero-width joiners.
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        for body in ["Done ✅🎉", family] {
            let saved = post(&request, &ctx, &alice, general.id, body).await;
            assert_eq!(saved.body.as_bytes(), body.as_bytes());
        }
        let page = request
            .get(&bob.at(&format!("/chat/{}", general.id)))
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(page.contains("Done ✅🎉"));
        assert!(page.contains(family));

        // The limit counts characters, not bytes.
        for (body, status) in [("✅".repeat(1000), 200), ("✅".repeat(1001), 422)] {
            let res = request
                .post(&alice.at(&format!("/chat/{}/messages", general.id)))
                .as_member(&alice)
                .add_header("HX-Request", "true")
                .form(&serde_json::json!({ "body": body }))
                .await;
            assert_eq!(res.status_code(), status);
        }
    })
    .await;
}
