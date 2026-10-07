use axum::http::{HeaderName, HeaderValue};
use collab::models::users::{self, RegisterParams};
use loco_rs::{app::AppContext, TestServer};

pub const USER_EMAIL: &str = "test@loco.com";
pub const USER_NAME: &str = "tester";
pub const USER_PASSWORD: &str = "correct-horse";

#[allow(dead_code)]
pub struct LoggedInUser {
    pub user: users::Model,
    pub cookie: (HeaderName, HeaderValue),
}

/// Creates a user directly through the model.
pub async fn create_user(ctx: &AppContext, email: &str, name: &str) -> users::Model {
    users::Model::create_with_password(
        &ctx.db,
        &RegisterParams {
            email: email.to_string(),
            password: USER_PASSWORD.to_string(),
            name: name.to_string(),
        },
    )
    .await
    .expect("test user should be created")
}

/// Signs in through the login form and returns the session cookie as a request header.
pub async fn login(request: &TestServer, email: &str, password: &str) -> (HeaderName, HeaderValue) {
    let response = request
        .post("/login")
        .form(&serde_json::json!({ "email": email, "password": password }))
        .await;
    assert_eq!(response.status_code(), 303, "login should redirect");
    let set_cookie = response
        .headers()
        .get("set-cookie")
        .expect("login should set the session cookie")
        .to_str()
        .unwrap();
    let pair = set_cookie.split(';').next().unwrap().to_string();
    (
        HeaderName::from_static("cookie"),
        HeaderValue::from_str(&pair).unwrap(),
    )
}

#[allow(dead_code)]
pub async fn init_user_login(request: &TestServer, ctx: &AppContext) -> LoggedInUser {
    let user = create_user(ctx, USER_EMAIL, USER_NAME).await;
    let cookie = login(request, USER_EMAIL, USER_PASSWORD).await;
    LoggedInUser { user, cookie }
}
