use chrono::{Duration, Local};
use collab::{app::App, models::users};
use loco_rs::testing::prelude::*;
use sea_orm::{ActiveModelTrait, ActiveValue, IntoActiveModel};
use serial_test::serial;

use super::prepare_data::{create_user, login, USER_EMAIL, USER_NAME, USER_PASSWORD};

fn set_cookie_header(headers: &axum::http::HeaderMap) -> String {
    headers
        .get("set-cookie")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default()
}

#[tokio::test]
#[serial]
async fn login_page_renders() {
    request::<App, _, _>(|request, _ctx| async move {
        let res = request.get("/login").await;
        assert_eq!(res.status_code(), 200);
        assert!(res.text().contains(r#"action="/login""#));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn login_sets_http_only_session_cookie() {
    request::<App, _, _>(|request, ctx| async move {
        create_user(&ctx, USER_EMAIL, USER_NAME).await;
        let res = request
            .post("/login")
            .form(&serde_json::json!({ "email": USER_EMAIL, "password": USER_PASSWORD }))
            .await;

        assert_eq!(res.status_code(), 303);
        assert_eq!(res.headers().get("location").unwrap(), "/");
        let cookie = set_cookie_header(res.headers());
        assert!(cookie.starts_with("auth_token="), "{cookie}");
        assert!(cookie.contains("HttpOnly"), "{cookie}");
        assert!(cookie.contains("SameSite=Lax"), "{cookie}");
        assert!(cookie.contains("Path=/"), "{cookie}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn login_ignores_email_case() {
    request::<App, _, _>(|request, ctx| async move {
        create_user(&ctx, "Mixed.Case@Example.com", USER_NAME).await;
        let user = users::Model::find_by_email(&ctx.db, "mixed.case@example.com").await;
        assert_eq!(user.unwrap().email, "mixed.case@example.com");
        login(&request, "MIXED.case@example.COM", USER_PASSWORD).await;
    })
    .await;
}

#[tokio::test]
#[serial]
async fn login_rejects_wrong_password_and_unknown_email_the_same_way() {
    request::<App, _, _>(|request, ctx| async move {
        create_user(&ctx, USER_EMAIL, USER_NAME).await;
        for (email, password) in [
            (USER_EMAIL, "wrong-password"),
            ("nobody@example.com", USER_PASSWORD),
        ] {
            let res = request
                .post("/login")
                .form(&serde_json::json!({ "email": email, "password": password }))
                .await;
            assert_eq!(res.status_code(), 422);
            assert!(res.text().contains("Email or password is incorrect."));
            assert!(set_cookie_header(res.headers()).is_empty());
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn logout_clears_the_cookie() {
    request::<App, _, _>(|request, _ctx| async move {
        let res = request.post("/logout").await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(res.headers().get("location").unwrap(), "/login");
        let cookie = set_cookie_header(res.headers());
        assert!(cookie.starts_with("auth_token=;"), "{cookie}");
        assert!(cookie.contains("Max-Age=0"), "{cookie}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn logout_from_htmx_uses_hx_redirect() {
    request::<App, _, _>(|request, _ctx| async move {
        let res = request
            .post("/logout")
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(res.headers().get("HX-Redirect").unwrap(), "/login");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn forgot_for_unknown_email_sends_nothing_but_looks_the_same() {
    request::<App, _, _>(|request, ctx| async move {
        let res = request
            .post("/forgot")
            .form(&serde_json::json!({ "email": "nobody@example.com" }))
            .await;
        assert_eq!(res.status_code(), 200);
        assert!(res.text().contains("Check your email"));
        assert_eq!(ctx.mailer.unwrap().deliveries().count, 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn full_password_reset_flow() {
    request::<App, _, _>(|request, ctx| async move {
        create_user(&ctx, USER_EMAIL, USER_NAME).await;

        let res = request
            .post("/forgot")
            .form(&serde_json::json!({ "email": USER_EMAIL }))
            .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(ctx.mailer.as_ref().unwrap().deliveries().count, 1);

        let user = users::Model::find_by_email(&ctx.db, USER_EMAIL).await.unwrap();
        let token = user.reset_token.clone().expect("token should be stored");
        let reset_path = format!("/reset/{token}");

        assert_eq!(request.get(&reset_path).await.status_code(), 200);

        let mismatch = request
            .post(&reset_path)
            .form(&serde_json::json!({ "password": "new-password-1", "password_confirmation": "different-1" }))
            .await;
        assert_eq!(mismatch.status_code(), 422);
        assert!(mismatch.text().contains("do not match"));

        let too_short = request
            .post(&reset_path)
            .form(&serde_json::json!({ "password": "short", "password_confirmation": "short" }))
            .await;
        assert_eq!(too_short.status_code(), 422);
        assert!(too_short.text().contains("at least 8 characters"));

        let ok = request
            .post(&reset_path)
            .form(&serde_json::json!({ "password": "new-password-1", "password_confirmation": "new-password-1" }))
            .await;
        assert_eq!(ok.status_code(), 303);
        assert_eq!(ok.headers().get("location").unwrap(), "/login?reset=1");

        login(&request, USER_EMAIL, "new-password-1").await;

        let reused = request
            .post(&reset_path)
            .form(&serde_json::json!({ "password": "another-pass-2", "password_confirmation": "another-pass-2" }))
            .await;
        assert_eq!(reused.status_code(), 404, "a reset link works only once");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn expired_reset_link_is_rejected() {
    request::<App, _, _>(|request, ctx| async move {
        let user = create_user(&ctx, USER_EMAIL, USER_NAME).await;
        let user = user
            .into_active_model()
            .set_forgot_password_sent(&ctx.db)
            .await
            .unwrap();
        let token = user.reset_token.clone().unwrap();
        let mut stale = user.into_active_model();
        stale.reset_sent_at = ActiveValue::Set(Some((Local::now() - Duration::minutes(61)).into()));
        stale.update(&ctx.db).await.unwrap();

        let res = request.get(&format!("/reset/{token}")).await;
        assert_eq!(res.status_code(), 404);
        assert!(res.text().contains("no longer works"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn cross_origin_posts_are_blocked() {
    request::<App, _, _>(|request, ctx| async move {
        create_user(&ctx, USER_EMAIL, USER_NAME).await;
        let form = serde_json::json!({ "email": USER_EMAIL, "password": USER_PASSWORD });

        let evil = request
            .post("/login")
            .add_header("Origin", "https://evil.example")
            .form(&form)
            .await;
        assert_eq!(evil.status_code(), 403);

        let same = request
            .post("/login")
            .add_header("Origin", "http://localhost:5150")
            .form(&form)
            .await;
        assert_eq!(same.status_code(), 303);
    })
    .await;
}
