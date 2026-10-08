use collab::{
    app::App,
    models::{conversations, messages, organisations},
};
use loco_rs::testing::prelude::*;
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait, IntoActiveModel};
use serial_test::serial;

use super::{prepare_data::sign_up, tasks::approved_member};

#[tokio::test]
#[serial]
async fn organisations_read_times_in_their_own_zone() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let org = organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap();
        assert_eq!(org.timezone, "Europe/London", "London by default");

        // A message sent at 17:00 UTC on a summer-time day.
        let general = conversations::Model::find_general(&ctx.db, org.id)
            .await
            .unwrap();
        request
            .post(&format!("/chat/{}/messages", general.id))
            .add_header(alice.0.clone(), alice.1.clone())
            .form(&serde_json::json!({ "body": "Stand-up notes are in" }))
            .await;
        let message = messages::Entity::find()
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        let mut active = message.into_active_model();
        active.created_at =
            ActiveValue::Set(chrono::DateTime::parse_from_rfc3339("2026-10-08T17:00:00Z").unwrap());
        active.update(&ctx.db).await.unwrap();

        let chat = |cookie: (axum::http::HeaderName, axum::http::HeaderValue)| {
            let request = &request;
            let path = format!("/chat/{}", general.id);
            async move {
                request
                    .get(&path)
                    .add_header(cookie.0, cookie.1)
                    .await
                    .text()
            }
        };
        assert!(chat(alice.clone()).await.contains("18:00"), "BST is UTC+1");

        let res = request
            .post("/members/timezone")
            .add_header(alice.0.clone(), alice.1.clone())
            .form(&serde_json::json!({ "timezone": "America/New_York" }))
            .await;
        assert_eq!(res.status_code(), 303);
        assert!(
            chat(alice.clone()).await.contains("13:00"),
            "New York is UTC-4 then"
        );

        let stored = messages::Entity::find()
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.created_at.to_rfc3339(),
            "2026-10-08T17:00:00+00:00",
            "still stored in UTC"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn only_owners_and_admins_change_the_zone() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let (bob, _) = approved_member(&request, &ctx, &alice, "bob", "bob@example.com").await;

        let page = request
            .get("/members")
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(page.contains("Time zone") && page.contains(r#"value="Europe/London" selected"#));
        let bobs = request
            .get("/members")
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(
            !bobs.contains("/members/timezone"),
            "members don't see the setting"
        );

        let res = request
            .post("/members/timezone")
            .add_header(bob.0, bob.1)
            .form(&serde_json::json!({ "timezone": "Asia/Kathmandu" }))
            .await;
        assert_eq!(res.status_code(), 403);

        let res = request
            .post("/members/timezone")
            .add_header(alice.0, alice.1)
            .add_header("HX-Request", "true")
            .form(&serde_json::json!({ "timezone": "Mars/Olympus" }))
            .await;
        assert_eq!(res.status_code(), 422);
        let org = organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap();
        assert_eq!(org.timezone, "Europe/London");
    })
    .await;
}
