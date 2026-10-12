use collab::app::App;
use loco_rs::testing::prelude::*;
use serial_test::serial;

/// The server-side rendered root page.
///
/// This asserts the whole path end to end — route, `ViewEngine` extractor,
/// Tera lookup, i18n function — rather than just a 200. `tests/views` renders
/// the same template directly, but nothing connected it to a route, which is
/// exactly how the starter came to ship a template that no request could
/// reach.
#[tokio::test]
#[serial]
async fn can_get_home_page() {
    request::<App, _, _>(|request, _ctx| async move {
        let res = request.get("/").await;

        assert_eq!(res.status_code(), 200);
        let body = res.text();
        assert!(
            body.contains("Every promise has an"),
            "expected the rendered view, got: {body}"
        );
        assert!(body.contains(r#"href="/signup""#), "{body}");
        assert!(body.contains(r#"href="/login""#), "{body}");
        // The full landing page, not the old card in the auth layout.
        assert!(!body.contains("auth-card"), "{body}");
    })
    .await;
}

/// The Documents row is on the homepage and linked from the nav. It must not
/// promise anything about logins or passwords, which are not built.
#[tokio::test]
#[serial]
async fn home_page_shows_documents() {
    request::<App, _, _>(|request, _ctx| async move {
        let res = request.get("/").await;

        assert_eq!(res.status_code(), 200);
        let body = res.text();
        assert!(body.contains(r#"id="documents""#), "{body}");
        assert!(body.contains(r##"href="#documents""##), "{body}");
        assert!(body.contains("Company papers, one tap away."), "{body}");
        // The Documents section claims only what is live: no logins yet.
        let start = body.find(r#"id="documents""#).expect("documents section");
        let section = &body[start..];
        let section = section[..section.find("</section>").expect("section end")].to_lowercase();
        for banned in ["password", "login", "encrypt", "attach"] {
            assert!(!section.contains(banned), "{banned} in: {section}");
        }
    })
    .await;
}
