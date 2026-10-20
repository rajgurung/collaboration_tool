//! Live chat. The browser opens `/chat/{id}/ws` with the HTMX ws extension,
//! sends messages as JSON (`{"body": "...", "HEADERS": {...}}`) and receives
//! rendered message HTML that HTMX appends to `#chat-feed`. A thread page
//! opens `/chat/{id}/ws?thread={parent_id}`: what it sends are replies, and it
//! receives only that thread's events.
use std::{collections::HashMap, sync::Arc};

use axum::{
    extract::ws::{rejection::WebSocketUpgradeRejection, Message, WebSocket, WebSocketUpgrade},
    http::{header::ORIGIN, HeaderMap, StatusCode},
};
use loco_rs::prelude::*;
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

use crate::{
    controllers::chat::{
        names, receipt, render_reactions, shows_receipt, thread_line, MessageView, ReactionRow,
        ThreadLine,
    },
    data::{
        chat_hub::{ChatEvent, ChatHub, ReceiptUpdate},
        settings::Settings,
    },
    extractors::current_member::CurrentMember,
    models::{
        conversation_members, conversations, memberships,
        messages::{self, MessageParams},
        organisations, users,
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
    /// The session version this socket was opened with.
    session_version: i32,
    /// The sign-in this socket was opened with, for closing one browser's
    /// sockets on a switch. `None` for cookies from before sign-in ids.
    session_id: Option<String>,
    conversation_id: i64,
    conversation_kind: String,
    /// The thread's parent, on a thread page's socket.
    thread_id: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct WsQuery {
    thread: Option<i64>,
}

/// Checks everything before upgrading: signed in and approved (the extractor),
/// a member of this conversation, a thread that is in it, and a same-site
/// `Origin`.
#[debug_handler]
async fn connect(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(view): ViewEngine<TeraView>,
    Path(id): Path<i64>,
    Query(query): Query<WsQuery>,
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
    if let Some(parent_id) = query.thread {
        messages::Model::find_thread_parent(&ctx.db, &conversation, parent_id).await?;
    }
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
        session_version: member.user.session_version,
        session_id: member.session_id,
        conversation_id: conversation.id,
        conversation_kind: conversation.kind,
        thread_id: query.thread,
    };
    Ok(ws.on_upgrade(move |socket| run(socket, session)))
}

/// Whether the socket's viewer may still follow the conversation: same
/// session version, still an active member of the organisation, and still one
/// of the conversation's members (removal takes them out). No skip for
/// acting: a super admin acting elsewhere has no seat in any conversation, so
/// `connect` already refused them.
async fn still_allowed(session: &Session) -> bool {
    let db = &session.ctx.db;
    let current = users::Model::find_by_id(db, session.user_id)
        .await
        .is_ok_and(|u| u.session_version == session.session_version);
    current
        && memberships::Model::active_in(db, session.org_id, session.user_id)
            .await
            .is_ok_and(|m| m.is_some())
        && conversations::Model::find_for_member(
            db,
            session.org_id,
            session.conversation_id,
            session.user_id,
        )
        .await
        .is_ok()
}

async fn run(mut socket: WebSocket, session: Session) {
    let mut events = session.hub.subscribe();
    // A close (sign-out, removal) between the checks in `connect` and
    // subscribing would be missed.
    if !still_allowed(&session).await {
        let _ = socket.send(Message::Close(None)).await;
        return;
    }
    // Reader counts already sent, per message, so a late event cannot lower them.
    let mut sent_counts: HashMap<i64, usize> = HashMap::new();
    // Reply counts already sent, per thread, for the same reason.
    let mut sent_replies: HashMap<i64, i64> = HashMap::new();
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
                Ok(ChatEvent::Close(scope))
                    if scope.closes(session.org_id, session.user_id, session.session_id.as_deref()) =>
                {
                    let _ = socket.send(Message::Close(None)).await;
                    break;
                }
                // The hub carries every organisation's events.
                Ok(event) if !wants(&event, session.conversation_id, session.thread_id) => {}
                Ok(ChatEvent::Message { author_id, message, .. }) => {
                    match render(&session, author_id, *message) {
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
                Ok(ChatEvent::Reply { parent_id, author_id, message, line, .. }) => {
                    let grew = grows(&mut sent_replies, parent_id, line.count);
                    match render_reply(&session, author_id, *message, &line, grew) {
                        Ok(html) if html.is_empty() => {}
                        Ok(html) => {
                            if socket.send(Message::Text(html.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(err) => tracing::error!(error = %err, "could not render thread reply"),
                    }
                }
                Ok(ChatEvent::Reactions { message_id, rows, .. }) => {
                    match render_reactions(&session.view, session.conversation_id, message_id, &rows, session.user_id) {
                        Ok(html) => {
                            if socket.send(Message::Text(html.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(err) => tracing::error!(error = %err, "could not render reactions"),
                    }
                }
                Ok(event @ ChatEvent::Read { .. }) => {
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
                // Someone else's sockets, or another org's or browser's.
                Ok(ChatEvent::Close(_)) => {}
                // Skipped events may include a close. Close; the browser
                // reconnects through the same checks and reloads the feed.
                Err(RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, user_id = session.user_id, "chat socket fell behind");
                    let _ = socket.send(Message::Close(None)).await;
                    break;
                }
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

/// Saves a message from this socket and broadcasts it: a reply on a thread's
/// socket, else a message. Membership, and the thread, are checked again on
/// every send. The parent comes from the session, never from the browser.
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
    let params = MessageParams { body };
    if let Some(parent_id) = session.thread_id {
        let parent = messages::Model::find_thread_parent(db, &conversation, parent_id).await?;
        let reply =
            messages::Model::create_reply(db, &conversation, &parent, session.user_id, &params)
                .await?;
        publish_reply(&session.ctx, session.org_id, &parent, &reply).await?;
        return super::notifications::reply_sent(
            &session.ctx,
            session.org_id,
            &conversation,
            &parent,
            &reply,
        )
        .await;
    }
    let message = messages::Model::create(db, &conversation, session.user_id, &params).await?;
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
        message: Box::new(MessageView::new(
            message,
            &names,
            0,
            time::zone(&org.timezone),
        )),
    });
    Ok(())
}

/// Announces a saved thread reply, with the parent's new reply line, to every
/// socket following its conversation.
///
/// # Errors
/// When the hub is missing or the thread cannot be read.
pub async fn publish_reply(
    ctx: &AppContext,
    org_id: i64,
    parent: &messages::Model,
    reply: &messages::Model,
) -> Result<()> {
    let hub = ctx
        .shared_store
        .get::<ChatHub>()
        .ok_or_else(|| Error::string("chat hub missing"))?;
    let names = names(ctx, org_id).await?;
    let tz = time::zone(
        &organisations::Model::find_by_id(&ctx.db, org_id)
            .await?
            .timezone,
    );
    let stats = messages::Model::thread_stats(&ctx.db, org_id, &[parent.id]).await?;
    let line = thread_line(
        parent.conversation_id,
        parent.id,
        stats.get(&parent.id).map_or(&[], Vec::as_slice),
        &names,
        tz,
    )
    .ok_or_else(|| Error::string("a saved reply is missing from its thread"))?;
    hub.publish(ChatEvent::Reply {
        conversation_id: parent.conversation_id,
        parent_id: parent.id,
        author_id: reply.user_id,
        message: Box::new(MessageView::new(reply, &names, 0, tz)),
        line: Box::new(line),
    });
    Ok(())
}

/// Announces a message's reactions, read after a change, to every socket
/// following its conversation.
///
/// # Errors
/// When the hub is missing.
pub fn publish_reactions(
    ctx: &AppContext,
    message: &messages::Model,
    rows: Arc<Vec<ReactionRow>>,
) -> Result<()> {
    ctx.shared_store
        .get::<ChatHub>()
        .ok_or_else(|| Error::string("chat hub missing"))?
        .publish(ChatEvent::Reactions {
            conversation_id: message.conversation_id,
            message_id: message.id,
            parent_id: message.parent_id,
            rows,
        });
    Ok(())
}

/// Marks a conversation read for `reader_id` and tells everyone who sees the
/// receipts of the messages that read covered (see [`receipts_for`]) who has
/// now read them.
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
    let read =
        messages::Model::read_in_span(&ctx.db, org_id, conversation_id, reader_id, &span).await?;
    if read.is_empty() {
        return Ok(());
    }
    let marks = conversation_members::Model::read_marks(&ctx.db, org_id, conversation_id).await?;
    // Only this conversation's members: everyone has General open, so this runs often.
    let names: HashMap<i64, String> =
        memberships::Model::team_among(&ctx.db, org_id, marks.iter().map(|(id, _, _)| *id))
            .await?
            .into_iter()
            .collect();
    let receipts: Vec<ReceiptUpdate> = read
        .iter()
        .map(|m| ReceiptUpdate {
            message_id: m.id,
            author_id: m.user_id,
            dm: conversation_kind == conversations::kind::DM,
            receipt: receipt(conversation_kind, m.user_id, m.created_at, &marks, &names),
        })
        .collect();
    ctx.shared_store
        .get::<ChatHub>()
        .ok_or_else(|| Error::string("chat hub missing"))?
        .publish(ChatEvent::Read {
            conversation_id,
            receipts: Arc::new(receipts),
        });
    Ok(())
}

/// The message as HTML for this socket's viewer, wrapped for an out-of-band append.
fn render(session: &Session, author_id: i64, mut message: MessageView) -> Result<String> {
    message.own = author_id == session.user_id;
    message.read_receipts = shows_receipt(&session.conversation_kind, message.own);
    session
        .view
        .render("chat/_message_oob.html", data!({ "message": message }))
}

/// A reply for this socket's viewer. A main-feed socket swaps the parent's
/// reply line; a thread socket appends the reply and updates the count. A
/// count that did not `grow` is a late event, so its line or count is left
/// out (the reply itself is still appended). Empty when nothing is left.
fn render_reply(
    session: &Session,
    author_id: i64,
    mut message: MessageView,
    line: &ThreadLine,
    grew: bool,
) -> Result<String> {
    let Some(parent_id) = message.parent_id else {
        return Ok(String::new());
    };
    if session.thread_id.is_none() {
        if !grew {
            return Ok(String::new());
        }
        return session.view.render(
            "chat/_thread_link.html",
            data!({ "message": { "id": parent_id, "thread": line }, "oob": true }),
        );
    }
    message.own = author_id == session.user_id;
    message.read_receipts = false;
    message.can_reply = false;
    let mut html = session.view.render(
        "chat/_message_oob.html",
        data!({ "message": message, "in_thread": true }),
    )?;
    if grew {
        html.push_str(&session.view.render(
            "chat/_thread_count.html",
            data!({ "count": line.count, "count_label": line.label, "oob": true }),
        )?);
    }
    Ok(html)
}

/// Whether `count` replies is at least what this socket last showed for the
/// thread, recording it if so. Replies only grow, so a lower count is a reply
/// event that arrived out of order.
fn grows(sent: &mut HashMap<i64, i64>, parent_id: i64, count: i64) -> bool {
    let last = sent.entry(parent_id).or_insert(0);
    if count < *last {
        return false;
    }
    *last = count;
    true
}

/// Whether a socket should handle `event`. The socket follows
/// `conversation_id`, and with `thread_id` set only that thread. The hub is one
/// broadcast for the whole process, so the conversation check, made first for
/// every event, is what keeps another conversation's (or organisation's) HTML
/// off this socket. Main-feed sockets take only main-feed events; a thread
/// socket takes only its own thread's. Closes are about a person, not a
/// conversation, and are matched before this.
fn wants(event: &ChatEvent, conversation_id: i64, thread_id: Option<i64>) -> bool {
    let id = match event {
        ChatEvent::Message {
            conversation_id: id,
            ..
        }
        | ChatEvent::Read {
            conversation_id: id,
            ..
        }
        | ChatEvent::Reactions {
            conversation_id: id,
            ..
        }
        | ChatEvent::Reply {
            conversation_id: id,
            ..
        } => *id,
        ChatEvent::Close(_) => return false,
    };
    if id != conversation_id {
        return false;
    }
    match (event, thread_id) {
        (ChatEvent::Message { .. } | ChatEvent::Read { .. }, None) => true,
        (ChatEvent::Message { .. } | ChatEvent::Read { .. }, Some(_)) => false,
        // The parent's reply line in the main feed; the reply in its thread.
        (ChatEvent::Reply { .. }, None) => true,
        (ChatEvent::Reply { parent_id, .. }, Some(thread)) => *parent_id == thread,
        // A reaction on a reply belongs in its thread; one on a top-level
        // message in the main feed, and also on that message's thread page.
        (ChatEvent::Reactions { parent_id, .. }, None) => parent_id.is_none(),
        (
            ChatEvent::Reactions {
                message_id,
                parent_id,
                ..
            },
            Some(thread),
        ) => *message_id == thread || *parent_id == Some(thread),
        (ChatEvent::Close(_), _) => false,
    }
}

/// The receipt updates in `event` that belong on this viewer's screen: only
/// for this conversation, and in a DM only on messages the viewer wrote. In a
/// group or channel every member sees who read every message.
fn receipts_for(event: &ChatEvent, conversation_id: i64, viewer_id: i64) -> Vec<&ReceiptUpdate> {
    match event {
        ChatEvent::Read {
            conversation_id: id,
            receipts,
        } if *id == conversation_id => receipts
            .iter()
            .filter(|r| !r.dm || r.author_id == viewer_id)
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

/// Receipts as out-of-band swaps of each message's `#receipt-{id}` slot, and
/// of its `#tick-{id}` when the viewer wrote it (no one else's page has one).
fn render_receipts(session: &Session, updates: &[&ReceiptUpdate]) -> Result<String> {
    let mut html = String::new();
    for update in updates {
        html.push_str(&session.view.render(
            "chat/_receipt.html",
            data!({
                "message": {
                    "id": update.message_id,
                    "receipt": update.receipt,
                    "own": update.author_id == session.user_id,
                },
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
    use crate::{controllers::chat::Receipt, data::live::CloseScope, views::layout::Person};

    fn update(message_id: i64, author_id: i64, dm: bool) -> ReceiptUpdate {
        ReceiptUpdate {
            message_id,
            author_id,
            dm,
            receipt: None,
        }
    }

    fn ids(updates: &[&ReceiptUpdate]) -> Vec<i64> {
        updates.iter().map(|r| r.message_id).collect()
    }

    #[test]
    fn group_sockets_get_receipts_for_every_message() {
        let event = ChatEvent::Read {
            conversation_id: 7,
            receipts: Arc::new(vec![update(10, 1, false), update(11, 2, false)]),
        };
        assert_eq!(ids(&receipts_for(&event, 7, 1)), vec![10, 11]);
        assert_eq!(
            ids(&receipts_for(&event, 7, 3)),
            vec![10, 11],
            "not the author"
        );
        assert!(
            receipts_for(&event, 8, 1).is_empty(),
            "another conversation"
        );
    }

    #[test]
    fn dm_sockets_only_get_receipts_for_their_viewers_messages() {
        let event = ChatEvent::Read {
            conversation_id: 7,
            receipts: Arc::new(vec![
                update(10, 1, true),
                update(11, 2, true),
                update(12, 1, true),
            ]),
        };
        assert_eq!(ids(&receipts_for(&event, 7, 1)), vec![10, 12]);
        assert!(receipts_for(&event, 7, 3).is_empty(), "not the author");
    }

    fn read_by(message_id: i64, readers: &[&str]) -> ReceiptUpdate {
        ReceiptUpdate {
            message_id,
            author_id: 1,
            dm: false,
            receipt: Some(Receipt {
                dm: false,
                text: String::new(),
                readers: readers
                    .iter()
                    .map(|name| Person {
                        id: 0,
                        username: (*name).to_string(),
                        color: "#ffb454",
                    })
                    .collect(),
                shown: Vec::new(),
                more: 0,
                all: false,
            }),
        }
    }

    #[test]
    fn late_read_events_never_lower_a_count() {
        let mut sent = HashMap::new();
        let both = read_by(10, &["bob", "carol"]);
        let other = read_by(11, &["bob"]);
        assert_eq!(not_older(vec![&both, &other], &mut sent).len(), 2);

        // Bob's event arrives after carol's: message 10 keeps both readers.
        let late = read_by(10, &["bob"]);
        let newer = read_by(11, &["bob", "carol"]);
        let kept: Vec<i64> = not_older(vec![&late, &newer], &mut sent)
            .iter()
            .map(|u| u.message_id)
            .collect();
        assert_eq!(kept, vec![11]);

        // The same count again is still sent.
        assert_eq!(not_older(vec![&both], &mut sent).len(), 1);
    }

    fn message(id: i64, conversation_id: i64, parent_id: Option<i64>) -> messages::Model {
        messages::Model {
            created_at: chrono::Utc::now().into(),
            updated_at: chrono::Utc::now().into(),
            id,
            body: "hi".to_string(),
            organisation_id: 1,
            conversation_id,
            user_id: 1,
            parent_id,
        }
    }

    fn view(message: &messages::Model) -> Box<MessageView> {
        Box::new(MessageView::new(
            message,
            &HashMap::new(),
            1,
            chrono_tz::UTC,
        ))
    }

    fn message_event(conversation_id: i64) -> ChatEvent {
        ChatEvent::Message {
            conversation_id,
            author_id: 1,
            message: view(&message(10, conversation_id, None)),
        }
    }

    fn read_event(conversation_id: i64) -> ChatEvent {
        ChatEvent::Read {
            conversation_id,
            receipts: Arc::new(Vec::new()),
        }
    }

    fn reactions_event(conversation_id: i64, message_id: i64, parent_id: Option<i64>) -> ChatEvent {
        ChatEvent::Reactions {
            conversation_id,
            message_id,
            parent_id,
            rows: Arc::new(Vec::new()),
        }
    }

    fn reply_event(conversation_id: i64, parent_id: i64) -> ChatEvent {
        ChatEvent::Reply {
            conversation_id,
            parent_id,
            author_id: 1,
            message: view(&message(11, conversation_id, Some(parent_id))),
            line: Box::new(ThreadLine {
                url: String::new(),
                count: 1,
                label: "1 reply".to_string(),
                last: "10:00".to_string(),
                faces: Vec::new(),
                more: 0,
            }),
        }
    }

    fn close() -> ChatEvent {
        ChatEvent::Close(CloseScope::Account { user_id: 1 })
    }

    /// Every event type, on the conversation the socket follows.
    fn every_event(conversation_id: i64) -> Vec<(&'static str, ChatEvent)> {
        vec![
            ("message", message_event(conversation_id)),
            ("read", read_event(conversation_id)),
            (
                "reactions on a top-level message",
                reactions_event(conversation_id, 10, None),
            ),
            (
                "reactions on a reply",
                reactions_event(conversation_id, 11, Some(10)),
            ),
            ("reply", reply_event(conversation_id, 10)),
        ]
    }

    #[test]
    fn another_conversation_is_never_wanted() {
        // Conversation 8 may belong to another organisation.
        for (name, event) in every_event(8) {
            assert!(!wants(&event, 7, None), "main feed: {name}");
            assert!(!wants(&event, 7, Some(10)), "thread: {name}");
        }
    }

    #[test]
    fn main_feed_sockets_take_only_main_feed_events() {
        assert!(wants(&message_event(7), 7, None));
        assert!(wants(&read_event(7), 7, None));
        assert!(wants(&reactions_event(7, 10, None), 7, None));
        assert!(
            wants(&reply_event(7, 10), 7, None),
            "updates the reply line"
        );
        assert!(
            !wants(&reactions_event(7, 11, Some(10)), 7, None),
            "a reply's reactions"
        );
        // Closes are matched on the person before this check.
        assert!(!wants(&close(), 7, None));
    }

    #[test]
    fn thread_sockets_take_only_their_own_threads_events() {
        let thread = Some(10);
        assert!(wants(&reply_event(7, 10), 7, thread));
        assert!(
            wants(&reactions_event(7, 10, None), 7, thread),
            "the parent's reactions"
        );
        assert!(
            wants(&reactions_event(7, 11, Some(10)), 7, thread),
            "a reply's reactions"
        );
        assert!(!wants(&reply_event(7, 20), 7, thread), "another thread");
        assert!(
            !wants(&reactions_event(7, 20, None), 7, thread),
            "another top-level message"
        );
        assert!(
            !wants(&reactions_event(7, 21, Some(20)), 7, thread),
            "another thread's reply"
        );
        assert!(!wants(&message_event(7), 7, thread), "main-feed message");
        assert!(!wants(&read_event(7), 7, thread), "receipts");
        assert!(!wants(&close(), 7, thread));
    }

    #[test]
    fn late_reply_events_never_lower_a_count() {
        let mut sent = HashMap::new();
        assert!(grows(&mut sent, 10, 2));
        assert!(!grows(&mut sent, 10, 1), "a late event");
        assert!(grows(&mut sent, 10, 2), "the same count again");
        assert!(grows(&mut sent, 10, 3));
        assert!(grows(&mut sent, 20, 1), "counted per thread");
    }

    #[test]
    fn new_messages_carry_no_receipts() {
        let event = ChatEvent::Message {
            conversation_id: 7,
            author_id: 1,
            message: view(&message(10, 7, None)),
        };
        assert!(receipts_for(&event, 7, 1).is_empty());
    }
}
