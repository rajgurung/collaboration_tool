//! A tab drawn for one organisation, after the browser switched to another.
//! Alice is in Acme and Beta, with `current_org` = Beta unless said
//! otherwise, so a claim for Acme comes from a stale tab.
use axum::http::Method;
use axum_test::multipart::{MultipartForm, Part};
use collab::{
    app::App,
    models::{
        access_tokens, conversation_members, conversations, documents, folders, memberships,
        oauth_codes, organisations, tasks, users,
    },
};
use loco_rs::{
    app::{AppContext, Hooks},
    prelude::*,
    TestServer,
};
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait, PaginatorTrait};
use serde_json::json;
use serial_test::serial;

use super::{
    multi_org::{alice_in_two_orgs, with_org},
    oauth::{location, post_decision, query_of, register, CLAUDE},
    prepare_data::{AsMember, TestMember},
    tasks::{project_in, task_form},
};

/// Routes that never run the check: no `CurrentMember`, or `PinnedMember`.
const EXEMPT: [(&str, &str); 16] = [
    ("POST", "/login"),
    ("POST", "/signup"),
    ("POST", "/join/{slug}"),
    ("POST", "/forgot"),
    ("POST", "/reset/{token}"),
    ("POST", "/logout"),
    ("POST", "/account/sign-out-everywhere"),
    ("POST", "/orgs/{id}/switch"),
    ("POST", "/admin/orgs/{id}/enter"),
    ("POST", "/admin/leave"),
    ("POST", "/oauth/register"),
    ("POST", "/oauth/token"),
    ("POST", "/mcp"),
    // Acts on the account, not the org: see `join::request`.
    ("POST", "/join/{slug}/request"),
    // PinnedMember: these forms check their own hidden org_id.
    ("POST", "/oauth/authorize"),
    ("POST", "/settings/claude/tokens"),
];

/// GET routes without `CurrentMember`. `/oauth/authorize` checks its query
/// before it looks at the member.
const EXEMPT_GET: [&str; 14] = [
    "/",
    "/login",
    "/forgot",
    "/reset/{token}",
    "/signup",
    "/join/{slug}",
    "/admin",
    "/_health",
    "/_ping",
    "/_readiness",
    "/.well-known/oauth-authorization-server",
    "/.well-known/oauth-protected-resource",
    "/.well-known/oauth-protected-resource/mcp",
    "/oauth/authorize",
];

struct Tabs {
    /// Alice with `current_org` = Beta, her pages drawn for Beta.
    beta: TestMember,
    acme_id: i64,
    beta_id: i64,
}

async fn tabs(request: &TestServer, ctx: &AppContext) -> Tabs {
    let (cookie, _, in_acme, in_beta) = alice_in_two_orgs(request, ctx).await;
    Tabs {
        beta: with_org(&cookie, in_beta.organisation_id),
        acme_id: in_acme.organisation_id,
        beta_id: in_beta.organisation_id,
    }
}

/// Every route method other than GET, HEAD and OPTIONS, with path parameters
/// filled with `1`. The check runs before any handler reads them.
fn routes(ctx: &AppContext) -> Vec<(Method, String, String)> {
    let mut all = Vec::new();
    for route in App::routes(ctx).collect() {
        let path = route
            .uri
            .split('/')
            .map(|part| if part.starts_with('{') { "1" } else { part })
            .collect::<Vec<_>>()
            .join("/");
        for method in route.actions {
            all.push((method, route.uri.clone(), path.clone()));
        }
    }
    all
}

fn is_safe(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn exempt(method: &Method, uri: &str) -> bool {
    EXEMPT.contains(&(method.as_str(), uri))
}

fn header(res: &axum_test::TestResponse, name: &str) -> String {
    res.headers()
        .get(name)
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default()
}

/// The answer to an HTMX or fetch request from a stale tab.
fn assert_sent_home_htmx(res: &axum_test::TestResponse, from: &str, what: &str) {
    assert_eq!(res.status_code(), 409, "{what}: {}", res.text());
    assert_eq!(
        header(res, "HX-Redirect"),
        format!("/dashboard?from={from}"),
        "{what}"
    );
    assert_eq!(header(res, "cache-control"), "no-store", "{what}");
}

/// The answer to a plain form from a stale tab.
fn assert_sent_home(res: &axum_test::TestResponse, from: &str, what: &str) {
    assert_eq!(res.status_code(), 303, "{what}: {}", res.text());
    assert_eq!(
        header(res, "location"),
        format!("/dashboard?from={from}"),
        "{what}"
    );
    assert_eq!(header(res, "cache-control"), "no-store", "{what}");
}

async fn task_count(ctx: &AppContext) -> u64 {
    tasks::Entity::find().count(&ctx.db).await.unwrap()
}

// ---------- Route walks ----------

#[tokio::test]
#[serial]
async fn every_non_get_route_refuses_a_header_from_another_org() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        for (method, uri, path) in routes(&ctx) {
            if is_safe(&method) || exempt(&method, &uri) {
                continue;
            }
            let res = request
                .method(method.clone(), &path)
                .add_header(t.beta.0.clone(), t.beta.1.clone())
                .add_header("x-org-id", t.acme_id.to_string())
                .form(&json!({}))
                .await;
            assert_sent_home_htmx(&res, &t.acme_id.to_string(), &format!("{method} {uri}"));
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn every_non_get_route_refuses_a_form_from_another_org() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        for (method, uri, path) in routes(&ctx) {
            if is_safe(&method) || exempt(&method, &uri) {
                continue;
            }
            let res = request
                .method(method.clone(), &path)
                .add_header(t.beta.0.clone(), t.beta.1.clone())
                .form(&json!({ "_org": t.acme_id.to_string() }))
                .await;
            assert_sent_home(&res, &t.acme_id.to_string(), &format!("{method} {uri}"));
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn every_non_get_route_refuses_a_request_with_no_org() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        for (method, uri, path) in routes(&ctx) {
            if is_safe(&method) || exempt(&method, &uri) {
                continue;
            }
            let res = request
                .method(method.clone(), &path)
                .add_header(t.beta.0.clone(), t.beta.1.clone())
                .form(&json!({}))
                .await;
            assert_sent_home(&res, "", &format!("{method} {uri}"));
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn every_htmx_get_from_another_org_is_sent_home() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        for (method, uri, path) in routes(&ctx) {
            if method != Method::GET || EXEMPT_GET.contains(&uri.as_str()) {
                continue;
            }
            let res = request
                .get(&path)
                .add_header(t.beta.0.clone(), t.beta.1.clone())
                .add_header("HX-Request", "true")
                .add_header("x-org-id", t.acme_id.to_string())
                .await;
            assert_sent_home_htmx(&res, &t.acme_id.to_string(), &format!("GET {uri}"));
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_exempt_lists_match_real_routes() {
    request::<App, _, _>(|_request, ctx| async move {
        let all = routes(&ctx);
        for (method, uri) in EXEMPT {
            assert!(
                all.iter().any(|(m, u, _)| m.as_str() == method && u == uri),
                "{method} {uri} is not a route"
            );
        }
        for uri in EXEMPT_GET {
            assert!(
                all.iter().any(|(m, u, _)| *m == Method::GET && u == uri),
                "GET {uri} is not a route"
            );
        }
        // The two PinnedMember routes, and only those, skip the check while
        // taking a member. A third needs a deliberate change here.
        let pinned: Vec<_> = EXEMPT[13..].iter().map(|(_, uri)| *uri).collect();
        assert_eq!(pinned, ["/oauth/authorize", "/settings/claude/tokens"]);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn exempt_routes_ignore_the_claim() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        for (method, uri) in EXEMPT {
            let path = uri
                .split('/')
                .map(|part| if part.starts_with('{') { "1" } else { part })
                .collect::<Vec<_>>()
                .join("/");
            let res = request
                .method(Method::from_bytes(method.as_bytes()).unwrap(), &path)
                .add_header(t.beta.0.clone(), t.beta.1.clone())
                .add_header("x-org-id", t.acme_id.to_string())
                .form(&json!({}))
                .await;
            let refused = res.status_code() == 409
                || header(&res, "HX-Redirect").starts_with("/dashboard?from=")
                || header(&res, "location").starts_with("/dashboard?from=");
            assert!(!refused, "{method} {uri} was refused by the check");
        }
    })
    .await;
}

// ---------- Claims and responses ----------

#[tokio::test]
#[serial]
async fn the_right_org_goes_through_by_header_and_by_field() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        let project = project_in(&ctx, "beta", "Beta launch").await;
        let res = request
            .post("/tasks")
            .as_member(&t.beta)
            .add_header("HX-Request", "true")
            .form(&task_form("By header", project.id, ""))
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());

        let mut form = task_form("By field", project.id, "");
        form["_org"] = t.beta_id.to_string().into();
        let res = request
            .post("/tasks")
            .add_header(t.beta.0.clone(), t.beta.1.clone())
            .form(&form)
            .await;
        assert_eq!(res.status_code(), 303, "{}", res.text());
        assert!(!header(&res, "location").starts_with("/dashboard?from="));

        for title in ["By header", "By field"] {
            let task = tasks::Entity::find()
                .all(&ctx.db)
                .await
                .unwrap()
                .into_iter()
                .find(|task| task.title == title)
                .unwrap();
            assert_eq!(task.organisation_id, t.beta_id, "{title}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn an_acme_tab_cannot_post_a_task_into_beta() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        let acme_project = project_in(&ctx, "acme", "Acme launch").await;
        let res = request
            .post("/tasks")
            .add_header(t.beta.0.clone(), t.beta.1.clone())
            .add_header("HX-Request", "true")
            .add_header("x-org-id", t.acme_id.to_string())
            .form(&task_form("From the Acme tab", acme_project.id, ""))
            .await;
        assert_sent_home_htmx(&res, &t.acme_id.to_string(), "POST /tasks");
        assert_eq!(task_count(&ctx).await, 0, "nothing in either org");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_raw_fetch_with_only_the_org_header_gets_hx_redirect() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        let res = request
            .post("/notifications/read")
            .add_header(t.beta.0.clone(), t.beta.1.clone())
            .add_header("x-org-id", t.acme_id.to_string())
            .form(&json!({}))
            .await;
        assert_sent_home_htmx(&res, &t.acme_id.to_string(), "fetch without HX-Request");
        assert!(header(&res, "location").is_empty(), "not a 303");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn garbage_claims_are_refused() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        for bad in ["abc", "", "12.5", "99999999999999999999"] {
            let res = request
                .post("/notifications/read")
                .add_header(t.beta.0.clone(), t.beta.1.clone())
                .add_header("x-org-id", bad)
                .form(&json!({}))
                .await;
            assert_sent_home_htmx(&res, "", &format!("X-Org-Id: {bad:?}"));
        }
        // Two headers, even if one is right.
        let res = request
            .post("/notifications/read")
            .add_header(t.beta.0.clone(), t.beta.1.clone())
            .add_header("x-org-id", t.beta_id.to_string())
            .add_header("x-org-id", t.beta_id.to_string())
            .form(&json!({}))
            .await;
        assert_sent_home_htmx(&res, "", "two X-Org-Id headers");

        let res = request
            .post("/notifications/read")
            .add_header(t.beta.0.clone(), t.beta.1.clone())
            .form(&json!({ "_org": "abc" }))
            .await;
        assert_sent_home(&res, "", "_org=abc");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_refusal_never_echoes_the_raw_claim() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        let res = request
            .post("/notifications/read")
            .add_header(t.beta.0.clone(), t.beta.1.clone())
            .add_header("x-org-id", "//evil.example")
            .form(&json!({}))
            .await;
        assert_sent_home_htmx(&res, "", "X-Org-Id: //evil.example");
        let res = request
            .post("/notifications/read")
            .add_header(t.beta.0.clone(), t.beta.1.clone())
            .form(&json!({ "_org": "//evil" }))
            .await;
        assert_sent_home(&res, "", "_org=%2F%2Fevil");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_content_type_is_matched_by_its_base() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        let res = request
            .post("/notifications/read")
            .add_header(t.beta.0.clone(), t.beta.1.clone())
            .content_type("Application/X-WWW-Form-Urlencoded; charset=UTF-8")
            .bytes(format!("_org={}", t.acme_id).into())
            .await;
        // Refused for Acme, so the layer read the field.
        assert_sent_home(&res, &t.acme_id.to_string(), "mixed-case content type");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_large_form_is_still_read() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        let project = project_in(&ctx, "beta", "Beta launch").await;
        // `padding` is ignored by the task form; it only makes the body large.
        let body = format!(
            "title=Long+one&project_id={}&priority=high&padding={}&_org={}",
            project.id,
            "x".repeat(100_000),
            t.beta_id
        );
        let res = request
            .post("/tasks")
            .add_header(t.beta.0.clone(), t.beta.1.clone())
            .content_type("application/x-www-form-urlencoded")
            .bytes(body.into())
            .await;
        assert_eq!(res.status_code(), 303, "{}", res.text());
        assert!(!header(&res, "location").starts_with("/dashboard?from="));
        assert_eq!(task_count(&ctx).await, 1);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_post_without_a_session_is_not_buffered() {
    request::<App, _, _>(|request, ctx| async move {
        tabs(&request, &ctx).await;
        let res = request
            .post("/login")
            .form(&json!({
                "email": "alice@example.com",
                "password": super::prepare_data::USER_PASSWORD,
                "_org": "12345",
            }))
            .await;
        assert_eq!(res.status_code(), 303);
        assert!(!header(&res, "location").starts_with("/dashboard?from="));
        assert!(
            header(&res, "set-cookie").starts_with("auth_token="),
            "signed in"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_cross_site_post_is_refused_before_the_layer() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        // From another org too: the claim check would send it Home, so a 403
        // means the origin check answered first. (The 413 for a body over the
        // limit is tested in `src/initializers/current_org.rs`.)
        let res = request
            .post("/tasks")
            .add_header(t.beta.0.clone(), t.beta.1.clone())
            .add_header("Origin", "https://evil.example")
            .form(&json!({ "_org": t.acme_id.to_string(), "title": "x" }))
            .await;
        assert_eq!(res.status_code(), 403);
        assert_eq!(res.text(), "Cross-site request blocked");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn polling_badges_move_a_stale_tab() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        for path in ["/notifications/unread", "/chat/unread"] {
            let res = request
                .get(path)
                .add_header(t.beta.0.clone(), t.beta.1.clone())
                .add_header("HX-Request", "true")
                .add_header("x-org-id", t.acme_id.to_string())
                .await;
            assert_sent_home_htmx(&res, &t.acme_id.to_string(), path);
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn an_htmx_get_without_a_claim_still_works() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        let res = request
            .get("/notifications/unread")
            .add_header(t.beta.0.clone(), t.beta.1.clone())
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 200);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn options_is_treated_like_get() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        let res = request
            .method(Method::OPTIONS, "/tasks")
            .add_header(t.beta.0.clone(), t.beta.1.clone())
            .add_header("x-org-id", t.acme_id.to_string())
            .await;
        assert_ne!(res.status_code(), 409);
        assert!(!header(&res, "location").starts_with("/dashboard?from="));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_page_load_ignores_the_claim() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        let res = request
            .get("/tasks")
            .add_header(t.beta.0.clone(), t.beta.1.clone())
            .add_header("x-org-id", t.acme_id.to_string())
            .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(
            super::prepare_data::page_org_id(&res.text()),
            Some(t.beta_id),
            "the page is drawn for Beta"
        );
    })
    .await;
}

async fn beta_company(ctx: &AppContext, beta_id: i64) -> folders::Model {
    folders::Model::list_for_org(&ctx.db, beta_id)
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.folder.name == "Company")
        .unwrap()
        .folder
}

fn a_pdf() -> MultipartForm {
    MultipartForm::new().add_part(
        "file",
        Part::bytes(super::documents::PDF.to_vec()).file_name("lease.pdf"),
    )
}

async fn document_count(ctx: &AppContext) -> u64 {
    documents::Entity::find().count(&ctx.db).await.unwrap()
}

#[tokio::test]
#[serial]
async fn a_document_upload_from_another_org_is_refused_and_nothing_is_stored() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        let folder = beta_company(&ctx, t.beta_id).await;
        let res = request
            .post(&format!("/documents/folders/{}/files", folder.id))
            .add_header(t.beta.0.clone(), t.beta.1.clone())
            .add_header("HX-Request", "true")
            .add_header("x-org-id", t.acme_id.to_string())
            .multipart(a_pdf())
            .await;
        assert_sent_home_htmx(&res, &t.acme_id.to_string(), "upload");
        // The handler never ran, so nothing reached storage either.
        assert_eq!(document_count(&ctx).await, 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_document_upload_with_the_right_org_goes_through() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        let folder = beta_company(&ctx, t.beta_id).await;
        let res = request
            .post(&format!("/documents/folders/{}/files", folder.id))
            .as_member(&t.beta)
            .add_header("HX-Request", "true")
            .multipart(a_pdf())
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        assert_eq!(document_count(&ctx).await, 1);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_multipart_post_with_no_header_fails_closed() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        let folder = beta_company(&ctx, t.beta_id).await;
        let res = request
            .post(&format!("/documents/folders/{}/files", folder.id))
            .add_header(t.beta.0.clone(), t.beta.1.clone())
            .multipart(a_pdf())
            .await;
        assert_sent_home(&res, "", "multipart without a header");
        assert_eq!(document_count(&ctx).await, 0);
    })
    .await;
}

/// The platform super admin, with a membership of their own in Acme.
async fn admin_owning_acme(
    request: &TestServer,
    ctx: &AppContext,
    acme_id: i64,
) -> super::prepare_data::Cookie {
    let cookie = super::admin::super_admin_cookie(request, ctx).await;
    let admin = users::Model::find_by_email(&ctx.db, "gurungraj26@gmail.com")
        .await
        .unwrap();
    memberships::ActiveModel {
        user_id: ActiveValue::Set(admin.id),
        username: ActiveValue::Set("raj".to_string()),
        role: ActiveValue::Set(memberships::role::OWNER.to_string()),
        status: ActiveValue::Set(memberships::status::ACTIVE.to_string()),
        ..Default::default()
    }
    .set_tenant(acme_id)
    .unwrap()
    .insert(&ctx.db)
    .await
    .unwrap();
    cookie
}

fn acting_in(cookie: &super::prepare_data::Cookie, org_id: i64) -> String {
    format!("{}; acting_org={org_id}", cookie.1.to_str().unwrap())
}

#[tokio::test]
#[serial]
async fn acting_checks_against_the_acting_org() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        let admin = admin_owning_acme(&request, &ctx, t.acme_id).await;
        let acting = acting_in(&admin, t.beta_id);
        // A tab drawn for their own org, while they are inside Beta.
        let res = request
            .post("/notifications/read")
            .add_header("cookie", acting.clone())
            .add_header("x-org-id", t.acme_id.to_string())
            .form(&json!({}))
            .await;
        assert_sent_home_htmx(&res, &t.acme_id.to_string(), "own org while acting");
        let res = request
            .post("/notifications/read")
            .add_header("cookie", acting)
            .add_header("x-org-id", t.beta_id.to_string())
            .form(&json!({}))
            .await;
        assert_eq!(res.status_code(), 303);
        assert!(!header(&res, "location").starts_with("/dashboard?from="));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_chat_socket_upgrade_is_not_checked() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        let general = conversations::Model::find_general(&ctx.db, t.beta_id)
            .await
            .unwrap();
        let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();
        conversation_members::Model::add(&ctx.db, &general, alice.id)
            .await
            .unwrap();
        let res = request
            .get(&format!("/chat/{}/ws", general.id))
            .add_header(t.beta.0.clone(), t.beta.1.clone())
            .add_header("x-org-id", t.acme_id.to_string())
            .await;
        // Past the check: refused only because this is not an upgrade.
        assert_eq!(res.status_code(), 400, "{}", res.text());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_allow_page_keeps_its_own_pin() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        let client_id = register(&request, CLAUDE).await;
        // A wrong header and the right org_id: the code is issued, for Beta.
        let res = post_decision(
            &request,
            &t.beta.cookie,
            &client_id,
            CLAUDE,
            "allow",
            Some(&t.beta_id.to_string()),
        )
        .await;
        assert_eq!(res.status_code(), 302, "{}", res.text());
        assert!(query_of(&location(res.headers())).contains_key("code"));
        let codes = oauth_codes::Entity::find().all(&ctx.db).await.unwrap();
        assert_eq!(codes.len(), 1);
        assert_eq!(codes[0].organisation_id, t.beta_id);

        // Deny with no org_id still goes back to Claude.
        let res = post_decision(&request, &t.beta.cookie, &client_id, CLAUDE, "deny", None).await;
        assert_eq!(res.status_code(), 302);
        assert_eq!(query_of(&location(res.headers()))["error"], "access_denied");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_token_form_keeps_its_own_pin() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        let res = request
            .post("/settings/claude/tokens")
            .add_header(t.beta.0.clone(), t.beta.1.clone())
            .add_header("x-org-id", t.acme_id.to_string())
            .form(&json!({ "name": "Laptop", "org_id": t.beta_id.to_string() }))
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        assert!(res.text().contains("collab_pat_"));

        let res = request
            .post("/settings/claude/tokens")
            .add_header(t.beta.0.clone(), t.beta.1.clone())
            .add_header("x-org-id", t.beta_id.to_string())
            .form(&json!({ "name": "Typed name", "org_id": t.acme_id.to_string() }))
            .await;
        assert_eq!(res.status_code(), 409);
        let page = res.text();
        assert!(page.contains("Typed name"), "the typed name is kept");
        assert!(page.contains("You switched organisation while this page was open"));
        assert_eq!(
            access_tokens::Entity::find().count(&ctx.db).await.unwrap(),
            1
        );
    })
    .await;
}

// ---------- The notice on Home ----------

async fn home_from(request: &TestServer, who: &TestMember, from: &str) -> axum_test::TestResponse {
    request
        .get(&format!("/dashboard?from={from}"))
        .add_header(who.0.clone(), who.1.clone())
        .await
}

#[tokio::test]
#[serial]
async fn home_names_the_org_the_tab_was_showing() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        let res = home_from(&request, &t.beta, &t.acme_id.to_string()).await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(header(&res, "cache-control"), "no-store");
        let page = res.text();
        let notice = notice(&page);
        assert!(
            notice.contains("This tab was showing Acme. You're now in Beta."),
            "{notice}"
        );
        assert!(notice.contains("Nothing was saved."));
        assert!(notice.contains(&format!(r#"action="/orgs/{}/switch""#, t.acme_id)));
        assert!(notice.contains("Back to Acme"));
    })
    .await;
}

async fn stranger_org(request: &TestServer, ctx: &AppContext, name: &str) -> i64 {
    let email = format!("{}@example.com", name.to_lowercase());
    super::prepare_data::sign_up_cookie(request, name, &name.to_lowercase(), &email).await;
    organisations::Model::find_by_slug(&ctx.db, &name.to_lowercase())
        .await
        .unwrap()
        .id
}

/// The notice card on Home.
fn notice(page: &str) -> &str {
    let start = page.find("data-stale-notice").expect("a notice");
    &page[start..start + page[start..].find("data-dismiss-notice").unwrap()]
}

fn assert_nameless(page: &str) {
    let notice = notice(page);
    assert!(
        notice.contains("This tab was showing another organisation."),
        "{notice}"
    );
    assert!(
        !notice.contains("Back to ") && !notice.contains("/switch"),
        "no Back: {notice}"
    );
}

#[tokio::test]
#[serial]
async fn home_does_not_name_an_org_you_are_not_in() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        let globex = stranger_org(&request, &ctx, "Globex").await;
        let page = home_from(&request, &t.beta, &globex.to_string())
            .await
            .text();
        assert_nameless(&page);
        assert!(!page.contains("Globex"), "the name appears nowhere");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn home_does_not_name_an_org_you_are_pending_in() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        let gamma = stranger_org(&request, &ctx, "Gamma").await;
        let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();
        memberships::ActiveModel {
            user_id: ActiveValue::Set(alice.id),
            username: ActiveValue::Set("alice".to_string()),
            role: ActiveValue::Set(memberships::role::MEMBER.to_string()),
            status: ActiveValue::Set(memberships::status::PENDING.to_string()),
            ..Default::default()
        }
        .set_tenant(gamma)
        .unwrap()
        .insert(&ctx.db)
        .await
        .unwrap();
        let page = home_from(&request, &t.beta, &gamma.to_string())
            .await
            .text();
        assert_nameless(&page);
        assert!(!page.contains("This tab was showing Gamma"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn home_does_not_name_a_deleted_org() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        let page = home_from(&request, &t.beta, "987654").await.text();
        assert_nameless(&page);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn home_offers_no_back_after_acting_outside_your_orgs() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        let admin = admin_owning_acme(&request, &ctx, t.acme_id).await;
        // A tab drawn while acting in Beta, after the admin went back to Acme.
        let page = request
            .get(&format!("/dashboard?from={}", t.beta_id))
            .add_header(admin.0.clone(), admin.1.clone())
            .await
            .text();
        assert_nameless(&page);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn home_with_an_empty_or_garbage_from_shows_the_nameless_notice() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        for from in ["", "abc", "%2F%2Fevil.example"] {
            let res = home_from(&request, &t.beta, from).await;
            assert_eq!(res.status_code(), 200, "from={from}");
            assert_nameless(&res.text());
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn home_from_the_current_org_shows_no_notice() {
    request::<App, _, _>(|request, ctx| async move {
        let t = tabs(&request, &ctx).await;
        let page = home_from(&request, &t.beta, &t.beta_id.to_string())
            .await
            .text();
        assert!(!page.contains("data-stale-notice"));
        let page = request
            .get("/dashboard")
            .add_header(t.beta.0.clone(), t.beta.1.clone())
            .await
            .text();
        assert!(!page.contains("data-stale-notice"));
    })
    .await;
}
