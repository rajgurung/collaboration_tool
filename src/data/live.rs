//! Which open chat sockets and notification streams an event closes.
//!
//! Close events, like the hubs that carry them, live in memory, so the app
//! must run as a single instance (see docs/deploy.md). Every long-lived
//! connection must handle every scope: a new kind of socket that ignored
//! `Org` would keep serving an organisation after its person was removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseScope {
    /// Sign out everywhere, a password reset or a lock: every socket of the
    /// user, in every organisation.
    Account { user_id: i64 },
    /// Removal from an organisation: only the user's sockets bound to it,
    /// on every device.
    Org { org_id: i64, user_id: i64 },
}

impl CloseScope {
    /// Whether this event closes a socket bound to `org_id`, opened by
    /// `user_id`. Each tab's socket stays in its own org, so nothing closes
    /// them on a switch.
    #[must_use]
    pub fn closes(&self, org_id: i64, user_id: i64) -> bool {
        match self {
            Self::Account { user_id: id } => *id == user_id,
            Self::Org {
                org_id: org,
                user_id: id,
            } => *org == org_id && *id == user_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_scope_closes_every_socket_of_that_user() {
        let scope = CloseScope::Account { user_id: 7 };
        assert!(scope.closes(1, 7));
        assert!(scope.closes(2, 7));
        assert!(!scope.closes(1, 8), "another user");
    }

    #[test]
    fn org_scope_closes_only_that_orgs_sockets() {
        let scope = CloseScope::Org {
            org_id: 2,
            user_id: 7,
        };
        assert!(scope.closes(2, 7));
        assert!(!scope.closes(1, 7), "another org");
        assert!(!scope.closes(2, 8), "another user");
    }
}
