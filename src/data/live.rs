//! Which open chat sockets and notification streams an event closes.
//!
//! Close events, like the hubs that carry them, live in memory, so the app
//! must run as a single instance (see docs/deploy.md). Every long-lived
//! connection must handle every scope: a new kind of socket that ignored
//! `Org` would keep serving an organisation after its person was removed.
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseScope {
    /// Sign out everywhere, a password reset or a lock: every socket of the
    /// user, in every organisation.
    Account { user_id: i64 },
    /// Removal from an organisation: only the user's sockets bound to it,
    /// on every device.
    Org { org_id: i64, user_id: i64 },
    /// A switch or a create in one browser: only that sign-in's sockets.
    /// The sign-in id only ever comes from the request's `SessionId`, never
    /// from the browser.
    Browser { user_id: i64, session_id: Arc<str> },
}

impl CloseScope {
    /// Whether this event closes a socket bound to `org_id`, opened by
    /// `user_id` on the sign-in `session_id`. A socket with no sign-in id
    /// (a cookie from before sign-in ids) is closed by a switch, as the user
    /// id must still match.
    #[must_use]
    pub fn closes(&self, org_id: i64, user_id: i64, session_id: Option<&str>) -> bool {
        match self {
            Self::Account { user_id: id } => *id == user_id,
            Self::Org {
                org_id: org,
                user_id: id,
            } => *org == org_id && *id == user_id,
            Self::Browser {
                user_id: id,
                session_id: sid,
            } => *id == user_id && session_id.is_none_or(|s| s == &**sid),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn browser(user_id: i64, sid: &str) -> CloseScope {
        CloseScope::Browser {
            user_id,
            session_id: Arc::from(sid),
        }
    }

    #[test]
    fn account_scope_closes_every_socket_of_that_user() {
        let scope = CloseScope::Account { user_id: 7 };
        assert!(scope.closes(1, 7, Some("a")));
        assert!(scope.closes(2, 7, Some("b")));
        assert!(scope.closes(3, 7, None));
        assert!(!scope.closes(1, 8, Some("a")), "another user");
    }

    #[test]
    fn org_scope_closes_only_that_orgs_sockets() {
        let scope = CloseScope::Org {
            org_id: 2,
            user_id: 7,
        };
        assert!(scope.closes(2, 7, Some("a")));
        assert!(scope.closes(2, 7, None), "every device");
        assert!(!scope.closes(1, 7, Some("a")), "another org");
        assert!(!scope.closes(2, 8, Some("a")), "another user");
    }

    #[test]
    fn browser_scope_closes_only_that_sign_in() {
        let scope = browser(7, "a");
        assert!(scope.closes(1, 7, Some("a")));
        assert!(scope.closes(2, 7, Some("a")), "any org");
        assert!(!scope.closes(1, 7, Some("b")), "another sign-in");
        assert!(scope.closes(1, 7, None), "a socket with no sign-in id");
    }

    #[test]
    fn browser_scope_never_closes_another_user() {
        let scope = browser(7, "a");
        assert!(!scope.closes(1, 8, Some("a")));
        assert!(!scope.closes(1, 8, None));
    }
}
