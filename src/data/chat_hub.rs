//! In-memory fan-out for live chat. One process holds every subscriber, so the
//! app must run as a single instance (see docs/deploy.md).
use tokio::sync::broadcast;

use crate::controllers::chat::MessageView;

/// Enough buffered events for a burst; slower sockets skip ahead and the
/// browser reloads the feed after reconnecting.
const CAPACITY: usize = 512;

/// A new message in a conversation. `message.own` is false; each socket sets it
/// for its own viewer.
#[derive(Debug, Clone)]
pub struct ChatEvent {
    pub conversation_id: i64,
    pub author_id: i64,
    pub message: MessageView,
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
