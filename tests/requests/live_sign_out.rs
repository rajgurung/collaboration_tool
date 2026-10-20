//! Ending someone's sessions also closes their open chat socket and
//! notification stream; a removal closes only that org's. Each tab's socket
//! and stream live under its own `/o/{slug}/`, so nothing else closes them.
//! Uses a real server, because the in-memory test client cannot hold a
//! WebSocket or a stream open.
use std::{net::SocketAddr, time::Duration};

use std::sync::Arc;

use collab::{
    app::App,
    data::{
        chat_hub::{ChatEvent, ChatHub},
        notify_hub::NotifyHub,
    },
    models::{
        conversation_members, conversations, login_events::Client, memberships, organisations,
        users,
    },
};
use futures_util::{SinkExt, StreamExt};
use loco_rs::{prelude::TenantActiveModelExt, testing::request::boot_test};
use sea_orm::{ActiveModelTrait, ActiveValue};
use serial_test::serial;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

pub(super) const WAIT: Duration = Duration::from_secs(5);

/// A real server with Acme's owner signed in and her General chat socket open
/// and listening.
pub(super) async fn open_general_socket() -> (
    loco_rs::app::AppContext,
    SocketAddr,
    String,
    users::Model,
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
) {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = boot.app_context.clone();
    let router = boot.router.clone().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let signup = http
        .post(format!("http://{addr}/signup"))
        .form(&[
            ("organisation_name", "Acme"),
            ("name", "alice"),
            ("email", "alice@example.com"),
            ("password", "correct-horse"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(signup.status(), 303);
    let cookie = signup.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
        .await
        .unwrap();
    let org = organisations::Model::find_by_slug(&ctx.db, "acme")
        .await
        .unwrap();
    let general = conversations::Model::find_general(&ctx.db, org.id)
        .await
        .unwrap();
    let mut request = format!("ws://{addr}/o/acme/chat/{}/ws", general.id)
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    // The socket echoes our own message once it is listening for events.
    socket
        .send(Message::text(r#"{"body":"hello"}"#))
        .await
        .unwrap();
    let echo = tokio::time::timeout(WAIT, socket.next()).await.unwrap();
    assert!(matches!(echo, Some(Ok(Message::Text(_)))), "{echo:?}");
    (ctx, addr, cookie, alice, socket)
}

#[tokio::test]
#[serial]
async fn ending_sessions_closes_the_chat_socket_and_the_notification_stream() {
    let (ctx, addr, cookie, alice, mut socket) = open_general_socket().await;
    let http = reqwest::Client::new();
    let mut stream = http
        .get(format!("http://{addr}/o/acme/notifications/stream"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), 200);

    users::Model::bump_session_version(&ctx, alice.id)
        .await
        .unwrap();

    let closed = tokio::time::timeout(WAIT, socket.next()).await.unwrap();
    assert!(
        matches!(closed, None | Some(Ok(Message::Close(_)))),
        "{closed:?}"
    );
    let ended = tokio::time::timeout(WAIT, stream.chunk()).await.unwrap();
    assert!(matches!(ended, Ok(None)), "{ended:?}");
}

/// A socket that falls behind may have missed a sign-out, so it closes.
#[tokio::test]
#[serial]
async fn a_chat_socket_that_falls_behind_closes() {
    let (ctx, _, _, _, mut socket) = open_general_socket().await;
    let hub = ctx.shared_store.get::<ChatHub>().unwrap();
    // More than the hub buffers, sent without yielding, about a conversation
    // this socket ignores.
    for _ in 0..600 {
        hub.publish(ChatEvent::Read {
            conversation_id: -1,
            receipts: Arc::new(Vec::new()),
        });
    }
    let closed = tokio::time::timeout(WAIT, socket.next()).await.unwrap();
    assert!(
        matches!(closed, None | Some(Ok(Message::Close(_)))),
        "{closed:?}"
    );
}

/// Signs up Beta (owned by Bob) and makes Alice an active member of it
/// directly. Returns Beta's id.
async fn alice_also_in_beta(ctx: &loco_rs::app::AppContext, addr: SocketAddr, alice: i64) -> i64 {
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let signup = http
        .post(format!("http://{addr}/signup"))
        .form(&[
            ("organisation_name", "Beta"),
            ("name", "bob"),
            ("email", "bob@example.com"),
            ("password", "correct-horse"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(signup.status(), 303);
    let beta = organisations::Model::find_by_slug(&ctx.db, "beta")
        .await
        .unwrap();
    memberships::ActiveModel {
        user_id: ActiveValue::Set(alice),
        username: ActiveValue::Set("alice".to_string()),
        role: ActiveValue::Set(memberships::role::MEMBER.to_string()),
        status: ActiveValue::Set(memberships::status::ACTIVE.to_string()),
        ..Default::default()
    }
    .set_tenant(beta.id)
    .unwrap()
    .insert(&ctx.db)
    .await
    .unwrap();
    beta.id
}

/// The old switch, for tabs drawn before the deploy.
async fn switch(addr: SocketAddr, cookie: &str, org_id: i64) -> reqwest::Response {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .post(format!("http://{addr}/orgs/{org_id}/switch"))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap()
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A second sign-in for Alice: another browser, with its own sign-in id.
async fn sign_in_again(addr: SocketAddr) -> String {
    let res = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .post(format!("http://{addr}/login"))
        .form(&[
            ("email", "alice@example.com"),
            ("password", "correct-horse"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 303);
    res.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

/// Opens `conversation_id`'s socket in `slug` with `cookie` and waits until it
/// listens.
async fn open_socket(addr: SocketAddr, cookie: &str, slug: &str, conversation_id: i64) -> Socket {
    let mut request = format!("ws://{addr}/o/{slug}/chat/{conversation_id}/ws")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert_still_open(&mut socket).await;
    socket
}

/// The socket echoes a message sent on it, so it is open and listening.
async fn assert_still_open(socket: &mut Socket) {
    socket
        .send(Message::text(r#"{"body":"still here"}"#))
        .await
        .unwrap();
    let echo = tokio::time::timeout(WAIT, socket.next()).await.unwrap();
    assert!(matches!(echo, Some(Ok(Message::Text(_)))), "{echo:?}");
}

async fn assert_closed(socket: &mut Socket) {
    let closed = tokio::time::timeout(WAIT, socket.next()).await.unwrap();
    assert!(
        matches!(closed, None | Some(Ok(Message::Close(_)))),
        "{closed:?}"
    );
}

async fn open_stream(addr: SocketAddr, cookie: &str, slug: &str) -> reqwest::Response {
    let stream = reqwest::Client::new()
        .get(format!("http://{addr}/o/{slug}/notifications/stream"))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), 200);
    stream
}

async fn assert_stream_ended(stream: &mut reqwest::Response) {
    let ended = tokio::time::timeout(WAIT, stream.chunk()).await.unwrap();
    assert!(matches!(ended, Ok(None)), "{ended:?}");
}

/// A notice for Alice in `org_id` reaches the stream, so it is still open.
async fn assert_stream_open(
    ctx: &loco_rs::app::AppContext,
    stream: &mut reqwest::Response,
    org_id: i64,
    alice: i64,
) {
    ctx.shared_store
        .get::<NotifyHub>()
        .unwrap()
        .publish(org_id, &[alice]);
    let chunk = tokio::time::timeout(WAIT, stream.chunk()).await.unwrap();
    assert!(
        matches!(&chunk, Ok(Some(bytes)) if String::from_utf8_lossy(bytes).contains("notify")),
        "{chunk:?}"
    );
}

/// Alice in Beta too, with a seat in its General. Returns Beta, her Beta
/// membership and Beta's General.
async fn alice_in_beta_general(
    ctx: &loco_rs::app::AppContext,
    addr: SocketAddr,
    alice: i64,
) -> (i64, memberships::Model, i64) {
    let beta = alice_also_in_beta(ctx, addr, alice).await;
    let general = conversations::Model::find_general(&ctx.db, beta)
        .await
        .unwrap();
    conversation_members::Model::add(&ctx.db, &general, alice)
        .await
        .unwrap();
    let membership = memberships::Model::active_in(&ctx.db, beta, alice)
        .await
        .unwrap()
        .unwrap();
    (beta, membership, general.id)
}

async fn bob_removes(ctx: &loco_rs::app::AppContext, beta: i64, membership: &memberships::Model) {
    let bob = users::Model::find_by_email(&ctx.db, "bob@example.com")
        .await
        .unwrap();
    let client = Client {
        ip: "127.0.0.1".to_string(),
        user_agent: "test".to_string(),
    };
    memberships::Model::remove(ctx, beta, bob.id, membership.id, &client)
        .await
        .unwrap();
}

#[tokio::test]
#[serial]
async fn removal_from_beta_closes_only_beta_sockets_and_streams() {
    // Browser 2 (the signup cookie) stays in Acme with General open.
    let (ctx, addr, in_acme, alice, mut acme_socket) = open_general_socket().await;
    let acme = organisations::Model::find_by_slug(&ctx.db, "acme")
        .await
        .unwrap()
        .id;
    let mut acme_stream = open_stream(addr, &in_acme, "acme").await;
    // Browser 1 works in Beta.
    let (beta, membership, beta_general) = alice_in_beta_general(&ctx, addr, alice.id).await;
    let in_beta = sign_in_again(addr).await;
    let mut beta_socket = open_socket(addr, &in_beta, "beta", beta_general).await;
    let mut beta_stream = open_stream(addr, &in_beta, "beta").await;

    bob_removes(&ctx, beta, &membership).await;

    assert_closed(&mut beta_socket).await;
    assert_stream_ended(&mut beta_stream).await;
    assert_still_open(&mut acme_socket).await;
    assert_stream_open(&ctx, &mut acme_stream, acme, alice.id).await;
}

#[tokio::test]
#[serial]
async fn removal_closes_the_beta_socket_without_a_session_change() {
    let (ctx, addr, cookie, alice, _acme_socket) = open_general_socket().await;
    let (beta, membership, beta_general) = alice_in_beta_general(&ctx, addr, alice.id).await;
    let mut beta_socket = open_socket(addr, &cookie, "beta", beta_general).await;
    let before = users::Model::find_by_id(&ctx.db, alice.id).await.unwrap();

    bob_removes(&ctx, beta, &membership).await;

    assert_closed(&mut beta_socket).await;
    let after = users::Model::find_by_id(&ctx.db, alice.id).await.unwrap();
    assert_eq!(after.session_version, before.session_version);
}

#[tokio::test]
#[serial]
async fn the_old_switch_post_lands_under_o_and_closes_nothing() {
    let (ctx, addr, cookie, alice, mut socket) = open_general_socket().await;
    let acme = organisations::Model::find_by_slug(&ctx.db, "acme")
        .await
        .unwrap()
        .id;
    let mut stream = open_stream(addr, &cookie, "acme").await;
    let beta = alice_also_in_beta(&ctx, addr, alice.id).await;

    let res = switch(addr, &cookie, beta).await;
    assert_eq!(res.status(), 303);
    assert_eq!(res.headers()["location"], "/o/beta/dashboard");

    assert_still_open(&mut socket).await;
    assert_stream_open(&ctx, &mut stream, acme, alice.id).await;
}

#[tokio::test]
#[serial]
async fn creating_an_org_lands_in_it_and_closes_nothing() {
    let (ctx, addr, cookie, alice, mut socket) = open_general_socket().await;
    let acme = organisations::Model::find_by_slug(&ctx.db, "acme")
        .await
        .unwrap()
        .id;
    let mut stream = open_stream(addr, &cookie, "acme").await;
    let res = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .post(format!("http://{addr}/o/acme/orgs"))
        .header("cookie", &cookie)
        .header("x-org-id", acme.to_string())
        .form(&[("organisation_name", "Lumen Café"), ("name", "ali")])
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 303);
    assert_eq!(
        res.headers()["location"],
        "/o/lumen-caf/dashboard?created=1"
    );

    assert_still_open(&mut socket).await;
    assert_stream_open(&ctx, &mut stream, acme, alice.id).await;
}

/// One sign-in with Acme and Beta open side by side: each tab's socket gets
/// its own org's messages, with links under its own address.
#[tokio::test]
#[serial]
async fn two_orgs_chat_live_in_their_own_tabs() {
    let (ctx, addr, cookie, alice, mut acme_socket) = open_general_socket().await;
    let (beta, _, beta_general) = alice_in_beta_general(&ctx, addr, alice.id).await;
    let mut beta_socket = open_socket(addr, &cookie, "beta", beta_general).await;
    let acme = organisations::Model::find_by_slug(&ctx.db, "acme")
        .await
        .unwrap();
    let acme_general = conversations::Model::find_general(&ctx.db, acme.id)
        .await
        .unwrap();

    // Bob writes in Beta's General: only the Beta tab hears it.
    let bob = users::Model::find_by_email(&ctx.db, "bob@example.com")
        .await
        .unwrap();
    let beta_general_model = conversations::Model::find_general(&ctx.db, beta)
        .await
        .unwrap();
    let said = collab::models::messages::Model::create(
        &ctx.db,
        &beta_general_model,
        bob.id,
        &collab::models::messages::MessageParams {
            body: "Beta news".to_string(),
        },
    )
    .await
    .unwrap();
    collab::controllers::chat_ws::publish(&ctx, beta, &said)
        .await
        .unwrap();
    let frame = tokio::time::timeout(WAIT, beta_socket.next())
        .await
        .unwrap();
    let Some(Ok(Message::Text(html))) = frame else {
        panic!("expected Beta's message, got {frame:?}");
    };
    assert!(html.contains("Beta news"), "{html}");
    assert_links_under(&html, "/o/beta/");
    assert_still_open(&mut acme_socket).await;

    // And Acme's tab hears Acme's own, with Acme links.
    let said = collab::models::messages::Model::create(
        &ctx.db,
        &acme_general,
        alice.id,
        &collab::models::messages::MessageParams {
            body: "Acme news".to_string(),
        },
    )
    .await
    .unwrap();
    collab::controllers::chat_ws::publish(&ctx, acme.id, &said)
        .await
        .unwrap();
    let frame = tokio::time::timeout(WAIT, acme_socket.next())
        .await
        .unwrap();
    let Some(Ok(Message::Text(html))) = frame else {
        panic!("expected Acme's message, got {frame:?}");
    };
    assert!(html.contains("Acme news"), "{html}");
    assert_links_under(&html, "/o/acme/");
    assert_still_open(&mut beta_socket).await;
}

/// Every app link in `html` is under `base`.
fn assert_links_under(html: &str, base: &str) {
    let attrs = regex_lite_links(html);
    assert!(!attrs.is_empty(), "no links in {html}");
    for value in attrs {
        assert!(value.starts_with(base), "{value} in {html}");
    }
}

/// The values of `href`, `hx-post`, `hx-get` and `data-*-url` attributes that
/// start with `/`.
fn regex_lite_links(html: &str) -> Vec<String> {
    let re = regex::Regex::new(r#"(?:href|hx-get|hx-post|data-[a-z-]*url)="(/[^"]*)""#).unwrap();
    re.captures_iter(html).map(|c| c[1].to_string()).collect()
}

#[tokio::test]
#[serial]
async fn each_tabs_stream_counts_its_own_org() {
    let (ctx, addr, cookie, alice, _socket) = open_general_socket().await;
    let acme = organisations::Model::find_by_slug(&ctx.db, "acme")
        .await
        .unwrap()
        .id;
    let beta = alice_also_in_beta(&ctx, addr, alice.id).await;
    let mut acme_stream = open_stream(addr, &cookie, "acme").await;
    let mut beta_stream = open_stream(addr, &cookie, "beta").await;

    // A notice in Beta reaches the Beta tab only.
    assert_stream_open(&ctx, &mut beta_stream, beta, alice.id).await;
    let quiet = tokio::time::timeout(Duration::from_millis(300), acme_stream.chunk()).await;
    assert!(quiet.is_err(), "nothing for the Acme tab: {quiet:?}");
    assert_stream_open(&ctx, &mut acme_stream, acme, alice.id).await;
}

#[tokio::test]
#[serial]
async fn a_bare_socket_upgrade_is_not_upgraded() {
    let (ctx, addr, cookie, _alice, _socket) = open_general_socket().await;
    let acme = organisations::Model::find_by_slug(&ctx.db, "acme")
        .await
        .unwrap();
    let general = conversations::Model::find_general(&ctx.db, acme.id)
        .await
        .unwrap();
    let mut request = format!("ws://{addr}/chat/{}/ws", general.id)
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    let refused = tokio_tungstenite::connect_async(request).await;
    match refused {
        Err(tokio_tungstenite::tungstenite::Error::Http(res)) => {
            assert_eq!(res.status(), 303);
            assert_eq!(res.headers()["location"], "/o/acme/dashboard?from=");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
#[serial]
async fn a_bare_stream_is_not_a_stream() {
    let (_ctx, addr, cookie, _alice, _socket) = open_general_socket().await;
    let res = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(format!("http://{addr}/notifications/stream"))
        .header("cookie", &cookie)
        .header("accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 303);
    assert_eq!(res.headers()["location"], "/o/acme/dashboard?from=");
    assert_ne!(
        res.headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap()),
        Some("text/event-stream")
    );
}
