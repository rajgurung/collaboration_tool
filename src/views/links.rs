//! Addresses inside an organisation: `/o/{slug}/tasks/58`. Every app link is
//! built here, so a link can never name a slug that isn't one.

/// The longest slug, in bytes.
pub const SLUG_MAX: usize = 60;

/// 1 to [`SLUG_MAX`] bytes, each `a`-`z`, `0`-`9` or `-`. Checked byte by
/// byte on the raw value: no decoding and no lowercasing, so anything
/// outside ASCII fails.
#[must_use]
pub fn is_slug(s: &str) -> bool {
    (1..=SLUG_MAX).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The first path segment of every route that works inside an organisation.
/// Only these are reachable under `/o/{slug}/`.
pub const TENANT_SECTIONS: [&str; 12] = [
    "dashboard",
    "tasks",
    "chat",
    "documents",
    "members",
    "more",
    "notifications",
    "roadmap",
    "meetings",
    "settings",
    "guide",
    "orgs",
];

/// A row an old bare link can name, for finding which org it belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Task,
    SavedView,
    Conversation,
    Folder,
    Document,
    Login,
    Membership,
    Project,
}

impl Item {
    /// The item's table. Fixed here, never taken from the request.
    #[must_use]
    pub const fn table(self) -> &'static str {
        match self {
            Self::Task => "tasks",
            Self::SavedView => "saved_views",
            Self::Conversation => "conversations",
            Self::Folder => "folders",
            Self::Document => "documents",
            Self::Login => "logins",
            Self::Membership => "memberships",
            Self::Project => "projects",
        }
    }
}

/// Which item an old bare page address names, if any: `/tasks/58`,
/// `/tasks?open=58`, `/chat/12/threads/9` and the rest of the page loads
/// that carry an id. A pure parse, no database.
#[must_use]
pub fn old_link_item(path: &str, query: Option<&str>) -> Option<(Item, i64)> {
    let id = |s: &str| s.parse::<i64>().ok().filter(|id| *id > 0);
    let param = |name: &str| {
        url::form_urlencoded::parse(query.unwrap_or_default().as_bytes())
            .find(|(key, _)| key == name)
            .and_then(|(_, value)| id(&value))
    };
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match segments.as_slice() {
        ["tasks"] => param("open").map(|n| (Item::Task, n)),
        ["tasks", n] | ["tasks", n, "edit"] => id(n).map(|n| (Item::Task, n)),
        ["tasks", "views", n, "edit"] => id(n).map(|n| (Item::SavedView, n)),
        ["chat"] => param("c").map(|n| (Item::Conversation, n)),
        ["chat", n] | ["chat", n, "threads", _] => id(n).map(|n| (Item::Conversation, n)),
        ["documents", "folders", n] => id(n).map(|n| (Item::Folder, n)),
        ["documents", "files", n] => id(n).map(|n| (Item::Document, n)),
        ["documents", "logins", n] => id(n).map(|n| (Item::Login, n)),
        ["members", n, "rotate"] => id(n).map(|n| (Item::Membership, n)),
        ["roadmap", "projects", n, "edit"] => id(n).map(|n| (Item::Project, n)),
        _ => None,
    }
}

/// The list a page's `active` key belongs to, with no query, for the
/// switcher's "same section in the other org" and the in-org 404's way back.
/// Every one of them opens for every role. Anything unknown is Home.
#[must_use]
pub fn section_path(active: &str) -> &'static str {
    match active {
        "notifications" => "/notifications",
        "tasks" => "/tasks",
        "chat" => "/chat",
        "roadmap" => "/roadmap",
        "meetings" => "/meetings",
        "documents" => "/documents",
        "members" => "/members",
        "more" => "/more",
        "claude" => "/settings/claude",
        _ => "/dashboard",
    }
}

/// The `active` key and name of the section a path is in, for the in-org
/// 404. `None` for Home and for paths with no list of their own.
#[must_use]
pub fn section_of(path: &str) -> Option<(&'static str, &'static str)> {
    let first = path.trim_start_matches('/').split(['/', '?']).next()?;
    Some(match first {
        "notifications" => ("notifications", "Notifications"),
        "tasks" => ("tasks", "Tasks"),
        "chat" => ("chat", "Chat"),
        "roadmap" => ("roadmap", "Roadmap"),
        "meetings" => ("meetings", "Meetings"),
        "documents" => ("documents", "Documents"),
        "members" => ("members", "Members"),
        "more" => ("more", "More"),
        "settings" => ("claude", "Connect Claude"),
        _ => return None,
    })
}

/// `/o/{slug}{path}`, for a path inside the organisation such as `/tasks/58`.
/// Fails closed: a bad slug, or a path that doesn't start with one `/` or
/// already names an organisation, gives a bare `/dashboard` and an error in
/// the logs, never a link built from it.
#[must_use]
pub fn org_path(slug: &str, path: &str) -> String {
    if is_slug(slug) && is_org_relative(path) {
        format!("/o/{slug}{path}")
    } else {
        tracing::error!(
            slug_ok = is_slug(slug),
            path_ok = is_org_relative(path),
            "refused to build an organisation link"
        );
        "/dashboard".to_string()
    }
}

/// `/o/{slug}`, the base templates' `org_url` adds paths to. Empty, with an
/// error in the logs, for a bad slug, so links fall back to bare addresses.
#[must_use]
pub fn org_base(slug: &str) -> String {
    if is_slug(slug) {
        format!("/o/{slug}")
    } else {
        tracing::error!("refused to build an organisation base");
        String::new()
    }
}

/// Starts with exactly one `/`, and isn't already under `/o/`.
#[must_use]
pub fn is_org_relative(path: &str) -> bool {
    path.starts_with('/') && !path.starts_with("//") && !path.starts_with("/o/") && path != "/o"
}

#[cfg(test)]
mod tests {
    use super::{
        is_slug, old_link_item, org_base, org_path, section_of, section_path, Item, SLUG_MAX,
    };

    #[test]
    fn slugs_are_1_to_60_safe_bytes() {
        assert!(!is_slug(""));
        assert!(is_slug("a"));
        assert!(is_slug(&"a".repeat(SLUG_MAX)));
        assert!(!is_slug(&"a".repeat(SLUG_MAX + 1)));
        assert!(is_slug("acme-studio-2"));
        for bad in [
            "Acme",
            "acme studio",
            "acme_studio",
            "acme.studio",
            "acme/x",
            "acme%2F",
            "café",
            "a\n",
            "a?b",
            "a#b",
        ] {
            assert!(!is_slug(bad), "{bad:?}");
        }
    }

    #[test]
    fn org_path_prefixes_a_path_inside_the_org() {
        assert_eq!(org_path("acme", "/tasks"), "/o/acme/tasks");
        assert_eq!(org_path("acme", "/tasks?open=58"), "/o/acme/tasks?open=58");
        assert_eq!(org_path("acme", "/"), "/o/acme/");
    }

    #[test]
    fn old_links_name_their_item() {
        let cases = [
            ("/tasks/58", None, Some((Item::Task, 58))),
            ("/tasks/58/edit", None, Some((Item::Task, 58))),
            ("/tasks", Some("open=58"), Some((Item::Task, 58))),
            ("/tasks", Some("view=board&open=58"), Some((Item::Task, 58))),
            ("/tasks/views/3/edit", None, Some((Item::SavedView, 3))),
            ("/chat/12", None, Some((Item::Conversation, 12))),
            ("/chat", Some("c=12"), Some((Item::Conversation, 12))),
            ("/chat/12/threads/9", None, Some((Item::Conversation, 12))),
            ("/documents/folders/7", None, Some((Item::Folder, 7))),
            ("/documents/files/8", None, Some((Item::Document, 8))),
            ("/documents/logins/4", None, Some((Item::Login, 4))),
            ("/members/5/rotate", None, Some((Item::Membership, 5))),
            ("/roadmap/projects/6/edit", None, Some((Item::Project, 6))),
            ("/tasks", None, None),
            ("/tasks", Some("open=x"), None),
            ("/tasks/0", None, None),
            ("/tasks/-1", None, None),
            ("/tasks/new", None, None),
            ("/chat/12/feed", None, None),
            ("/chat/12/ws", None, None),
            ("/roadmap", Some("lane=now"), None),
            ("/notifications", None, None),
            ("/chat/%2F%2Fevil.com", None, None),
        ];
        for (path, query, want) in cases {
            assert_eq!(old_link_item(path, query), want, "{path} {query:?}");
        }
    }

    #[test]
    fn sections_map_to_fixed_lists() {
        assert_eq!(section_path("tasks"), "/tasks");
        assert_eq!(section_path("claude"), "/settings/claude");
        for other in ["dashboard", "orgs", "not_found", "../../evil", ""] {
            assert_eq!(section_path(other), "/dashboard", "{other}");
        }
        assert_eq!(section_of("/tasks/913"), Some(("tasks", "Tasks")));
        assert_eq!(section_of("/chat?c=1"), Some(("chat", "Chat")));
        assert_eq!(
            section_of("/settings/claude"),
            Some(("claude", "Connect Claude"))
        );
        assert_eq!(section_of("/dashboard"), None);
        assert_eq!(section_of("/orgs/9"), None);
    }

    #[test]
    fn org_base_is_empty_for_a_bad_slug() {
        assert_eq!(org_base("acme"), "/o/acme");
        assert_eq!(org_base("Acme"), "");
    }

    #[test]
    fn org_path_fails_closed() {
        for (slug, path) in [
            ("Acme", "/tasks"),
            ("", "/tasks"),
            ("acme", "tasks"),
            ("acme", ""),
            ("acme", "//evil.com"),
            ("acme", "/o/beta/tasks"),
            ("acme", "/o"),
            ("acme", "https://evil.com"),
        ] {
            assert_eq!(org_path(slug, path), "/dashboard", "{slug:?} {path:?}");
        }
    }

    #[test]
    fn join_pages_use_the_same_slug_shape() {
        let longest = format!("/join/{}", "a".repeat(SLUG_MAX));
        assert!(crate::controllers::auth::join_page(&longest).is_some());
        let too_long = format!("/join/{}", "a".repeat(SLUG_MAX + 1));
        assert!(crate::controllers::auth::join_page(&too_long).is_none());
    }
}
