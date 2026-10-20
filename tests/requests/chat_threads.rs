//! Threads: replies stay out of the main feed, the thread page and its feed,
//! replying, who can open or reply in a thread, and live updates.
use collab::{
    app::App,
    data::chat_hub::{ChatEvent, ChatHub},
    models::{
        conversation_members,
        conversations::{self, GroupParams},
        messages, reactions, users,
    },
};
use futures_util::{SinkExt, StreamExt};
use loco_rs::{app::AppContext, testing::prelude::*, TestServer};
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
use serial_test::serial;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

use super::{
    chat::{acme_id, acme_with_bob, approve, general, post, user_id, TestMember},
    prepare_data::{join, sign_up_member, AsMember},
};

const MISSING: &str = "This thread isn't available";

fn thread_path(conversation_id: i64, parent_id: i64) -> String {
    format!("/chat/{conversation_id}/threads/{parent_id}")
}

/// Replies in `parent_id`'s thread through the plain HTTP route.
pub(super) async fn reply(
    request: &TestServer,
    who: &TestMember,
    conversation_id: i64,
    parent_id: i64,
    body: &str,
) -> axum_test::TestResponse {
    request
        .post(&format!(
            "{}/replies",
            thread_path(conversation_id, parent_id)
        ))
        .as_member(who)
        .form(&serde_json::json!({ "body": body }))
        .await
}

async fn find(ctx: &AppContext, body: &str) -> messages::Model {
    messages::Entity::find()
        .filter(messages::Column::Body.eq(body))
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap()
}

async fn get(request: &TestServer, who: &TestMember, path: &str) -> axum_test::TestResponse {
    request
        .get(path)
        .add_header(who.0.clone(), who.1.clone())
        .await
}

async fn count(ctx: &AppContext) -> u64 {
    messages::Entity::find().count(&ctx.db).await.unwrap()
}

#[tokio::test]
#[serial]
async fn replies_stay_out_of_the_main_feed() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let parent = post(
            &request,
            &ctx,
            &alice,
            general.id,
            "Supplier sent the price list",
        )
        .await;
        let res = reply(&request, &bob, general.id, parent.id, "Bowls are up 8%").await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(
            res.headers()["location"],
            thread_path(general.id, parent.id).as_str()
        );
        let saved = find(&ctx, "Bowls are up 8%").await;
        assert_eq!(saved.parent_id, Some(parent.id));

        let page = get(&request, &alice, &format!("/chat/{}", general.id))
            .await
            .text();
        let feed = get(&request, &alice, &format!("/chat/{}/feed", general.id))
            .await
            .text();
        for html in [&page, &feed] {
            assert!(html.contains("Supplier sent the price list"));
            assert!(!html.contains("Bowls are up 8%"), "{html}");
            assert!(
                !html.contains(&format!(r#"id="message-{}""#, saved.id)),
                "{html}"
            );
            assert!(
                html.contains(&format!(r#"id="thread-{}""#, parent.id)),
                "{html}"
            );
            assert!(html.contains("<b>1 reply</b>"), "{html}");
            assert!(
                html.contains(&format!(
                    r#"href="/chat/{}/threads/{}""#,
                    general.id, parent.id
                )),
                "{html}"
            );
        }
        // Reply in thread on the toolbar of a main-feed message.
        assert!(page.contains(&format!(
            r#"href="/chat/{}/threads/{}?reply=1""#,
            general.id, parent.id
        )));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_chat_list_preview_never_shows_a_reply() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let parent = post(&request, &ctx, &alice, general.id, "Keys on the 24th").await;
        reply(&request, &bob, general.id, parent.id, "SECRET-REPLY").await;
        let list = get(&request, &alice, "/chat").await.text();
        assert!(list.contains("You: Keys on the 24th"), "{list}");
        assert!(!list.contains("SECRET-REPLY"), "{list}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn replies_do_not_count_as_unread() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let parent = post(&request, &ctx, &alice, general.id, "Opening is on").await;
        // Alice reads General, then bob replies.
        get(&request, &alice, &format!("/chat/{}", general.id)).await;
        reply(&request, &bob, general.id, parent.id, "Brilliant").await;

        assert_eq!(get(&request, &alice, "/chat/unread").await.text(), "");
        let counts = conversation_members::Model::unread_counts(
            &ctx.db,
            acme_id(&ctx).await,
            user_id(&ctx, "alice@example.com").await,
        )
        .await
        .unwrap();
        assert!(!counts.contains_key(&general.id), "{counts:?}");

        // Opening the thread does not mark General read: bob's own unread
        // message stays unread for him until he opens General.
        post(&request, &ctx, &alice, general.id, "Second message").await;
        get(&request, &bob, &thread_path(general.id, parent.id)).await;
        let badge = get(&request, &bob, "/chat/unread").await.text();
        assert!(!badge.is_empty(), "the thread page leaves General unread");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_thread_page_shows_the_parent_and_replies_in_order() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let parent = post(&request, &ctx, &bob, general.id, "VAT return is due 7 November").await;

        // No replies yet: the empty state names who will be told.
        let empty = get(&request, &alice, &thread_path(general.id, parent.id)).await;
        assert_eq!(empty.status_code(), 200);
        let empty = empty.text();
        assert!(empty.contains("No replies yet"), "{empty}");
        assert!(empty.contains("bob will get a notice."), "{empty}");
        assert!(!empty.contains("autofocus"), "{empty}");
        assert!(
            get(&request, &alice, &format!("{}?reply=1", thread_path(general.id, parent.id)))
                .await
                .text()
                .contains("autofocus")
        );

        reply(&request, &alice, general.id, parent.id, "Is the fitters' invoice in?").await;
        reply(&request, &bob, general.id, parent.id, "Not yet").await;
        let page = get(&request, &alice, &thread_path(general.id, parent.id))
            .await
            .text();
        let at = |text: &str| page.find(text).unwrap_or_else(|| panic!("{text}: {page}"));
        assert!(at("VAT return is due") < at(">2 replies<"));
        assert!(at(">2 replies<") < at("fitters&#39; invoice"));
        assert!(at("fitters&#39; invoice") < at("Not yet"));
        assert!(page.contains(&format!(
            r#"action="/chat/{}/threads/{}/replies""#,
            general.id, parent.id
        )));
        assert!(page.contains(&format!(
            r#"ws-connect="/chat/{}/ws?thread={}""#,
            general.id, parent.id
        )));
        assert!(page.contains(&format!(
            r#"data-chat-feed-url="/chat/{}/threads/{}/feed""#,
            general.id, parent.id
        )));
        assert!(page.contains(&format!(
            r#"href="/chat/{}#message-{}" class="icon-btn" data-thread-back aria-label="Back to #general""#,
            general.id, parent.id
        )));
        assert!(page.contains("# general"));
        // No receipts in a thread, even on alice's own reply, no reply line and
        // no "Reply in thread".
        assert!(!page.contains("receipt-"), "{page}");
        assert!(!page.contains("thread-foot"), "{page}");
        assert!(!page.contains("Reply in thread</a>"), "{page}");
        // Sheets opened here leave out Reply in thread.
        assert!(page.contains(&format!(
            r#"data-actions-url="/chat/{}/messages/{}/actions?thread=1""#,
            general.id, parent.id
        )));

        // The feed route is the same list, for reconnects.
        let feed = get(
            &request,
            &alice,
            &format!("{}/feed", thread_path(general.id, parent.id)),
        )
        .await;
        assert_eq!(feed.status_code(), 200);
        let feed = feed.text();
        assert!(feed.contains("VAT return is due") && feed.contains("Not yet"));
        assert!(feed.contains(r#"id="thread-count""#));
        assert!(!feed.contains("receipt-"), "{feed}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_sheet_offers_reply_in_thread_on_top_level_messages_only() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let parent = post(&request, &ctx, &bob, general.id, "Signage proof is in").await;
        let actions =
            |id: i64, query: &str| format!("/chat/{}/messages/{id}/actions{query}", general.id);
        let sheet = get(&request, &alice, &actions(parent.id, "")).await.text();
        assert!(sheet.contains(&format!(
            r#"href="/chat/{}/threads/{}?reply=1""#,
            general.id, parent.id
        )));
        assert!(!sheet.contains("1 reply"));

        reply(&request, &alice, general.id, parent.id, "Looks right").await;
        let sheet = get(&request, &alice, &actions(parent.id, "")).await.text();
        assert!(
            sheet.contains(r#"<span class="meta">1 reply</span>"#),
            "{sheet}"
        );
        // On the thread page, and on a reply, there is no thread to start.
        let in_thread = get(&request, &alice, &actions(parent.id, "?thread=1"))
            .await
            .text();
        assert!(!in_thread.contains("Reply in thread"), "{in_thread}");
        let child = find(&ctx, "Looks right").await;
        let on_reply = get(&request, &alice, &actions(child.id, "")).await;
        assert_eq!(on_reply.status_code(), 200);
        assert!(!on_reply.text().contains("Reply in thread"));
    })
    .await;
}

/// Every thread route answers only to approved members of the conversation,
/// and only for a top-level message in it.
#[tokio::test]
#[serial]
async fn threads_need_a_member_and_a_top_level_message_in_the_conversation() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let in_general = post(&request, &ctx, &bob, general.id, "Morning all").await;
        reply(&request, &alice, general.id, in_general.id, "Morning").await;
        let a_reply = find(&ctx, "Morning").await;
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
        let before = count(&ctx).await;

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
                "parent from another conversation",
                &alice,
                general.id,
                in_group.id,
                404,
            ),
            ("a reply as the parent", &alice, general.id, a_reply.id, 404),
            ("no such message", &alice, general.id, 987_654, 404),
        ];
        for (case, who, conversation_id, parent_id, status) in cases {
            let path = thread_path(conversation_id, parent_id);
            for page in [path.clone(), format!("{path}/feed")] {
                let res = get(&request, who, &page).await;
                assert_eq!(res.status_code(), status, "GET {page}: {case}");
                // Text the viewer must not see. Alice's own chat list shows
                // both previews, and bob's shows General's.
                let hidden: &[&str] = match case {
                    "another organisation" | "pending" => &["Salaries", "Morning all"],
                    "not in the group" => &["Salaries"],
                    _ => &[],
                };
                let body = res.text();
                for text in hidden {
                    assert!(!body.contains(text), "GET {page}: {case}");
                }
            }
            let res = reply(&request, who, conversation_id, parent_id, "let me in").await;
            // A call from a tab with no active org left is sent Home instead.
            let posted = if case == "pending" { 409 } else { status };
            assert_eq!(res.status_code(), posted, "POST reply: {case}");
            let res = request
                .get(&format!("/chat/{conversation_id}/ws?thread={parent_id}"))
                .add_header(who.0.clone(), who.1.clone())
                .add_header("Origin", "http://localhost:5150")
                .await;
            assert_eq!(res.status_code(), status, "socket: {case}");
        }
        assert_eq!(count(&ctx).await, before, "nothing was saved");

        // The same routes work for members. Without upgrade headers the
        // socket is just not a WebSocket, after every check has passed.
        assert_eq!(
            get(&request, &alice, &thread_path(group.id, in_group.id))
                .await
                .status_code(),
            200
        );
        let socket = request
            .get(&format!("/chat/{}/ws?thread={}", group.id, in_group.id))
            .add_header(alice.0.clone(), alice.1.clone())
            .add_header("Origin", "http://localhost:5150")
            .await;
        assert_eq!(socket.status_code(), 400);
    })
    .await;
}

/// A missing thread and one the viewer cannot see look exactly the same.
#[tokio::test]
#[serial]
async fn a_missing_thread_and_a_private_one_get_the_same_page() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let group = conversations::Model::create_group(
            &ctx.db,
            acme_id(&ctx).await,
            user_id(&ctx, "alice@example.com").await,
            &GroupParams {
                name: "payroll".to_string(),
                member_ids: vec![],
            },
        )
        .await
        .unwrap();
        let secret = post(&request, &ctx, &alice, group.id, "Salaries are in").await;

        let private = get(&request, &bob, &thread_path(group.id, secret.id)).await;
        let missing = get(&request, &bob, &thread_path(group.id, 987_654)).await;
        let gone = get(&request, &bob, &thread_path(987_654, 987_654)).await;
        for res in [&private, &missing, &gone] {
            assert_eq!(res.status_code(), 404);
            assert!(res.text().contains(MISSING), "{}", res.text());
            assert!(res
                .text()
                .contains(r#"href="/chat" class="btn btn-secondary">Go to Chat"#));
            assert!(!res.text().contains("Salaries"));
            assert!(!res.text().contains("composer"));
        }
        assert_eq!(private.text(), missing.text());
        assert_eq!(private.text(), gone.text());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn threads_are_one_level_deep() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let parent = post(&request, &ctx, &alice, general.id, "Top").await;
        reply(&request, &bob, general.id, parent.id, "First reply").await;
        let child = find(&ctx, "First reply").await;

        // The model refuses a reply as a parent, whatever the route does.
        let nested = messages::Model::create_reply(
            &ctx.db,
            &general,
            &child,
            child.user_id,
            &messages::MessageParams {
                body: "nested".to_string(),
            },
        )
        .await;
        assert!(nested.is_err());
        let res = reply(&request, &alice, general.id, child.id, "nested").await;
        assert_eq!(res.status_code(), 404);
        assert_eq!(
            messages::Entity::find()
                .filter(messages::Column::Body.eq("nested"))
                .count(&ctx.db)
                .await
                .unwrap(),
            0
        );

        // Nor can a parent from another conversation be used, even by the model.
        let group = conversations::Model::create_group(
            &ctx.db,
            acme_id(&ctx).await,
            user_id(&ctx, "alice@example.com").await,
            &GroupParams {
                name: "leads".to_string(),
                member_ids: vec![],
            },
        )
        .await
        .unwrap();
        let elsewhere = messages::Model::create_reply(
            &ctx.db,
            &group,
            &parent,
            parent.user_id,
            &messages::MessageParams {
                body: "elsewhere".to_string(),
            },
        )
        .await;
        assert!(elsewhere.is_err());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn empty_and_overlong_replies_are_rejected() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let parent = post(&request, &ctx, &alice, general.id, "Top").await;
        for body in ["   ".to_string(), "x".repeat(1001)] {
            let res = request
                .post(&format!("{}/replies", thread_path(general.id, parent.id)))
                .as_member(&alice)
                .add_header("HX-Request", "true")
                .form(&serde_json::json!({ "body": body }))
                .await;
            assert_eq!(res.status_code(), 422);
            assert_eq!(res.headers()["HX-Reswap"], "none");
            assert!(res.headers().contains_key("HX-Trigger"));
        }
        assert_eq!(count(&ctx).await, 1);

        // Emoji save byte for byte, and the limit counts characters.
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        for body in ["Done ✅🎉", family] {
            let res = request
                .post(&format!("{}/replies", thread_path(general.id, parent.id)))
                .as_member(&alice)
                .add_header("HX-Request", "true")
                .form(&serde_json::json!({ "body": body }))
                .await;
            assert_eq!(res.status_code(), 200);
            assert!(res.text().contains(body));
            // Your own reply, rendered without a receipt slot.
            assert!(res.text().contains("msg msg-own"));
            assert!(!res.text().contains("receipt-"));
            assert_eq!(find(&ctx, body).await.body.as_bytes(), body.as_bytes());
        }
        assert_eq!(
            reply(&request, &alice, general.id, parent.id, &"✅".repeat(1000))
                .await
                .status_code(),
            303
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn http_replies_publish_a_reply_event_and_no_message() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let parent = post(&request, &ctx, &alice, general.id, "Top").await;
        let mut events = ctx.shared_store.get::<ChatHub>().unwrap().subscribe();

        reply(&request, &bob, general.id, parent.id, "Live reply").await;
        let ChatEvent::Reply {
            conversation_id,
            parent_id,
            message,
            line,
            ..
        } = events.try_recv().expect("the reply is broadcast")
        else {
            panic!("expected a reply event");
        };
        assert_eq!((conversation_id, parent_id), (general.id, parent.id));
        assert_eq!(message.body, "Live reply");
        assert_eq!(message.parent_id, Some(parent.id));
        assert!(!message.can_reply);
        assert_eq!(line.label, "1 reply");
        assert_eq!(line.url, thread_path(general.id, parent.id));
        assert_eq!(line.faces[0].username, "bob");
        assert!(events.try_recv().is_err(), "no message event");

        // Reactions on a reply carry its parent, so they stay in the thread.
        let child = find(&ctx, "Live reply").await;
        request
            .post(&format!(
                "/chat/{}/messages/{}/reactions",
                general.id, child.id
            ))
            .as_member(&alice)
            .form(&serde_json::json!({ "emoji": "👍", "on": "1" }))
            .await;
        let Ok(ChatEvent::Reactions { parent_id, .. }) = events.try_recv() else {
            panic!("expected a reactions event");
        };
        assert_eq!(parent_id, Some(parent.id));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn deleting_the_parents_author_removes_the_thread() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let parent = post(&request, &ctx, &bob, general.id, "Bob's parent").await;
        reply(&request, &alice, general.id, parent.id, "Alice's reply").await;
        let child = find(&ctx, "Alice's reply").await;
        request
            .post(&format!(
                "/chat/{}/messages/{}/reactions",
                general.id, child.id
            ))
            .as_member(&alice)
            .form(&serde_json::json!({ "emoji": "👍", "on": "1" }))
            .await;
        assert_eq!(reactions::Entity::find().count(&ctx.db).await.unwrap(), 1);

        // As `user:delete` does.
        users::Entity::delete_by_id(user_id(&ctx, "bob@example.com").await)
            .exec(&ctx.db)
            .await
            .unwrap();
        assert_eq!(count(&ctx).await, 0, "the parent and alice's reply");
        assert_eq!(reactions::Entity::find().count(&ctx.db).await.unwrap(), 0);
        let res = get(&request, &alice, &thread_path(general.id, parent.id)).await;
        assert_eq!(res.status_code(), 404);
        assert!(res.text().contains(MISSING));
    })
    .await;
}

/// Real sockets: General's socket gets the parent's new reply line and not
/// the reply; the thread's socket gets the reply and the new count.
#[tokio::test]
#[serial]
async fn sockets_get_reply_lines_and_replies() {
    let (ctx, addr, cookie, alice, mut feed) = super::live_sign_out::open_general_socket().await;
    let hello = messages::Entity::find()
        .filter(messages::Column::Body.eq("hello"))
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    let mut request = format!(
        "ws://{addr}/chat/{}/ws?thread={}",
        hello.conversation_id, hello.id
    )
    .into_client_request()
    .unwrap();
    request
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    let (mut thread, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    // Another thread's reply, published straight to the hub, must not show.
    let mut other = hello.clone();
    other.id = 987_654;
    other.parent_id = Some(987_653);
    ctx.shared_store
        .get::<ChatHub>()
        .unwrap()
        .publish(ChatEvent::Reply {
            conversation_id: hello.conversation_id,
            parent_id: 987_653,
            author_id: alice.id,
            message: Box::new(collab::controllers::chat::MessageView::new(
                &other,
                &std::collections::HashMap::new(),
                0,
                chrono_tz::UTC,
            )),
            line: Box::new(collab::controllers::chat::ThreadLine {
                url: String::new(),
                count: 9,
                label: "OTHER-THREAD".to_string(),
                last: String::new(),
                faces: Vec::new(),
                more: 0,
            }),
        });

    thread
        .send(Message::text(r#"{"body":"From the thread"}"#))
        .await
        .unwrap();

    let next = tokio::time::timeout(super::live_sign_out::WAIT, thread.next())
        .await
        .unwrap();
    let Some(Ok(Message::Text(html))) = next else {
        panic!("expected the reply, got {next:?}");
    };
    assert!(!html.contains("OTHER-THREAD"), "{html}");
    assert!(html.contains("From the thread"), "{html}");
    assert!(html.contains("msg msg-own"), "{html}");
    assert!(!html.contains("receipt-"), "{html}");
    assert!(
        html.contains(r#"id="thread-count" aria-live="off" hx-swap-oob="true">1 reply<"#),
        "{html}"
    );

    // General's socket first gets the other thread's line (it follows every
    // thread in General), then hello's.
    let mut lines = Vec::new();
    for _ in 0..2 {
        let next = tokio::time::timeout(super::live_sign_out::WAIT, feed.next())
            .await
            .unwrap();
        let Some(Ok(Message::Text(html))) = next else {
            panic!("expected a reply line, got {next:?}");
        };
        lines.push(html.to_string());
    }
    let line = &lines[1];
    assert!(
        line.contains(&format!(
            r#"id="thread-{}" aria-live="off" hx-swap-oob="true""#,
            hello.id
        )),
        "{line}"
    );
    assert!(line.contains("<b>1 reply</b>"), "{line}");
    assert!(!line.contains("From the thread"), "{line}");
    let saved = messages::Entity::find()
        .filter(messages::Column::Body.eq("From the thread"))
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.parent_id,
        Some(hello.id),
        "the parent is the session's"
    );
}
