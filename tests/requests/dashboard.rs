use collab::app::App;
use loco_rs::testing::prelude::*;
use serial_test::serial;

use super::prepare_data::{sign_up, TENANT_PAGES};

#[tokio::test]
#[serial]
async fn anonymous_visitors_are_sent_to_login() {
    request::<App, _, _>(|request, _ctx| async move {
        for page in TENANT_PAGES {
            let res = request.get(page).await;
            assert_eq!(res.status_code(), 303, "{page}");
            assert_eq!(res.headers().get("location").unwrap(), "/login", "{page}");
        }
        let htmx = request.get("/tasks").add_header("HX-Request", "true").await;
        assert_eq!(htmx.headers().get("HX-Redirect").unwrap(), "/login");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn active_members_reach_every_page_with_their_org_in_the_header() {
    request::<App, _, _>(|request, _ctx| async move {
        let cookie = sign_up(&request, "Himalayan Ritual", "raj", "raj@example.com").await;
        for page in TENANT_PAGES {
            let res = request
                .get(page)
                .add_header(cookie.0.clone(), cookie.1.clone())
                .await;
            assert_eq!(res.status_code(), 200, "{page}");
            let body = res.text();
            assert!(body.contains("Himalayan Ritual"), "{page}");
            assert!(body.contains("raj"), "{page}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_bad_session_cookie_is_treated_as_signed_out() {
    request::<App, _, _>(|request, _ctx| async move {
        let res = request
            .get("/dashboard")
            .add_header("cookie", "auth_token=not-a-jwt")
            .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(res.headers().get("location").unwrap(), "/login");
    })
    .await;
}
