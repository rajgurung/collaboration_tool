//! In-memory fan-out that tells open pages someone has new notifications.
//! Like the chat hub, it needs the app to run as a single instance.
use tokio::sync::broadcast;

const CAPACITY: usize = 256;

#[derive(Clone)]
pub struct NotifyHub {
    tx: broadcast::Sender<i64>,
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
            let _ = self.tx.send(*id);
        }
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<i64> {
        self.tx.subscribe()
    }
}
