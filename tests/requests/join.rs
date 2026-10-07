use collab::{
    app::App,
    models::{memberships, users},
};
use loco_rs::testing::prelude::*;
use serial_test::serial;

use super::prepare_data::{join, sign_up, TENANT_PAGES, USER_PASSWORD};

#[tokio::test]
#[serial]
async fn join_page_shows_the_organisation() {
    request::<App, _, _>(|request, _ctx| async move {
        sign_up(&request, "Himalayan Ritual", "raj", "raj@example.com").await;
        let res = request.get("/join/himalayan-ritual").await;
        assert_eq!(res.status_code(), 200);
        assert!(res.text().contains("Himalayan Ritual"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn unknown_join_link_is_404() {
    request::<App, _, _>(|request, _ctx| async move {
        assert_eq!(request.get("/join/nope").await.status_code(), 404);
        let res = request
            .post("/join/nope")
            .form(&serde_json::json!({ "name": "sam", "email": "sam@example.com", "password": USER_PASSWORD }))
            .await;
        assert_eq!(res.status_code(), 404);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn joining_leaves_the_user_pending_on_every_page() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up(&request, "Himalayan Ritual", "raj", "raj@example.com").await;
        let cookie = join(&request, "himalayan-ritual", "maya", "maya@example.com").await;

        let user = users::Model::find_by_email(&ctx.db, "maya@example.com")
            .await
            .unwrap();
        let membership = memberships::Model::find_for_user(&ctx.db, user.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(membership.status, "pending");
        assert_eq!(membership.role, "member");

        for page in TENANT_PAGES {
            let res = request
                .get(page)
                .add_header(cookie.0.clone(), cookie.1.clone())
                .await;
            assert_eq!(res.status_code(), 403, "{page}");
            assert!(res.text().contains("Waiting for approval"), "{page}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn rejected_members_see_the_declined_page() {
    request::<App, _, _>(|request, ctx| async move {
        use sea_orm::{ActiveModelTrait, ActiveValue, IntoActiveModel};

        sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let cookie = join(&request, "acme", "bob", "bob@example.com").await;
        let bob = users::Model::find_by_email(&ctx.db, "bob@example.com")
            .await
            .unwrap();
        let mut membership = memberships::Model::find_for_user(&ctx.db, bob.id)
            .await
            .unwrap()
            .unwrap()
            .into_active_model();
        membership.status = ActiveValue::Set("rejected".to_string());
        membership.update(&ctx.db).await.unwrap();

        let res = request
            .get("/dashboard")
            .add_header(cookie.0, cookie.1)
            .await;
        assert_eq!(res.status_code(), 403);
        assert!(res.text().contains("Request declined"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn usernames_are_unique_per_organisation_only() {
    request::<App, _, _>(|request, _ctx| async move {
        sign_up(&request, "Acme", "alice", "alice@example.com").await;
        sign_up(&request, "Globex", "gina", "gina@example.com").await;

        let clash = request
            .post("/join/acme")
            .form(&serde_json::json!({ "name": "ALICE", "email": "other@example.com", "password": USER_PASSWORD }))
            .await;
        assert_eq!(clash.status_code(), 422);
        assert!(clash.text().contains("already used in this organisation"));

        // The same name is fine in a different organisation.
        join(&request, "globex", "alice", "alice2@example.com").await;
    })
    .await;
}

#[tokio::test]
#[serial]
async fn signed_in_users_skip_the_join_and_signup_forms() {
    request::<App, _, _>(|request, _ctx| async move {
        let cookie = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        for page in ["/join/acme", "/signup", "/"] {
            let res = request
                .get(page)
                .add_header(cookie.0.clone(), cookie.1.clone())
                .await;
            assert_eq!(res.status_code(), 303, "{page}");
            assert_eq!(
                res.headers().get("location").unwrap(),
                "/dashboard",
                "{page}"
            );
        }
    })
    .await;
}
