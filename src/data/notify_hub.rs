//! In-memory fan-out that tells open pages someone has new notifications.
//! Like the chat hub, it needs the app to run as a single instance.
use tokio::sync::broadcast;

const CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyEvent {
    /// This user has new notifications.
    Changed(i64),
    /// This user's sessions were ended, or they switched organisation;
    /// their open streams close.
    SignedOut(i64),
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

    /// Tells every open page of these users to refresh their bell.
    pub fn publish(&self, user_ids: &[i64]) {
        for id in user_ids {
            let _ = self.tx.send(NotifyEvent::Changed(*id));
        }
    }

    /// Closes every open stream of this user.
    pub fn sign_out(&self, user_id: i64) {
        let _ = self.tx.send(NotifyEvent::SignedOut(user_id));
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<NotifyEvent> {
        self.tx.subscribe()
    }
}
