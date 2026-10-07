use collab::{
    app::App,
    models::{conversations, memberships, organisations, users},
};
use loco_rs::testing::prelude::*;
use sea_orm::{EntityTrait, PaginatorTrait};
use serial_test::serial;

use super::prepare_data::{sign_up, USER_PASSWORD};

#[tokio::test]
#[serial]
async fn signup_page_renders() {
    request::<App, _, _>(|request, _ctx| async move {
        let res = request.get("/signup").await;
        assert_eq!(res.status_code(), 200);
        assert!(res.text().contains(r#"name="organisation_name""#));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn signup_creates_owner_org_and_general_channel() {
    request::<App, _, _>(|request, ctx| async move {
        let cookie = sign_up(&request, "Himalayan Ritual", "raj", "Raj@Example.com").await;

        let user = users::Model::find_by_email(&ctx.db, "raj@example.com")
            .await
            .unwrap();
        let org = organisations::Model::find_by_slug(&ctx.db, "himalayan-ritual")
            .await
            .unwrap();
        assert_eq!(org.name, "Himalayan Ritual");
        assert_eq!(org.created_by_id, Some(user.id));

        let membership = memberships::Model::find_for_user(&ctx.db, user.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(membership.organisation_id, org.id);
        assert_eq!(membership.role, "owner");
        assert_eq!(membership.status, "active");
        assert_eq!(membership.username, "raj");

        let general = conversations::Model::find_general(&ctx.db, org.id)
            .await
            .unwrap();
        assert_eq!(general.kind, "channel");

        let dashboard = request
            .get("/dashboard")
            .add_header(cookie.0, cookie.1)
            .await;
        assert_eq!(dashboard.status_code(), 200);
        assert!(dashboard.text().contains("Himalayan Ritual"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn same_org_name_gets_a_unique_slug() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up(&request, "Acme", "alice", "alice@example.com").await;
        sign_up(&request, "Acme", "bob", "bob@example.com").await;
        assert!(organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .is_ok());
        assert!(organisations::Model::find_by_slug(&ctx.db, "acme-2")
            .await
            .is_ok());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn signup_shows_field_errors_and_saves_nothing() {
    request::<App, _, _>(|request, ctx| async move {
        let res = request
            .post("/signup")
            .form(&serde_json::json!({
                "organisation_name": "A",
                "name": "1bad name",
                "email": "not-an-email",
                "password": "short",
            }))
            .await;
        assert_eq!(res.status_code(), 422);
        let body = res.text();
        assert!(body.contains("Use 2 to 80 characters."), "{body}");
        assert!(body.contains("Start with a letter"), "{body}");
        assert!(body.contains("Enter a valid email address."), "{body}");
        assert!(body.contains("Use at least 8 characters."), "{body}");
        assert!(
            !body.contains("short"),
            "the password must not be echoed back"
        );

        assert_eq!(users::Entity::find().count(&ctx.db).await.unwrap(), 0);
        assert_eq!(
            organisations::Entity::find().count(&ctx.db).await.unwrap(),
            0
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn signup_with_existing_email_fails_cleanly() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let res = request
            .post("/signup")
            .form(&serde_json::json!({
                "organisation_name": "Other",
                "name": "alice2",
                "email": "ALICE@example.com",
                "password": USER_PASSWORD,
            }))
            .await;
        assert_eq!(res.status_code(), 422);
        assert!(res.text().contains("already exists"));
        assert_eq!(
            organisations::Entity::find().count(&ctx.db).await.unwrap(),
            1
        );
    })
    .await;
}
