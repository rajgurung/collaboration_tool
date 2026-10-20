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
    use super::{is_slug, org_base, org_path, section_of, section_path, SLUG_MAX};

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
