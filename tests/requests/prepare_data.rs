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
    cookie_from(&response.headers().clone())
}

#[allow(dead_code)]
pub async fn init_user_login(request: &TestServer, ctx: &AppContext) -> LoggedInUser {
    let user = create_user(ctx, USER_EMAIL, USER_NAME).await;
    let cookie = login(request, USER_EMAIL, USER_PASSWORD).await;
    LoggedInUser { user, cookie }
}

/// Signs up a new organisation through the form and returns the owner's cookie.
#[allow(dead_code)]
pub async fn sign_up(
    request: &TestServer,
    organisation: &str,
    name: &str,
    email: &str,
) -> (HeaderName, HeaderValue) {
    let response = request
        .post("/signup")
        .form(&serde_json::json!({
            "organisation_name": organisation,
            "name": name,
            "email": email,
            "password": USER_PASSWORD,
        }))
        .await;
    assert_eq!(
        response.status_code(),
        303,
        "signup should redirect: {}",
        response.text()
    );
    cookie_from(&response.headers().clone())
}

/// Requests to join an organisation through its link and returns the new user's cookie.
#[allow(dead_code)]
pub async fn join(
    request: &TestServer,
    slug: &str,
    name: &str,
    email: &str,
) -> (HeaderName, HeaderValue) {
    let response = request
        .post(&format!("/join/{slug}"))
        .form(&serde_json::json!({ "name": name, "email": email, "password": USER_PASSWORD }))
        .await;
    assert_eq!(
        response.status_code(),
        303,
        "join should redirect: {}",
        response.text()
    );
    cookie_from(&response.headers().clone())
}

fn cookie_from(headers: &axum::http::HeaderMap) -> (HeaderName, HeaderValue) {
    let set_cookie = headers
        .get("set-cookie")
        .expect("response should set the session cookie")
        .to_str()
        .unwrap();
    let pair = set_cookie.split(';').next().unwrap().to_string();
    (
        HeaderName::from_static("cookie"),
        HeaderValue::from_str(&pair).unwrap(),
    )
}

/// Every page that must only be visible to approved members.
#[allow(dead_code)]
pub const TENANT_PAGES: [&str; 6] = [
    "/dashboard",
    "/roadmap",
    "/tasks",
    "/meetings",
    "/chat",
    "/members",
];
