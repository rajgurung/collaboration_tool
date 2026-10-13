//! Ending someone's sessions also closes their open chat socket and
//! notification stream. Uses a real server, because the in-memory test
//! client cannot hold a WebSocket or a stream open.
use std::{net::SocketAddr, time::Duration};

use std::sync::Arc;

use collab::{
    app::App,
    data::chat_hub::{ChatEvent, ChatHub},
    models::{conversations, organisations, users},
};
use futures_util::{SinkExt, StreamExt};
use loco_rs::testing::request::boot_test;
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
    let mut request = format!("ws://{addr}/chat/{}/ws", general.id)
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
        .get(format!("http://{addr}/notifications/stream"))
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
