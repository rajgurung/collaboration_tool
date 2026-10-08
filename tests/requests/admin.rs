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

pub(super) async fn super_admin_cookie(
    request: &loco_rs::TestServer,
    ctx: &loco_rs::app::AppContext,
) -> (axum::http::HeaderName, axum::http::HeaderValue) {
    run_task::<App>(
        ctx,
        Some(&"super_admin".to_string()),
        &task::Vars::default(),
    )
    .await
    .unwrap();
    login(request, "gurungraj26@gmail.com", "test-admin-password").await
}

#[tokio::test]
#[serial]
async fn admin_lists_every_org_and_person() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up(&request, "Acme", "alice", "alice@example.com").await;
        super::prepare_data::join(&request, "acme", "bob", "bob@example.com").await;
        let admin = super_admin_cookie(&request, &ctx).await;
        let body = request
            .get("/admin")
            .add_header(admin.0, admin.1)
            .await
            .text();
        for expected in [
            "Acme",
            "Himalayan Ritual",
            "alice@example.com",
            "bob@example.com",
            "pending",
        ] {
            assert!(body.contains(expected), "{expected}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn super_admin_can_enter_and_leave_another_org() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let admin = super_admin_cookie(&request, &ctx).await;
        let acme = collab::models::organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap();

        let enter = request
            .post(&format!("/admin/orgs/{}/enter", acme.id))
            .add_header(admin.0.clone(), admin.1.clone())
            .await;
        assert_eq!(enter.status_code(), 303);
        let acting = enter
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            acting.starts_with(&format!("acting_org={};", acme.id)),
            "{acting}"
        );
        assert!(acting.contains("HttpOnly"));

        let both = format!("{}; acting_org={}", admin.1.to_str().unwrap(), acme.id);
        let page = request
            .get("/dashboard")
            .add_header("cookie", both.clone())
            .await;
        assert_eq!(page.status_code(), 200);
        let body = page.text();
        assert!(body.contains("Acme"));
        assert!(body.contains("as platform admin"));

        // Acting as owner: the admin can see Acme's members page.
        let members = request
            .get("/members")
            .add_header("cookie", both.clone())
            .await;
        assert!(members.text().contains("alice@example.com"));

        let leave = request
            .post("/admin/leave")
            .add_header("cookie", both)
            .await;
        assert_eq!(leave.status_code(), 303);
        let cleared = leave
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            cleared.starts_with("acting_org=;") && cleared.contains("Max-Age=0"),
            "{cleared}"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_acting_cookie_means_nothing_to_normal_users() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        sign_up(&request, "Globex", "gina", "gina@example.com").await;
        let globex = collab::models::organisations::Model::find_by_slug(&ctx.db, "globex")
            .await
            .unwrap();

        let forged = format!("{}; acting_org={}", alice.1.to_str().unwrap(), globex.id);
        let page = request.get("/dashboard").add_header("cookie", forged).await;
        assert_eq!(page.status_code(), 200);
        assert!(page.text().contains("Acme"));
        assert!(!page.text().contains("Globex"));

        for path in [
            format!("/admin/orgs/{}/enter", globex.id),
            "/admin/leave".to_string(),
        ] {
            let res = request
                .post(&path)
                .add_header(alice.0.clone(), alice.1.clone())
                .await;
            assert_eq!(res.status_code(), 404, "{path}");
        }
    })
    .await;
}
