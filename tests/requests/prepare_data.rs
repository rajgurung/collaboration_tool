use axum::http::{HeaderName, HeaderValue};
use collab::models::{
    organisations,
    users::{self, RegisterParams},
};
use loco_rs::{app::AppContext, TestServer};

pub const USER_EMAIL: &str = "test@loco.com";
pub const USER_NAME: &str = "tester";
pub const USER_PASSWORD: &str = "correct-horse";

pub type Cookie = (HeaderName, HeaderValue);

/// A signed-in member: the session cookie, and the organisation their pages
/// are drawn for. Derefs to the cookie, so GETs keep using `.0` and `.1`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestMember {
    pub cookie: Cookie,
    pub org_id: i64,
}

impl TestMember {
    #[allow(dead_code)]
    pub fn new(cookie: Cookie, org_id: i64) -> Self {
        Self { cookie, org_id }
    }
}

impl std::ops::Deref for TestMember {
    type Target = Cookie;

    fn deref(&self) -> &Cookie {
        &self.cookie
    }
}

pub trait AsMember {
    /// The member's cookie and the `X-Org-Id` their page would send, as a tab
    /// drawn for their organisation does. A builder step, read as "as this
    /// member", hence the name.
    #[allow(clippy::wrong_self_convention)]
    fn as_member(self, member: &TestMember) -> Self;
}

impl AsMember for axum_test::TestRequest {
    fn as_member(self, member: &TestMember) -> Self {
        self.add_header(member.cookie.0.clone(), member.cookie.1.clone())
            .add_header("x-org-id", member.org_id.to_string())
    }
}

/// The organisation id on a page's `<body data-org-id>`.
#[allow(dead_code)]
pub fn page_org_id(html: &str) -> Option<i64> {
    let start = html.find("data-org-id=\"")? + "data-org-id=\"".len();
    let len = html[start..].find('"')?;
    html[start..start + len].parse().ok()
}

/// Which organisation the cookie's pages are drawn for, read from Home.
#[allow(dead_code)]
pub async fn member_from(request: &TestServer, cookie: Cookie) -> TestMember {
    let home = request
        .get("/dashboard")
        .add_header(cookie.0.clone(), cookie.1.clone())
        .await;
    let org_id = page_org_id(&home.text()).expect("Home names its organisation");
    TestMember { cookie, org_id }
}

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

/// Signs up a new organisation through the form and returns the owner.
#[allow(dead_code)]
pub async fn sign_up_member(
    request: &TestServer,
    organisation: &str,
    name: &str,
    email: &str,
) -> TestMember {
    let cookie = sign_up_cookie(request, organisation, name, email).await;
    member_from(request, cookie).await
}

/// Signs up a new organisation and returns only the owner's cookie.
pub async fn sign_up_cookie(
    request: &TestServer,
    organisation: &str,
    name: &str,
    email: &str,
) -> Cookie {
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

/// Requests to join an organisation through its link and returns the new
/// user, with that organisation for when they are approved.
#[allow(dead_code)]
pub async fn join(
    request: &TestServer,
    ctx: &AppContext,
    slug: &str,
    name: &str,
    email: &str,
) -> TestMember {
    let org = organisations::Model::find_by_slug(&ctx.db, slug)
        .await
        .expect("the organisation exists");
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
    TestMember::new(cookie_from(&response.headers().clone()), org.id)
}

pub fn cookie_from(headers: &axum::http::HeaderMap) -> (HeaderName, HeaderValue) {
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
