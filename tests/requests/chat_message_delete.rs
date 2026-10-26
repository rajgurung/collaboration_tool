//! Deleting a chat message. The sender may delete it while it has no
//! replies; owners and admins who are in the conversation may delete any,
//! and a thread parent takes its replies. The row goes for good, with its
//! photo, reactions and bell notices, and open pages hear about it over the
//! socket. Everyone else is refused, and nothing changes.
use std::{net::SocketAddr, path::Path, time::Duration};

use collab::{
    app::App,
    data::chat_hub::{ChatEvent, ChatHub},
    mailers::mentions::send_due,
    models::{
        conversation_members, conversations, documents, memberships, messages, notifications,
        reactions,
    },
};
use futures_util::{SinkExt, StreamExt};
use loco_rs::{app::AppContext, testing::prelude::*, TestServer};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter, Statement};
use serial_test::serial;
use tokio::sync::broadcast::Receiver;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

use crate::locks::{hold, waiting_locks};

use super::{
    chat_photos::{group_of, latest_message, leaks, open_photo, page, secret_png, send_photo},
    juniper::{juniper, Team},
    prepare_data::{join, login, sign_up_member, AsMember, TestMember, USER_PASSWORD},
};

const REFUSED: &str = "Only the person who sent a message, owners and admins can delete it.";
const LEEDS: &str = "@ana I quoted Leeds £4.20 a tin by mistake. Can we talk before they pay?";
const STALL: &str = "Who can do the stall from 7 to 10 on Saturday?";
const INVOICE: &str = "Courier invoice from Tuesday, £18.40";

/// Juniper Tea Co with Leo in #finance too, so all four are there.
async fn team(request: &TestServer, ctx: &AppContext) -> Team {
    let team = juniper(request, ctx).await;
    conversation_members::Model::add(&ctx.db, &team.finance, team.leo_id)
        .await
        .unwrap();
    team
}

/// `who` sends `body` over plain HTTP, which notifies as the socket does.
async fn say(
    request: &TestServer,
    ctx: &AppContext,
    who: &TestMember,
    conversation_id: i64,
    body: &str,
) -> messages::Model {
    let res = request
        .post(&who.at(&format!("/chat/{conversation_id}/messages")))
        .as_member(who)
        .add_header("HX-Request", "true")
        .form(&serde_json::json!({ "body": body }))
        .await;
    assert_eq!(res.status_code(), 200, "{}", res.text());
    latest_message(ctx).await
}

async fn reply(
    request: &TestServer,
    ctx: &AppContext,
    who: &TestMember,
    parent: &messages::Model,
    body: &str,
) -> messages::Model {
    let res = request
        .post(&who.at(&format!(
            "/chat/{}/threads/{}/replies",
            parent.conversation_id, parent.id
        )))
        .as_member(who)
        .add_header("HX-Request", "true")
        .form(&serde_json::json!({ "body": body }))
        .await;
    assert_eq!(res.status_code(), 200, "{}", res.text());
    latest_message(ctx).await
}

/// Tom's stall question in `conversation`, with replies from Leo, Maya and Ana.
async fn stall_thread(
    request: &TestServer,
    ctx: &AppContext,
    team: &Team,
    conversation_id: i64,
) -> (messages::Model, Vec<messages::Model>) {
    let parent = say(request, ctx, &team.tom, conversation_id, STALL).await;
    let mut replies = Vec::new();
    for (who, body) in [
        (&team.leo, "I can do 7 to 8"),
        (&team.maya, "@tom me after 8"),
        (&team.ana, "I'll bring the urn"),
    ] {
        replies.push(reply(request, ctx, who, &parent, body).await);
    }
    (parent, replies)
}

fn delete_path(conversation_id: i64, message_id: i64) -> String {
    format!("/chat/{conversation_id}/messages/{message_id}/delete")
}

async fn delete(
    request: &TestServer,
    who: &TestMember,
    message: &messages::Model,
) -> axum_test::TestResponse {
    request
        .post(&who.at(&delete_path(message.conversation_id, message.id)))
        .as_member(who)
        .add_header("HX-Request", "true")
        .await
}

/// A plain form post with no org claim, as from the waiting page.
async fn plain_post(
    request: &TestServer,
    who: &TestMember,
    message: &messages::Model,
) -> axum_test::TestResponse {
    request
        .post(&who.at(&delete_path(message.conversation_id, message.id)))
        .add_header(who.cookie.0.clone(), who.cookie.1.clone())
        .await
}

async fn sheet(
    request: &TestServer,
    who: &TestMember,
    message: &messages::Model,
    query: &str,
) -> String {
    request
        .get(&who.at(&format!(
            "/chat/{}/messages/{}/actions{query}",
            message.conversation_id, message.id
        )))
        .as_member(who)
        .add_header("HX-Request", "true")
        .await
        .text()
}

async fn exists(ctx: &AppContext, id: i64) -> bool {
    messages::Entity::find_by_id(id)
        .one(&ctx.db)
        .await
        .unwrap()
        .is_some()
}

async fn find(ctx: &AppContext, id: i64) -> messages::Model {
    messages::Entity::find_by_id(id)
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap()
}

async fn notices_about(ctx: &AppContext, ids: &[i64]) -> u64 {
    notifications::Entity::find()
        .filter(notifications::Column::MessageId.is_in(ids.to_vec()))
        .count(&ctx.db)
        .await
        .unwrap()
}

async fn unread_bell(ctx: &AppContext, org_id: i64, user_id: i64) -> u64 {
    notifications::Model::unread_count(&ctx.db, org_id, user_id)
        .await
        .unwrap()
}

/// Ana's unread count for a conversation.
async fn unread_chat(ctx: &AppContext, team: &Team, conversation_id: i64) -> u64 {
    conversation_members::Model::unread_counts(&ctx.db, team.org_id, team.ana_id)
        .await
        .unwrap()
        .get(&conversation_id)
        .copied()
        .unwrap_or(0)
}

fn hub_events(ctx: &AppContext) -> Receiver<ChatEvent> {
    ctx.shared_store.get::<ChatHub>().unwrap().subscribe()
}

/// Every `MessageDeleted` waiting on `events`, as (conversation, message,
/// parent, count). Other events are skipped.
fn deletions(events: &mut Receiver<ChatEvent>) -> Vec<(i64, i64, Option<i64>, i64)> {
    let mut found = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let ChatEvent::MessageDeleted {
            conversation_id,
            message_id,
            parent_id,
            count,
            ..
        } = event
        {
            found.push((conversation_id, message_id, parent_id, count));
        }
    }
    found
}

/// Every `PhotoRemoved` waiting on `events`, as (conversation, message).
fn photo_removals(events: &mut Receiver<ChatEvent>) -> Vec<(i64, i64)> {
    let mut found = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let ChatEvent::PhotoRemoved {
            conversation_id,
            message_id,
        } = event
        {
            found.push((conversation_id, message_id));
        }
    }
    found
}

#[tokio::test]
#[serial]
async fn the_sender_deletes_a_message_for_everyone() {
    request::<App, _, _>(|request, ctx| async move {
        let team = team(&request, &ctx).await;
        let message = say(&request, &ctx, &team.tom, team.general.id, LEEDS).await;
        reactions::Model::set(&ctx.db, &message, team.leo_id, "👀", true)
            .await
            .unwrap();
        let mut events = hub_events(&ctx);

        let res = delete(&request, &team.tom, &message).await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        assert_eq!(
            deletions(&mut events),
            vec![(team.general.id, message.id, None, 0)]
        );
        assert!(!exists(&ctx, message.id).await);
        let feed = page(&request, &team.maya, &format!("/chat/{}", team.general.id)).await;
        assert!(
            !feed.contains(&format!(r#"id="message-{}""#, message.id)),
            "{feed}"
        );
        assert!(!feed.contains("Leeds"), "{feed}");
        let left = ctx
            .db
            .query_one_raw(Statement::from_string(
                ctx.db.get_database_backend(),
                "SELECT count(*) AS n FROM messages WHERE body LIKE '%Leeds £4.20%'",
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "n")
            .unwrap();
        assert_eq!(left, 0, "the text is gone from the database");
        assert_eq!(reactions::Entity::find().count(&ctx.db).await.unwrap(), 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn owners_and_admins_delete_anyones_message_and_its_replies() {
    request::<App, _, _>(|request, ctx| async move {
        let team = team(&request, &ctx).await;
        let (parent, replies) = stall_thread(&request, &ctx, &team, team.finance.id).await;
        let mut ids: Vec<i64> = replies.iter().map(|r| r.id).collect();
        ids.push(parent.id);
        assert!(
            notices_about(&ctx, &ids).await > 0,
            "the replies told people"
        );

        let res = delete(&request, &team.maya, &parent).await;
        assert_eq!(res.status_code(), 200, "the admin: {}", res.text());
        for id in &ids {
            assert!(!exists(&ctx, *id).await);
        }
        assert_eq!(notices_about(&ctx, &ids).await, 0);
        let thread = request
            .get(
                &team
                    .tom
                    .at(&format!("/chat/{}/threads/{}", team.finance.id, parent.id)),
            )
            .add_header(team.tom.cookie.0.clone(), team.tom.cookie.1.clone())
            .await;
        assert_eq!(thread.status_code(), 404);
        assert!(thread.text().contains("This thread isn't available"));

        let dm = conversations::Model::start_dm(&ctx.db, team.org_id, team.leo_id, team.ana_id)
            .await
            .unwrap();
        let meter = say(&request, &ctx, &team.leo, dm.id, "Meter reading 4471").await;
        let res = delete(&request, &team.ana, &meter).await;
        assert_eq!(res.status_code(), 200, "the owner");
        assert!(!exists(&ctx, meter.id).await);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_sender_is_refused_once_there_are_replies() {
    request::<App, _, _>(|request, ctx| async move {
        let team = team(&request, &ctx).await;
        let (parent, _) = stall_thread(&request, &ctx, &team, team.general.id).await;
        let mut events = hub_events(&ctx);

        let res = delete(&request, &team.tom, &parent).await;
        assert_eq!(res.status_code(), 403);
        assert!(res.text().contains(messages::HAS_REPLIES), "{}", res.text());
        assert_eq!(messages::Entity::find().count(&ctx.db).await.unwrap(), 4);
        assert!(deletions(&mut events).is_empty());

        let toms = sheet(&request, &team.tom, &parent, "").await;
        assert!(toms.contains(messages::HAS_REPLIES), "{toms}");
        assert!(!toms.contains("Delete message"), "{toms}");
        let mayas = sheet(&request, &team.maya, &parent, "").await;
        assert!(mayas.contains("Delete message"), "{mayas}");
        assert!(mayas.contains("Its 3 replies are deleted too."), "{mayas}");
        assert!(!mayas.contains(messages::HAS_REPLIES), "{mayas}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_member_cannot_delete_someone_elses_message() {
    request::<App, _, _>(|request, ctx| async move {
        let team = team(&request, &ctx).await;
        let message = say(&request, &ctx, &team.tom, team.finance.id, LEEDS).await;
        let mut events = hub_events(&ctx);

        let res = delete(&request, &team.leo, &message).await;
        assert_eq!(res.status_code(), 403);
        assert!(res.text().contains(REFUSED), "{}", res.text());
        assert_eq!(find(&ctx, message.id).await, message);
        assert!(deletions(&mut events).is_empty());

        let leos = sheet(&request, &team.leo, &message, "").await;
        assert!(!leos.contains("Delete message"), "{leos}");
        assert!(!leos.contains(messages::HAS_REPLIES), "{leos}");
        let toms = sheet(&request, &team.tom, &message, "").await;
        assert!(
            toms.contains(&format!(r#"data-message-delete="{}""#, message.id)),
            "{toms}"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn people_outside_the_conversation_cannot_delete() {
    request::<App, _, _>(|request, ctx| async move {
        let team = team(&request, &ctx).await;
        let suppliers = group_of(
            &ctx,
            team.org_id,
            team.tom_id,
            "suppliers",
            vec![team.leo_id],
        )
        .await;
        let message = say(&request, &ctx, &team.tom, suppliers.id, INVOICE).await;
        let finance_message = say(&request, &ctx, &team.tom, team.finance.id, "Van booked").await;
        let mut events = hub_events(&ctx);
        let mut refused = |what: &str, res: axum_test::TestResponse, status: u16| {
            assert_eq!(res.status_code(), status, "{what}: {}", res.text());
            assert!(!res.text().contains("£18.40"), "{what}");
            assert!(deletions(&mut events).is_empty(), "{what} sent an event");
        };

        // Owner and admin, not in the group: role does not matter.
        for who in [&team.ana, &team.maya] {
            refused(
                "outside the group",
                delete(&request, who, &message).await,
                404,
            );
        }
        // Crossed ids.
        let res = request
            .post(&team.tom.at(&delete_path(suppliers.id, finance_message.id)))
            .as_member(&team.tom)
            .await;
        refused("#finance's message at #suppliers", res, 404);
        let res = request
            .post(&team.tom.at(&delete_path(team.finance.id, message.id)))
            .as_member(&team.tom)
            .await;
        refused("#suppliers' message at #finance", res, 404);

        // Signed out.
        let res = request
            .post(&team.tom.at(&delete_path(suppliers.id, message.id)))
            .await;
        assert_eq!(res.headers()["location"], "/login");
        refused("signed out", res, 303);

        // Waiting to join.
        let pat = join(&request, &ctx, &team.tom.slug, "pat", "pat@example.com").await;
        refused("pending", plain_post(&request, &pat, &message).await, 403);
        refused(
            "pending, from a script",
            delete(&request, &pat, &message).await,
            409,
        );

        // A member of another organisation only, even claiming Juniper's tab.
        let other = sign_up_member(&request, "Other Tea", "mallory", "mallory@example.com").await;
        let forged = TestMember::new(other.cookie.clone(), team.org_id, team.tom.slug.clone());
        refused(
            "another org",
            plain_post(&request, &forged, &message).await,
            404,
        );
        refused(
            "another org, from a script",
            delete(&request, &forged, &message).await,
            409,
        );

        // A super admin acting in Juniper Tea Co.
        let admin = super::admin::super_admin_cookie(&request, &ctx).await;
        let acting = format!("{}; acting_org={}", admin.1.to_str().unwrap(), team.org_id);
        let res = request
            .post(&team.tom.at(&delete_path(suppliers.id, message.id)))
            .add_header("cookie", acting)
            .add_header("x-org-id", team.org_id.to_string())
            .await;
        refused("acting", res, 403);

        // A cross-site form or fetch, with Tom's cookie.
        let res = request
            .post(&team.tom.at(&delete_path(suppliers.id, message.id)))
            .as_member(&team.tom)
            .add_header("Origin", "https://evil.example")
            .await;
        refused("cross-site", res, 403);

        // Leo is removed from Juniper Tea Co.
        let leo_m = memberships::Model::default_for_user(&ctx.db, team.leo_id)
            .await
            .unwrap()
            .unwrap();
        memberships::Model::remove(
            &ctx,
            team.org_id,
            team.ana_id,
            leo_m.id,
            &Default::default(),
        )
        .await
        .unwrap();
        let removed = TestMember::new(
            login(&request, "leo@example.com", USER_PASSWORD).await,
            team.org_id,
            team.tom.slug.clone(),
        );
        refused(
            "removed",
            plain_post(&request, &removed, &message).await,
            403,
        );
        refused(
            "removed, from a script",
            delete(&request, &removed, &message).await,
            409,
        );

        // Nothing changed.
        assert_eq!(find(&ctx, message.id).await, message);
        assert_eq!(find(&ctx, finance_message.id).await, finance_message);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_photo_message_loses_its_photo_too() {
    request::<App, _, _>(|request, ctx| async move {
        let team = team(&request, &ctx).await;
        let finance = team.finance.id;
        let res = send_photo(
            &request,
            &team.leo,
            finance,
            Some(INVOICE),
            "invoice.png",
            &secret_png(),
        )
        .await;
        assert_eq!(res.status_code(), 200);
        let photo = latest_message(&ctx).await;
        let key = photo.photo_key.clone().unwrap();

        let res = delete(&request, &team.leo, &photo).await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        assert!(ctx
            .storage
            .download::<Vec<u8>>(Path::new(&key))
            .await
            .is_err());
        assert!(!exists(&ctx, photo.id).await);
        let res = open_photo(&request, &team.maya, finance, photo.id).await;
        assert_eq!(res.status_code(), 404);
        let feed = page(&request, &team.maya, &format!("/chat/{finance}")).await;
        assert!(!leaks(feed.as_bytes()));
        assert!(!feed.contains("Courier invoice"), "{feed}");

        // #general: the Documents file goes too.
        let general = team.general.id;
        let res = send_photo(
            &request,
            &team.tom,
            general,
            Some(INVOICE),
            "invoice.png",
            &secret_png(),
        )
        .await;
        assert_eq!(res.status_code(), 200);
        let photo = latest_message(&ctx).await;
        let file = documents::Model::find_in_org(&ctx.db, team.org_id, photo.document_id.unwrap())
            .await
            .unwrap();
        let res = delete(&request, &team.maya, &photo).await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        assert!(documents::Entity::find_by_id(file.id)
            .one(&ctx.db)
            .await
            .unwrap()
            .is_none());
        assert!(ctx
            .storage
            .download::<Vec<u8>>(Path::new(&file.storage_key))
            .await
            .is_err());
        assert!(!exists(&ctx, photo.id).await);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_repeat_delete_is_not_found() {
    request::<App, _, _>(|request, ctx| async move {
        let team = team(&request, &ctx).await;
        let message = say(&request, &ctx, &team.tom, team.general.id, LEEDS).await;
        assert_eq!(
            delete(&request, &team.tom, &message).await.status_code(),
            200
        );
        let mut events = hub_events(&ctx);
        assert_eq!(
            delete(&request, &team.tom, &message).await.status_code(),
            404
        );
        assert_eq!(
            delete(&request, &team.maya, &message).await.status_code(),
            404
        );
        assert!(deletions(&mut events).is_empty());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn mentions_go_with_the_message_and_are_never_emailed() {
    request::<App, _, _>(|request, ctx| async move {
        let team = team(&request, &ctx).await;
        let message = say(&request, &ctx, &team.tom, team.general.id, LEEDS).await;
        let mention = notifications::Entity::find()
            .filter(notifications::Column::UserId.eq(team.ana_id))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(mention.message_id, Some(message.id));
        assert_eq!(unread_bell(&ctx, team.org_id, team.ana_id).await, 1);

        assert_eq!(
            delete(&request, &team.tom, &message).await.status_code(),
            200
        );
        let bell = page(&request, &team.ana, "/notifications").await;
        assert!(!bell.contains("Leeds"), "{bell}");
        assert_eq!(unread_bell(&ctx, team.org_id, team.ana_id).await, 0);

        // Every mention left is old enough to email: only the live one goes.
        say(
            &request,
            &ctx,
            &team.maya,
            team.general.id,
            "@ana the stall rota is up",
        )
        .await;
        ctx.db
            .execute_unprepared(
                "UPDATE notifications SET created_at = now() - interval '20 minutes'",
            )
            .await
            .unwrap();
        assert_eq!(send_due(&ctx).await.unwrap(), 1);
        assert_eq!(send_due(&ctx).await.unwrap(), 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn reply_notices_go_with_the_reply() {
    request::<App, _, _>(|request, ctx| async move {
        let team = team(&request, &ctx).await;
        let parent = say(&request, &ctx, &team.tom, team.general.id, STALL).await;
        let answer = reply(
            &request,
            &ctx,
            &team.leo,
            &parent,
            "@maya can you take 8 to 10?",
        )
        .await;
        for who in [team.maya_id, team.tom_id] {
            assert_eq!(unread_bell(&ctx, team.org_id, who).await, 1, "user {who}");
        }
        assert_eq!(notices_about(&ctx, &[answer.id]).await, 2);

        assert_eq!(
            delete(&request, &team.leo, &answer).await.status_code(),
            200
        );
        assert_eq!(notices_about(&ctx, &[answer.id]).await, 0);
        for who in [team.maya_id, team.tom_id] {
            assert_eq!(unread_bell(&ctx, team.org_id, who).await, 0, "user {who}");
        }
        assert!(exists(&ctx, parent.id).await);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_deleted_reply_leaves_the_thread() {
    request::<App, _, _>(|request, ctx| async move {
        let team = team(&request, &ctx).await;
        let (parent, replies) = stall_thread(&request, &ctx, &team, team.general.id).await;
        let leos = &replies[0];
        let mut events = hub_events(&ctx);

        assert_eq!(delete(&request, &team.leo, leos).await.status_code(), 200);
        assert_eq!(
            deletions(&mut events),
            vec![(team.general.id, leos.id, Some(parent.id), 2)]
        );
        let thread = page(
            &request,
            &team.maya,
            &format!("/chat/{}/threads/{}", team.general.id, parent.id),
        )
        .await;
        assert!(!thread.contains("I can do 7 to 8"), "{thread}");
        assert!(thread.contains("me after 8"), "{thread}");
        assert!(thread.contains(">2 replies<"), "{thread}");
        let feed = page(&request, &team.maya, &format!("/chat/{}", team.general.id)).await;
        assert!(feed.contains("<b>2 replies</b>"), "{feed}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_list_and_home_show_the_message_before() {
    request::<App, _, _>(|request, ctx| async move {
        let team = team(&request, &ctx).await;
        let finance = team.finance.id;
        say(
            &request,
            &ctx,
            &team.maya,
            finance,
            "Van booked for Saturday",
        )
        .await;
        let last = say(&request, &ctx, &team.tom, finance, LEEDS).await;
        assert_eq!(unread_chat(&ctx, &team, finance).await, 2);
        for path in ["/chat", "/dashboard"] {
            let ana = page(&request, &team.ana, path).await;
            assert!(ana.contains("tom: @ana I quoted Leeds"), "{path}: {ana}");
        }

        assert_eq!(delete(&request, &team.tom, &last).await.status_code(), 200);
        assert_eq!(unread_chat(&ctx, &team, finance).await, 1);
        for path in ["/chat", "/dashboard"] {
            let ana = page(&request, &team.ana, path).await;
            assert!(
                ana.contains("maya: Van booked for Saturday<"),
                "{path}: {ana}"
            );
            assert!(!ana.contains("Leeds"), "{path}: {ana}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_confirm_names_what_goes() {
    request::<App, _, _>(|request, ctx| async move {
        let team = team(&request, &ctx).await;
        let text = say(&request, &ctx, &team.tom, team.general.id, LEEDS).await;
        let html = sheet(&request, &team.tom, &text, "").await;
        assert!(html.contains("<template data-message-delete-body>"), "{html}");
        assert!(
            html.contains("It's removed for everyone in #general. This can't be undone."),
            "{html}"
        );
        assert!(html.contains("Delete this message?"), "{html}");

        let res = send_photo(&request, &team.leo, team.finance.id, Some(INVOICE), "invoice.png", &secret_png()).await;
        assert_eq!(res.status_code(), 200);
        let photo = latest_message(&ctx).await;
        let html = sheet(&request, &team.leo, &photo, "").await;
        assert!(
            html.contains("It's removed for everyone in #finance. The photo is deleted too. This can't be undone."),
            "{html}"
        );
        assert!(html.contains(&format!("<p>Photo · {INVOICE}</p>")), "{html}");

        let res = send_photo(&request, &team.tom, team.general.id, None, "whiteboard.png", &secret_png()).await;
        assert_eq!(res.status_code(), 200);
        let general_photo = latest_message(&ctx).await;
        let html = sheet(&request, &team.maya, &general_photo, "").await;
        assert!(
            html.contains("It's removed for everyone in #general. The photo is deleted too. It's also deleted from Documents. This can't be undone."),
            "{html}"
        );

        let parent = say(&request, &ctx, &team.tom, team.finance.id, STALL).await;
        let only = reply(&request, &ctx, &team.leo, &parent, "I can do 7 to 8").await;
        let html = sheet(&request, &team.maya, &parent, "").await;
        assert!(html.contains("Its reply is deleted too. This can't be undone."), "{html}");

        let dm = conversations::Model::start_dm(&ctx.db, team.org_id, team.leo_id, team.ana_id)
            .await
            .unwrap();
        let meter = say(&request, &ctx, &team.leo, dm.id, "Meter reading 4471").await;
        let html = sheet(&request, &team.leo, &meter, "").await;
        assert!(html.contains("It's removed for you and ana. This can't be undone."), "{html}");

        // A reply's own sheet, on the thread page.
        let html = sheet(&request, &team.leo, &only, "?thread=1").await;
        assert!(html.contains("Delete message"), "{html}");
        assert!(!html.contains("deleted too"), "{html}");
        for page_path in [
            format!("/chat/{}", team.general.id),
            format!("/chat/{}/threads/{}", team.finance.id, parent.id),
        ] {
            let shown = page(&request, &team.tom, &page_path).await;
            assert!(shown.contains(r#"<dialog id="message-delete" class="sheet""#), "{shown}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn viewer_delete_photo_still_keeps_the_caption() {
    request::<App, _, _>(|request, ctx| async move {
        let team = team(&request, &ctx).await;
        let finance = team.finance.id;
        let res = send_photo(
            &request,
            &team.tom,
            finance,
            Some(INVOICE),
            "invoice.png",
            &secret_png(),
        )
        .await;
        assert_eq!(res.status_code(), 200);
        let photo = latest_message(&ctx).await;
        let html = sheet(&request, &team.tom, &photo, "").await;
        assert!(!html.contains("Delete photo"), "{html}");
        let feed = page(&request, &team.tom, &format!("/chat/{finance}")).await;
        assert!(feed.contains("data-delete-url"), "{feed}");

        let res = request
            .post(
                &team
                    .tom
                    .at(&format!("/chat/{finance}/photos/{}/delete", photo.id)),
            )
            .as_member(&team.tom)
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 200);
        let kept = find(&ctx, photo.id).await;
        assert!(kept.photo_removed());
        assert_eq!(kept.body, INVOICE);
    })
    .await;
}

/// The photo goes before the rest, which then times out: 500, the message
/// stays as "Photo removed" with its caption, open pages hear the photo
/// went, and a retry deletes the message.
#[tokio::test]
#[serial]
async fn a_failed_delete_answers_500_and_a_retry_works() {
    request::<App, _, _>(|request, ctx| async move {
        let team = team(&request, &ctx).await;
        let finance = team.finance.id;
        let res = send_photo(
            &request,
            &team.tom,
            finance,
            Some(INVOICE),
            "invoice.png",
            &secret_png(),
        )
        .await;
        assert_eq!(res.status_code(), 200);
        let photo = latest_message(&ctx).await;
        let (_conn, txn) = hold(&ctx, photo.id, "FOR KEY SHARE").await;
        let mut events = hub_events(&ctx);

        let res = delete(&request, &team.tom, &photo).await;
        assert_eq!(res.status_code(), 500, "{}", res.text());
        assert!(!res.text().contains("£18.40"));
        let kept = find(&ctx, photo.id).await;
        assert!(kept.photo_removed());
        assert_eq!(kept.body, INVOICE);
        assert_eq!(photo_removals(&mut events), vec![(finance, photo.id)]);

        txn.rollback().await.unwrap();
        let res = delete(&request, &team.tom, &photo).await;
        assert_eq!(res.status_code(), 200, "the retry");
        assert!(!exists(&ctx, photo.id).await);
        assert_eq!(deletions(&mut events), vec![(finance, photo.id, None, 0)]);
    })
    .await;
}

/// A reply commits after the sender's photo went but before the message
/// is locked: the delete is refused, and open pages still drop the photo.
#[tokio::test]
#[serial]
async fn a_reply_that_lands_after_the_photo_refuses_the_rest() {
    request::<App, _, _>(|request, ctx| async move {
        let team = team(&request, &ctx).await;
        let finance = team.finance.id;
        let res = send_photo(&request, &team.tom, finance, Some(INVOICE), "invoice.png", &secret_png()).await;
        assert_eq!(res.status_code(), 200);
        let photo = latest_message(&ctx).await;
        let (conn, txn) = hold(&ctx, photo.id, "FOR KEY SHARE").await;
        txn.execute_unprepared(&format!(
            "INSERT INTO messages (created_at, updated_at, body, organisation_id, conversation_id, user_id, parent_id) \
             VALUES (now(), now(), 'Paid it', {}, {finance}, {}, {})",
            team.org_id, team.leo_id, photo.id
        ))
        .await
        .unwrap();
        // Commits once the delete is waiting on its lock.
        let committer = tokio::spawn(async move {
            for _ in 0..100 {
                if waiting_locks(&conn).await > 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            txn.commit().await.unwrap();
        });
        let mut events = hub_events(&ctx);

        let res = delete(&request, &team.tom, &photo).await;
        committer.await.unwrap();
        assert_eq!(res.status_code(), 403, "{}", res.text());
        assert!(res.text().contains(messages::HAS_REPLIES), "{}", res.text());
        let kept = find(&ctx, photo.id).await;
        assert!(kept.photo_removed());
        assert_eq!(kept.body, INVOICE);
        assert_eq!(photo_removals(&mut events), vec![(finance, photo.id)]);
        assert!(deletions(&mut events).is_empty());
    })
    .await;
}

/// Opens a chat socket for `cookie`, on a thread with `thread`, and waits
/// until it listens (it echoes a message sent on it).
async fn open_socket(
    addr: SocketAddr,
    cookie: &str,
    conversation_id: i64,
    thread: Option<i64>,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let query = thread.map_or(String::new(), |t| format!("?thread={t}"));
    let mut request = format!("ws://{addr}/o/acme/chat/{conversation_id}/ws{query}")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    socket
        .send(Message::text(r#"{"body":"listening"}"#))
        .await
        .unwrap();
    let echo = tokio::time::timeout(super::live_sign_out::WAIT, socket.next())
        .await
        .unwrap();
    let Some(Ok(Message::Text(html))) = echo else {
        panic!("expected the echo, got {echo:?}");
    };
    assert!(
        html.contains(" data-live"),
        "a live message always starts a run: {html}"
    );
    socket
}

/// The next text frame that contains `marker`, skipping others.
async fn next_with(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    marker: &str,
) -> String {
    loop {
        let next = tokio::time::timeout(super::live_sign_out::WAIT, socket.next())
            .await
            .unwrap_or_else(|_| panic!("no frame with {marker}"));
        if let Some(Ok(Message::Text(html))) = next {
            if html.contains(marker) {
                return html.to_string();
            }
        }
    }
}

/// What a socket gets for each kind of delete, from a real server: ids and
/// counts, never the text. Every swap is top-level.
#[tokio::test]
#[serial]
async fn the_socket_payload_carries_no_text() {
    let (ctx, addr, cookie, alice, mut feed) = super::live_sign_out::open_general_socket().await;
    let general = messages::Entity::find()
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap()
        .conversation_id;
    let conversation = conversations::Entity::find_by_id(general)
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    let make = |body: &'static str| {
        let (db, conversation) = (ctx.db.clone(), conversation.clone());
        async move {
            messages::Model::create(
                &db,
                &conversation,
                alice.id,
                &messages::MessageParams {
                    body: body.to_string(),
                },
            )
            .await
            .unwrap()
        }
    };
    let leeds = make("Leeds £4.20 a tin").await;
    let parent = make("Who can do the stall on Saturday?").await;
    let mut replies = Vec::new();
    for body in ["Leeds reply one", "Leeds reply two"] {
        replies.push(
            messages::Model::create_reply(
                &ctx.db,
                &conversation,
                &parent,
                alice.id,
                &messages::MessageParams {
                    body: body.to_string(),
                },
            )
            .await
            .unwrap(),
        );
    }
    let mut thread = open_socket(addr, &cookie, general, Some(parent.id)).await;
    let mut events = hub_events(&ctx);
    let http = reqwest::Client::new();
    let delete = |id: i64| {
        let (http, cookie) = (http.clone(), cookie.clone());
        let org = conversation.organisation_id;
        async move {
            let res = http
                .post(format!("http://{addr}/o/acme{}", delete_path(general, id)))
                .header("cookie", &cookie)
                .header("x-org-id", org.to_string())
                .send()
                .await
                .unwrap();
            assert_eq!(res.status(), 200);
        }
    };
    let clean = |what: &str, html: &str| {
        assert!(
            !html.contains("Leeds") && !html.contains("stall"),
            "{what}: {html}"
        );
    };

    // A reply: the main feed swaps the parent's line; the thread drops the
    // reply and updates its count. The thread socket's own "listening" is
    // a reply too, so two are left.
    delete(replies[0].id).await;
    let html = next_with(&mut feed, "2 replies").await;
    assert!(
        html.contains(&format!(r#"id="thread-{}""#, parent.id)),
        "{html}"
    );
    clean("main feed, reply", &html);
    let html = next_with(&mut thread, "hx-swap-oob=\"delete\"").await;
    clean("thread, reply", &html);
    assert!(
        html.starts_with(&format!(
            r#"<div id="message-{}" hx-swap-oob="delete"></div>"#,
            replies[0].id
        )),
        "{html}"
    );
    assert!(
        html.contains(r#"id="thread-count""#) && html.contains(">2 replies<"),
        "{html}"
    );

    // The parent: the main feed removes it; the thread page says it is gone.
    delete(parent.id).await;
    let html = next_with(&mut feed, "hx-swap-oob=\"delete\"").await;
    assert_eq!(
        html,
        format!(
            r#"<div id="message-{}" hx-swap-oob="delete"></div>"#,
            parent.id
        )
    );
    let html = next_with(&mut thread, "This thread was deleted").await;
    clean("thread, parent", &html);
    assert!(
        html.starts_with(r#"<div id="chat-feed" hx-swap-oob="innerHTML">"#),
        "{html}"
    );
    assert!(
        html.ends_with(r#"<div id="thread-composer" hx-swap-oob="outerHTML"></div>"#),
        "{html}"
    );
    assert!(html.contains("Back to chat"), "{html}");

    // A plain message.
    delete(leeds.id).await;
    let html = next_with(&mut feed, "hx-swap-oob=\"delete\"").await;
    assert_eq!(
        html,
        format!(
            r#"<div id="message-{}" hx-swap-oob="delete"></div>"#,
            leeds.id
        )
    );

    while let Ok(event) = events.try_recv() {
        if matches!(event, ChatEvent::MessageDeleted { .. }) {
            let text = format!("{event:?}");
            assert!(!text.contains("Leeds") && !text.contains("stall"), "{text}");
        }
    }
}

/// Messages are deleted in one place, so the photo-first order and the
/// thread rule can't be skipped. `Entity::delete` is also part of
/// `Entity::delete_by_id`; the model's `notifications::Entity::delete_many`
/// is not counted. `truncate_table(.., messages::Entity)` in app.rs matches
/// no pattern.
#[test]
fn messages_are_only_deleted_by_the_model() {
    let mut offenders = Vec::new();
    let mut in_model = 0;
    let mut dirs = vec![std::path::PathBuf::from("src")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.to_string_lossy().replace('\\', "/");
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            // Calls split over lines count too.
            let text: String = std::fs::read_to_string(&path)
                .unwrap()
                .split_whitespace()
                .collect();
            if name == "src/models/messages.rs" {
                in_model = text.matches("Entity::delete").count()
                    - text.matches("notifications::Entity::delete").count();
                continue;
            }
            for pattern in [
                "messages::Entity::delete",
                "Messages::delete",
                "messages::ActiveModel::delete",
            ] {
                if text.contains(pattern) {
                    offenders.push(format!("{name}: {pattern}"));
                }
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:#?}");
    assert_eq!(in_model, 1, "only delete_message deletes a message row");
}
