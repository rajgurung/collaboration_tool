use collab::{app::App, models::users};
use loco_rs::{boot::run_task, task, testing::prelude::*};
use serial_test::serial;

use super::prepare_data::{login, sign_up};

#[tokio::test]
#[serial]
async fn admin_is_hidden_from_normal_users() {
    request::<App, _, _>(|request, _ctx| async move {
        let cookie = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let res = request.get("/admin").add_header(cookie.0, cookie.1).await;
        assert_eq!(res.status_code(), 404);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn super_admin_reaches_admin_and_their_own_org() {
    request::<App, _, _>(|request, ctx| async move {
        run_task::<App>(
            &ctx,
            Some(&"super_admin".to_string()),
            &task::Vars::default(),
        )
        .await
        .unwrap();
        let admin = users::Model::find_by_email(&ctx.db, "gurungraj26@gmail.com")
            .await
            .unwrap();
        assert!(admin.is_super_admin);

        let cookie = login(&request, "gurungraj26@gmail.com", "test-admin-password").await;
        let res = request
            .get("/admin")
            .add_header(cookie.0.clone(), cookie.1.clone())
            .await;
        assert_eq!(res.status_code(), 200);

        let res = request
            .get("/dashboard")
            .add_header(cookie.0, cookie.1)
            .await;
        assert_eq!(res.status_code(), 200);
        assert!(res.text().contains("Himalayan Ritual"));
        assert!(
            res.text().contains(r#"href="/admin""#),
            "super admins get the admin link"
        );
    })
    .await;
}
