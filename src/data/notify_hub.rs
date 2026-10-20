//! In-memory fan-out that tells open pages someone has new notifications.
//! Like the chat hub, it needs the app to run as a single instance.
use tokio::sync::broadcast;

use super::live::CloseScope;

const CAPACITY: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotifyEvent {
    /// This user has new notifications in this organisation.
    Changed { org_id: i64, user_id: i64 },
    /// Some of a person's streams close, as for chat sockets (see
    /// [`CloseScope`]).
    Close(CloseScope),
}

#[derive(Clone)]
pub struct NotifyHub {
    tx: broadcast::Sender<NotifyEvent>,
}

impl Default for NotifyHub {
    fn default() -> Self {
        Self::new()
    }
}

impl NotifyHub {
    #[must_use]
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(CAPACITY);
        Self { tx }
    }

    /// Tells these users' open pages in this organisation to refresh their
    /// bell.
    pub fn publish(&self, org_id: i64, user_ids: &[i64]) {
        for id in user_ids {
            let _ = self.tx.send(NotifyEvent::Changed {
                org_id,
                user_id: *id,
            });
        }
    }

    /// Closes the streams this scope matches.
    pub fn close(&self, scope: CloseScope) {
        let _ = self.tx.send(NotifyEvent::Close(scope));
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<NotifyEvent> {
        self.tx.subscribe()
    }
}
