//! Live chat. The browser opens `/chat/{id}/ws` with the HTMX ws extension,
//! sends messages as JSON (`{"body": "...", "HEADERS": {...}}`) and receives
//! rendered message HTML that HTMX appends to `#chat-feed`.
use axum::{
    extract::ws::{rejection::WebSocketUpgradeRejection, Message, WebSocket, WebSocketUpgrade},
    http::{header::ORIGIN, HeaderMap, StatusCode},
};
use loco_rs::prelude::*;
use tokio::sync::broadcast::error::RecvError;

use crate::{
    controllers::chat::{names, MessageView},
    data::{
        chat_hub::{ChatEvent, ChatHub},
        settings::Settings,
    },
    extractors::current_member::CurrentMember,
    models::{
        conversation_members, conversations,
        messages::{self, MessageParams},
    },
};

/// Who is on this socket and which conversation it follows.
struct Session {
    ctx: AppContext,
    view: TeraView,
    hub: ChatHub,
    org_id: i64,
    user_id: i64,
    conversation_id: i64,
}

/// Checks everything before upgrading: signed in and approved (the extractor),
/// a member of this conversation, and a same-site `Origin`.
#[debug_handler]
async fn connect(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(view): ViewEngine<TeraView>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    ws: std::result::Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Result<Response> {
    let app_url = Settings::from_context(&ctx)?.app_url;
    if let Some(origin) = headers.get(ORIGIN) {
        if origin.to_str().map_or(true, |o| o != app_url) {
            return Err(Error::CustomError(
                StatusCode::FORBIDDEN,
                loco_rs::controller::ErrorDetail::new("forbidden", "Cross-site WebSocket blocked"),
            ));
        }
    }
    let conversation =
        conversations::Model::find_for_member(&ctx.db, member.org.id, id, member.user.id).await?;
    let ws = ws.map_err(|_| Error::BadRequest("Expected a WebSocket upgrade.".to_string()))?;
    let hub = ctx
        .shared_store
        .get::<ChatHub>()
        .ok_or_else(|| Error::string("chat hub missing"))?;
    let session = Session {
        ctx,
        view,
        hub,
        org_id: member.org.id,
        user_id: member.user.id,
        conversation_id: conversation.id,
    };
    Ok(ws.on_upgrade(move |socket| run(socket, session)))
}

async fn run(mut socket: WebSocket, session: Session) {
    let mut events = session.hub.subscribe();
    loop {
        tokio::select! {
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    if let Err(err) = receive(&session, &text).await {
                        tracing::warn!(error = %err, user_id = session.user_id, "chat message rejected");
                    }
                }
                Some(Ok(_)) => {}
                _ => break,
            },
            event = events.recv() => match event {
                Ok(event) if event.conversation_id == session.conversation_id => {
                    match render(&session, event) {
                        Ok(html) => {
                            if socket.send(Message::Text(html.into())).await.is_err() {
                                break;
                            }
                            // The viewer has this conversation open, so it is read.
                            let _ = conversation_members::Model::mark_read(
                                &session.ctx.db,
                                session.org_id,
                                session.conversation_id,
                                session.user_id,
                            )
                            .await;
                        }
                        Err(err) => tracing::error!(error = %err, "could not render chat message"),
                    }
                }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
        }
    }
}

/// Saves a message from this socket and broadcasts it. Membership is checked
/// again on every send.
async fn receive(session: &Session, text: &str) -> Result<()> {
    let body = serde_json::from_str::<serde_json::Value>(text)?
        .get("body")
        .and_then(|b| b.as_str())
        .unwrap_or_default()
        .to_string();
    let db = &session.ctx.db;
    let conversation = conversations::Model::find_for_member(
        db,
        session.org_id,
        session.conversation_id,
        session.user_id,
    )
    .await?;
    let message =
        messages::Model::create(db, &conversation, session.user_id, &MessageParams { body })
            .await?;
    publish(&session.ctx, session.org_id, &message).await?;
    super::notifications::message_sent(&session.ctx, session.org_id, &conversation, &message).await
}

/// Announces a saved message to every socket following its conversation.
///
/// # Errors
/// When the hub is missing or usernames cannot be loaded.
pub async fn publish(ctx: &AppContext, org_id: i64, message: &messages::Model) -> Result<()> {
    let hub = ctx
        .shared_store
        .get::<ChatHub>()
        .ok_or_else(|| Error::string("chat hub missing"))?;
    let names = names(ctx, org_id).await?;
    hub.publish(ChatEvent {
        conversation_id: message.conversation_id,
        author_id: message.user_id,
        message: MessageView::new(message, &names, 0),
    });
    Ok(())
}

/// The message as HTML for this socket's viewer, wrapped for an out-of-band append.
fn render(session: &Session, event: ChatEvent) -> Result<String> {
    let mut message = event.message;
    message.own = event.author_id == session.user_id;
    session
        .view
        .render("chat/_message_oob.html", data!({ "message": message }))
}

pub fn routes() -> Routes {
    Routes::new().add("/chat/{id}/ws", get(connect))
}
