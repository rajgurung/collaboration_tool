//! Rate limits on sign-in and password reset requests.
use collab::{app::App, models::auth_attempts};
use loco_rs::{app::AppContext, testing::prelude::*, TestServer};
use sea_orm::{ConnectionTrait, EntityTrait, PaginatorTrait};
use serial_test::serial;

use super::prepare_data::{create_user, USER_EMAIL, USER_NAME, USER_PASSWORD};

const TOO_MANY: &str = "Too many failed sign-ins. Try again in 15 minutes";

async fn sign_in(request: &TestServer, email: &str, password: &str, ip: &str) -> (u16, String) {
    let res = request
        .post("/login")
        .add_header("X-Forwarded-For", ip)
        .form(&serde_json::json!({ "email": email, "password": password }))
        .await;
    (res.status_code().as_u16(), res.text())
}

/// Moves every recorded attempt `minutes` into the past.
async fn age_attempts(ctx: &AppContext, minutes: i64) {
    ctx.db
        .execute_unprepared(&format!(
            "UPDATE auth_attempts SET created_at = created_at - interval '{minutes} minutes'"
        ))
        .await
        .unwrap();
}

#[tokio::test]
#[serial]
async fn five_bad_sign_ins_refuse_the_sixth_even_with_the_right_password() {
    request::<App, _, _>(|request, ctx| async move {
        create_user(&ctx, USER_EMAIL, USER_NAME).await;
        for _ in 0..5 {
            let (status, body) =
                sign_in(&request, USER_EMAIL, "wrong-password", "203.0.113.1").await;
            assert_eq!(status, 422);
            assert!(body.contains("Email or password is incorrect."));
        }
        // From another address too: the lock is on the email.
        let (status, body) = sign_in(&request, USER_EMAIL, USER_PASSWORD, "203.0.113.2").await;
        assert_eq!(status, 429);
        assert!(body.contains(TOO_MANY), "{body}");

        // The lock clears itself after 15 minutes.
        age_attempts(&ctx, 15).await;
        let (status, _) = sign_in(&request, USER_EMAIL, USER_PASSWORD, "203.0.113.1").await;
        assert_eq!(status, 303);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn unknown_emails_are_counted_and_refused_the_same_way() {
    request::<App, _, _>(|request, _ctx| async move {
        for _ in 0..5 {
            let (status, _) = sign_in(&request, "nobody@example.com", "x", "203.0.113.1").await;
            assert_eq!(status, 422);
        }
        let (status, body) = sign_in(&request, "Nobody@Example.com ", "x", "203.0.113.1").await;
        assert_eq!(status, 429);
        assert!(body.contains(TOO_MANY));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_successful_sign_in_clears_the_count() {
    request::<App, _, _>(|request, ctx| async move {
        create_user(&ctx, USER_EMAIL, USER_NAME).await;
        for _ in 0..4 {
            sign_in(&request, USER_EMAIL, "wrong-password", "203.0.113.1").await;
        }
        assert_eq!(
            sign_in(&request, USER_EMAIL, USER_PASSWORD, "203.0.113.1")
                .await
                .0,
            303
        );
        for _ in 0..4 {
            sign_in(&request, USER_EMAIL, "wrong-password", "203.0.113.1").await;
        }
        assert_eq!(
            sign_in(&request, USER_EMAIL, USER_PASSWORD, "203.0.113.1")
                .await
                .0,
            303
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn twenty_failures_from_one_ip_lock_that_ip_only() {
    request::<App, _, _>(|request, ctx| async move {
        create_user(&ctx, USER_EMAIL, USER_NAME).await;
        for n in 0..20 {
            let (status, _) = sign_in(
                &request,
                &format!("guess{n}@example.com"),
                "x",
                "198.51.100.7",
            )
            .await;
            assert_eq!(status, 422);
        }
        let (status, body) = sign_in(&request, USER_EMAIL, USER_PASSWORD, "198.51.100.7").await;
        assert_eq!(status, 429);
        assert!(body.contains(TOO_MANY));
        let (status, _) = sign_in(&request, USER_EMAIL, USER_PASSWORD, "198.51.100.8").await;
        assert_eq!(status, 303, "another address is not locked");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn reset_emails_are_limited_per_email_and_per_ip() {
    request::<App, _, _>(|request, ctx| async move {
        create_user(&ctx, USER_EMAIL, USER_NAME).await;
        for _ in 0..4 {
            let res = request
                .post("/forgot")
                .add_header("X-Forwarded-For", "203.0.113.1")
                .form(&serde_json::json!({ "email": USER_EMAIL }))
                .await;
            assert_eq!(res.status_code(), 200);
            assert!(res.text().contains("Check your email"));
        }
        assert_eq!(ctx.mailer.as_ref().unwrap().deliveries().count, 3);

        for n in 0..12 {
            let res = request
                .post("/forgot")
                .add_header("X-Forwarded-For", "198.51.100.7")
                .form(&serde_json::json!({ "email": format!("someone{n}@example.com") }))
                .await;
            assert_eq!(res.status_code(), 200);
        }
        let from_ip = auth_attempts::Entity::find()
            .all(&ctx.db)
            .await
            .unwrap()
            .into_iter()
            .filter(|a| a.ip == "198.51.100.7")
            .count();
        assert_eq!(from_ip, 10);
        assert_eq!(
            auth_attempts::Entity::find().count(&ctx.db).await.unwrap(),
            13
        );
    })
    .await;
}
