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
        && s
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
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

/// Starts with exactly one `/`, and isn't already under `/o/`.
fn is_org_relative(path: &str) -> bool {
    path.starts_with('/') && !path.starts_with("//") && !path.starts_with("/o/") && path != "/o"
}

#[cfg(test)]
mod tests {
    use super::{is_slug, org_path, SLUG_MAX};

    #[test]
    fn slugs_are_1_to_60_safe_bytes() {
        assert!(!is_slug(""));
        assert!(is_slug("a"));
        assert!(is_slug(&"a".repeat(SLUG_MAX)));
        assert!(!is_slug(&"a".repeat(SLUG_MAX + 1)));
        assert!(is_slug("acme-studio-2"));
        for bad in [
            "Acme", "acme studio", "acme_studio", "acme.studio", "acme/x", "acme%2F", "café", "a\n",
            "a?b", "a#b",
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
