use collab::app::App;
use loco_rs::testing::prelude::*;
use serial_test::serial;

use super::prepare_data::{join, sign_up};

#[tokio::test]
#[serial]
async fn more_lists_the_other_sections_and_pending_count() {
    request::<App, _, _>(|request, _ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        join(&request, "acme", "bob", "bob@example.com").await;
        let res = request.get("/more").add_header(owner.0, owner.1).await;
        assert_eq!(res.status_code(), 200);
        let body = res.text();
        for link in ["/roadmap", "/meetings", "/members", "/logout"] {
            assert!(body.contains(link), "{link}");
        }
        assert!(body.contains("1 waiting"));
        // Appearance: System, Light or Dark, applied before the page draws.
        for choice in ["system", "light", "dark"] {
            assert!(
                body.contains(&format!(r#"data-theme-choice="{choice}""#)),
                "{choice}"
            );
        }
        assert!(body.contains(r#"localStorage.getItem("theme")"#));
    })
    .await;
}
