//! In-memory fan-out for live chat. One process holds every subscriber, so the
//! app must run as a single instance (see docs/deploy.md).
use tokio::sync::broadcast;

use crate::controllers::chat::{MessageView, Receipt};

/// Enough buffered events for a burst; slower sockets skip ahead and the
/// browser reloads the feed after reconnecting.
const CAPACITY: usize = 512;

/// Something that happened in a conversation.
#[derive(Debug, Clone)]
pub enum ChatEvent {
    /// A new message. `message.own` is false; each socket sets it for its own viewer.
    Message {
        conversation_id: i64,
        author_id: i64,
        message: Box<MessageView>,
    },
    /// Someone read up to now. Carries the new receipts for the messages that
    /// read covered; each socket forwards only those its viewer wrote.
    Read {
        conversation_id: i64,
        receipts: Vec<ReceiptUpdate>,
    },
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
