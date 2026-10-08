//! Live chat. The browser opens `/chat/{id}/ws` with the HTMX ws extension,
//! sends messages as JSON (`{"body": "...", "HEADERS": {...}}`) and receives
//! rendered message HTML that HTMX appends to `#chat-feed`.
use std::collections::HashMap;

use axum::{
    extract::ws::{rejection::WebSocketUpgradeRejection, Message, WebSocket, WebSocketUpgrade},
    http::{header::ORIGIN, HeaderMap, StatusCode},
};
use loco_rs::prelude::*;
use tokio::sync::broadcast::error::RecvError;

use crate::{
    controllers::chat::{names, receipt, MessageView},
    data::{
        chat_hub::{ChatEvent, ChatHub, ReceiptUpdate},
        settings::Settings,
    },
    extractors::current_member::CurrentMember,
    models::{
        conversation_members,
        conversations::{self, kind},
        messages::{self, MessageParams},
        organisations,
    },
    views::time,
};

/// Who is on this socket and which conversation it follows.
struct Session {
    ctx: AppContext,
    view: TeraView,
    hub: ChatHub,
    org_id: i64,
    user_id: i64,
    conversation_id: i64,
    conversation_kind: String,
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
        conversation_kind: conversation.kind,
    };
    Ok(ws.on_upgrade(move |socket| run(socket, session)))
}

async fn run(mut socket: WebSocket, session: Session) {
    let mut events = session.hub.subscribe();
    // Reader counts already sent, per message, so a late event cannot lower them.
    let mut sent_counts: HashMap<i64, usize> = HashMap::new();
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
                Ok(ChatEvent::Message { conversation_id, author_id, message })
                    if conversation_id == session.conversation_id =>
                {
                    match render(&session, author_id, message) {
                        Ok(html) => {
                            if socket.send(Message::Text(html.into())).await.is_err() {
                                break;
                            }
                            // The viewer has this conversation open, so it is read.
                            if let Err(err) = read_on_socket(&session, author_id).await {
                                tracing::warn!(error = %err, "could not mark chat read");
                            }
                        }
                        Err(err) => tracing::error!(error = %err, "could not render chat message"),
                    }
                }
                Ok(event) => {
                    let updates = not_older(
                        receipts_for(&event, session.conversation_id, session.user_id),
                        &mut sent_counts,
                    );
                    if updates.is_empty() {
                        continue;
                    }
                    match render_receipts(&session, &updates) {
                        Ok(html) => {
                            if socket.send(Message::Text(html.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(err) => tracing::error!(error = %err, "could not render read receipts"),
                    }
                }
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
        }
    }
}

/// Marks the conversation read for this socket's viewer. Their own message
/// covers nothing new from others, so it skips the receipt work.
async fn read_on_socket(session: &Session, author_id: i64) -> Result<()> {
    if author_id == session.user_id {
        conversation_members::Model::touch_read(
            &session.ctx.db,
            session.org_id,
            session.conversation_id,
            session.user_id,
        )
        .await?;
        return Ok(());
    }
    mark_read(
        &session.ctx,
        session.org_id,
        session.conversation_id,
        &session.conversation_kind,
        session.user_id,
    )
    .await
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
    let org = organisations::Model::find_by_id(&ctx.db, org_id).await?;
    hub.publish(ChatEvent::Message {
        conversation_id: message.conversation_id,
        author_id: message.user_id,
        message: MessageView::new(message, &names, 0, time::zone(&org.timezone)),
    });
    Ok(())
}

/// Marks a conversation read for `reader_id` and, in a DM or group, tells the
/// authors of the messages that read covered who has now read them.
///
/// # Errors
/// On database errors, or when the hub is missing.
pub async fn mark_read(
    ctx: &AppContext,
    org_id: i64,
    conversation_id: i64,
    conversation_kind: &str,
    reader_id: i64,
) -> Result<()> {
    let span =
        conversation_members::Model::mark_read(&ctx.db, org_id, conversation_id, reader_id).await?;
    let Some(span) = span else { return Ok(()) };
    if conversation_kind == kind::CHANNEL {
        return Ok(());
    }
    let read =
        messages::Model::read_in_span(&ctx.db, org_id, conversation_id, reader_id, &span).await?;
    if read.is_empty() {
        return Ok(());
    }
    let marks = conversation_members::Model::read_marks(&ctx.db, org_id, conversation_id).await?;
    let names = names(ctx, org_id).await?;
    let receipts = read
        .iter()
        .map(|m| ReceiptUpdate {
            message_id: m.id,
            author_id: m.user_id,
            receipt: receipt(conversation_kind, m.user_id, m.created_at, &marks, &names),
        })
        .collect();
    ctx.shared_store
        .get::<ChatHub>()
        .ok_or_else(|| Error::string("chat hub missing"))?
        .publish(ChatEvent::Read {
            conversation_id,
            receipts,
        });
    Ok(())
}

/// The message as HTML for this socket's viewer, wrapped for an out-of-band append.
fn render(session: &Session, author_id: i64, mut message: MessageView) -> Result<String> {
    message.own = author_id == session.user_id;
    message.read_receipts = message.own && session.conversation_kind != kind::CHANNEL;
    session
        .view
        .render("chat/_message_oob.html", data!({ "message": message }))
}

/// The receipt updates in `event` that belong on this viewer's screen: only
/// for this conversation, and only on messages the viewer wrote.
fn receipts_for(event: &ChatEvent, conversation_id: i64, viewer_id: i64) -> Vec<&ReceiptUpdate> {
    match event {
        ChatEvent::Read {
            conversation_id: id,
            receipts,
        } if *id == conversation_id => receipts
            .iter()
            .filter(|r| r.author_id == viewer_id)
            .collect(),
        _ => Vec::new(),
    }
}

/// Drops updates with fewer readers than this socket already showed. Readers
/// only grow, so a lower count is a read event that arrived out of order.
fn not_older<'a>(
    updates: Vec<&'a ReceiptUpdate>,
    sent_counts: &mut HashMap<i64, usize>,
) -> Vec<&'a ReceiptUpdate> {
    updates
        .into_iter()
        .filter(|u| {
            let count = u.receipt.as_ref().map_or(0, |r| r.readers.len());
            let last = sent_counts.entry(u.message_id).or_insert(0);
            if count < *last {
                return false;
            }
            *last = count;
            true
        })
        .collect()
}

/// Receipts as out-of-band swaps of each message's `#receipt-{id}` slot.
fn render_receipts(session: &Session, updates: &[&ReceiptUpdate]) -> Result<String> {
    let mut html = String::new();
    for update in updates {
        html.push_str(&session.view.render(
            "chat/_receipt.html",
            data!({
                "message": { "id": update.message_id, "receipt": update.receipt },
                "oob": true,
            }),
        )?);
    }
    Ok(html)
}

pub fn routes() -> Routes {
    Routes::new().add("/chat/{id}/ws", get(connect))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controllers::chat::Receipt;

    fn update(message_id: i64, author_id: i64) -> ReceiptUpdate {
        ReceiptUpdate {
            message_id,
            author_id,
            receipt: None,
        }
    }

    #[test]
    fn sockets_only_get_receipts_for_their_viewers_messages() {
        let event = ChatEvent::Read {
            conversation_id: 7,
            receipts: vec![update(10, 1), update(11, 2), update(12, 1)],
        };
        let mine: Vec<i64> = receipts_for(&event, 7, 1)
            .iter()
            .map(|r| r.message_id)
            .collect();
        assert_eq!(mine, vec![10, 12]);
        assert!(receipts_for(&event, 7, 3).is_empty(), "not the author");
        assert!(
            receipts_for(&event, 8, 1).is_empty(),
            "another conversation"
        );
    }

    fn read_by(message_id: i64, readers: &[&str]) -> ReceiptUpdate {
        ReceiptUpdate {
            message_id,
            author_id: 1,
            receipt: Some(Receipt {
                text: String::new(),
                readers: readers.iter().map(ToString::to_string).collect(),
                all: false,
            }),
        }
    }

    #[test]
    fn late_read_events_never_lower_a_count() {
        let mut sent = HashMap::new();
        let everyone = read_by(10, &["bob", "carol"]);
        let other = read_by(11, &["bob"]);
        assert_eq!(not_older(vec![&everyone, &other], &mut sent).len(), 2);

        // Bob's event arrives after carol's: message 10 keeps "everyone".
        let late = read_by(10, &["bob"]);
        let newer = read_by(11, &["bob", "carol"]);
        let kept: Vec<i64> = not_older(vec![&late, &newer], &mut sent)
            .iter()
            .map(|u| u.message_id)
            .collect();
        assert_eq!(kept, vec![11]);

        // The same count again is still sent.
        assert_eq!(not_older(vec![&everyone], &mut sent).len(), 1);
    }

    #[test]
    fn new_messages_carry_no_receipts() {
        let message = messages::Model {
            created_at: chrono::Utc::now().into(),
            updated_at: chrono::Utc::now().into(),
            id: 10,
            body: "hi".to_string(),
            organisation_id: 1,
            conversation_id: 7,
            user_id: 1,
        };
        let event = ChatEvent::Message {
            conversation_id: 7,
            author_id: 1,
            message: MessageView::new(&message, &HashMap::new(), 1, chrono_tz::UTC),
        };
        assert!(receipts_for(&event, 7, 1).is_empty());
    }
}
