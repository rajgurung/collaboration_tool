//! In-memory fan-out for live chat. One process holds every subscriber, so the
//! app must run as a single instance (see docs/deploy.md).
use std::sync::Arc;

use tokio::sync::broadcast;

use crate::controllers::chat::{MessageView, ReactionRow, Receipt, ThreadLine};

/// Enough buffered events for a burst; slower sockets skip ahead and the
/// browser reloads the feed after reconnecting.
const CAPACITY: usize = 512;

/// Something that happened in a conversation.
#[derive(Debug, Clone)]
pub enum ChatEvent {
    /// A new top-level message. `message.own` is false; each socket sets it
    /// for its own viewer.
    Message {
        conversation_id: i64,
        author_id: i64,
        message: Box<MessageView>,
    },
    /// Someone read up to now. Carries the new receipts for the messages that
    /// read covered; each socket forwards only those its viewer wrote. Shared,
    /// since every subscriber gets its own copy of the event.
    Read {
        conversation_id: i64,
        receipts: Arc<Vec<ReceiptUpdate>>,
    },
    /// Someone added or removed a reaction. Carries the message's full set,
    /// read after the change, so each socket renders it for its own viewer
    /// without a query. The last event wins.
    Reactions {
        conversation_id: i64,
        message_id: i64,
        /// The reacted message's parent, when it is a thread reply.
        parent_id: Option<i64>,
        rows: Arc<Vec<ReactionRow>>,
    },
    /// A new thread reply. Main-feed sockets update the parent's reply line;
    /// sockets on that thread append the reply. `message.own` is false, as
    /// for `Message`.
    Reply {
        conversation_id: i64,
        parent_id: i64,
        author_id: i64,
        message: Box<MessageView>,
        line: Box<ThreadLine>,
    },
    /// The user's sessions were ended (sign out everywhere, password reset,
    /// removal). Their open sockets close.
    SignedOut { user_id: i64 },
}

/// The current receipt for one message, for its author's screen.
#[derive(Debug, Clone)]
pub struct ReceiptUpdate {
    pub message_id: i64,
    pub author_id: i64,
    pub receipt: Option<Receipt>,
}

#[derive(Clone)]
pub struct ChatHub {
    tx: broadcast::Sender<ChatEvent>,
}

impl Default for ChatHub {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatHub {
    #[must_use]
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(CAPACITY);
        Self { tx }
    }

    /// Sends to every open socket. Having no listeners is fine.
    pub fn publish(&self, event: ChatEvent) {
        let _ = self.tx.send(event);
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<ChatEvent> {
        self.tx.subscribe()
    }
}
