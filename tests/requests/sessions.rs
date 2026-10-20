//! Session versions: every session cookie carries the user's version, and
//! bumping it (sign out everywhere, a password reset, a lock) ends them all.
use collab::{
    app::App,
    data::{
        chat_hub::{ChatEvent, ChatHub},
        live::CloseScope,
        notify_hub::{NotifyEvent, NotifyHub},
    },
    models::users,
};
use loco_rs::{auth::jwt, testing::prelude::*, TestServer};
use serial_test::serial;

use super::prepare_data::{cookie_from, login, open_page, sign_up_member, Cookie, USER_PASSWORD};

async fn can_open_more(request: &TestServer, cookie: &Cookie) -> bool {
    let (res, _) = open_page(request, cookie, "/more").await;
    match res.status_code().as_u16() {
        200 => true,
        303 => {
            assert_eq!(res.headers().get("location").unwrap(), "/login");
            false
        }
        other => panic!("unexpected status {other}"),
    }
}

#[tokio::test]
#[serial]
async fn sign_out_everywhere_ends_other_sessions_but_keeps_this_one() {
    request::<App, _, _>(|request, ctx| async move {
        let phone = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let laptop = login(&request, "alice@example.com", USER_PASSWORD).await;
        let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();
        let mut chat = ctx.shared_store.get::<ChatHub>().unwrap().subscribe();
        let mut notify = ctx.shared_store.get::<NotifyHub>().unwrap().subscribe();

        let res = request
            .post("/account/sign-out-everywhere")
            .add_header(laptop.0.clone(), laptop.1.clone())
            .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(res.headers().get("location").unwrap(), "/more?signed_out=1");
        let fresh = cookie_from(&res.headers().clone());

        assert!(
            !can_open_more(&request, &phone).await,
            "the phone is signed out"
        );
        assert!(
            !can_open_more(&request, &laptop).await,
            "the old laptop cookie too"
        );
        assert!(
            can_open_more(&request, &fresh).await,
            "this device stays in"
        );
        let page = open_page(&request, &fresh, "/more?signed_out=1")
            .await
            .0
            .text();
        assert!(page.contains("Signed out on every other device."));

        // Open chat sockets and notification streams are told to close.
        let everywhere = CloseScope::Account { user_id: alice.id };
        assert!(matches!(
            chat.try_recv().unwrap(),
            ChatEvent::Close(scope) if scope == everywhere
        ));
        assert_eq!(notify.try_recv().unwrap(), NotifyEvent::Close(everywhere));

        let again = users::Model::find_by_id(&ctx.db, alice.id).await.unwrap();
        assert_eq!(again.session_version, alice.session_version + 1);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn signing_out_everywhere_needs_a_session() {
    request::<App, _, _>(|request, _ctx| async move {
        let res = request.post("/account/sign-out-everywhere").await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(res.headers().get("location").unwrap(), "/login");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_password_reset_ends_every_session() {
    request::<App, _, _>(|request, ctx| async move {
        let cookie = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        request
            .post("/forgot")
            .form(&serde_json::json!({ "email": "alice@example.com" }))
            .await;
        let token = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap()
            .reset_token
            .unwrap();
        let res = request
            .post(&format!("/reset/{token}"))
            .form(&serde_json::json!({ "password": "new-password-1", "password_confirmation": "new-password-1" }))
            .await;
        assert_eq!(res.status_code(), 303);
        assert!(!can_open_more(&request, &cookie).await);

        // The reset link, then "Your password was changed".
        let deliveries = ctx.mailer.as_ref().unwrap().deliveries();
        assert_eq!(deliveries.count, 2);
        assert!(deliveries.messages[1].contains("Your Collaboration Tool password was changed"));
        assert!(deliveries.messages[1].contains("To: alice@example.com"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_token_without_a_session_version_is_refused() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();
        let config = ctx.config.get_jwt_config().unwrap();
        // How tokens looked before session versions.
        let old = jwt::JWT::new(&config.secret)
            .generate_token(
                config.expiration,
                alice.pid.to_string(),
                serde_json::Map::new(),
            )
            .unwrap();
        let cookie = (
            axum::http::HeaderName::from_static("cookie"),
            axum::http::HeaderValue::from_str(&format!("auth_token={old}")).unwrap(),
        );
        assert!(!can_open_more(&request, &cookie).await);

        let current = alice
            .generate_jwt(&config.secret, config.expiration)
            .unwrap();
        let cookie = (
            axum::http::HeaderName::from_static("cookie"),
            axum::http::HeaderValue::from_str(&format!("auth_token={current}")).unwrap(),
        );
        assert!(can_open_more(&request, &cookie).await);
    })
    .await;
}
