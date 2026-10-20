//! Addresses without `/o/{slug}/`: old bookmarks, emails and links, and tabs
//! drawn before the deploy. A page load is redirected into an org, keeping
//! its path and query; anything else is refused and nothing is saved.
use axum::http::Method;
use axum_test::multipart::{MultipartForm, Part};
use collab::{
    app::App,
    models::{
        access_tokens, conversations, documents, folders, logins, meetings, memberships, messages,
        notifications, organisations, projects, saved_views, task_notes, tasks, users,
    },
    views::links::TENANT_SECTIONS,
};
use loco_rs::{
    app::{AppContext, Hooks},
    prelude::*,
    TestServer,
};
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait, IntoActiveModel, PaginatorTrait};
use serde_json::json;
use serial_test::serial;

use super::{
    admin::super_admin_cookie,
    multi_org::{alice_in_two_orgs, set_status, with_org},
    oauth::{authorize_url, register, CLAUDE},
    prepare_data::{approve_as_owner, join, sign_up_member, Cookie, TestMember},
    tasks::{project_in, task_form},
};

fn header(res: &axum_test::TestResponse, name: &str) -> String {
    res.headers()
        .get(name)
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default()
}

/// A plain page load with just the session cookie, as a bookmark or an email
/// link sends it.
async fn open(request: &TestServer, cookie: &Cookie, path: &str) -> axum_test::TestResponse {
    request
        .get(path)
        .add_header(cookie.0.clone(), cookie.1.clone())
        .add_header("accept", "text/html")
        .await
}

fn assert_see_other(res: &axum_test::TestResponse, to: &str, what: &str) {
    assert_eq!(res.status_code(), 303, "{what}: {}", res.text());
    assert_eq!(header(res, "location"), to, "{what}");
    assert_eq!(header(res, "cache-control"), "no-store", "{what}");
}

/// Tenant routes with this method, path parameters filled with `1`.
fn tenant_routes(ctx: &AppContext, get: bool) -> Vec<(Method, String)> {
    let mut all = Vec::new();
    for route in App::routes(ctx).collect() {
        let first = route.uri.trim_start_matches('/').split('/').next().unwrap();
        if !TENANT_SECTIONS.contains(&first) {
            continue;
        }
        let path = route
            .uri
            .split('/')
            .map(|part| if part.starts_with('{') { "1" } else { part })
            .collect::<Vec<_>>()
            .join("/");
        for method in route.actions {
            if (method == Method::GET) == get {
                all.push((method, path.clone()));
            }
        }
    }
    all
}

#[tokio::test]
#[serial]
async fn every_bare_tenant_page_load_redirects_keeping_path_and_query() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        for (_, path) in tenant_routes(&ctx, true) {
            let with_query = format!("{path}?x=1&y=two");
            let res = open(&request, &alice, &with_query).await;
            assert_see_other(&res, &format!("/o/acme{with_query}"), &path);
            let res = request
                .method(Method::HEAD, &path)
                .add_header(alice.0.clone(), alice.1.clone())
                .await;
            assert_see_other(&res, &format!("/o/acme{path}"), &format!("HEAD {path}"));
        }
    })
    .await;
}

/// One of each id-bearing item in Beta, where Alice is an admin.
struct BetaItems {
    paths: Vec<String>,
}

async fn beta_items(request: &TestServer, ctx: &AppContext, alice: &TestMember) -> BetaItems {
    let beta = organisations::Model::find_by_slug(&ctx.db, "beta")
        .await
        .unwrap();
    let in_beta = TestMember::new(alice.cookie.clone(), beta.id, "beta");
    let hx = |req: axum_test::TestRequest| {
        req.add_header(in_beta.0.clone(), in_beta.1.clone())
            .add_header("x-org-id", beta.id.to_string())
            .add_header("hx-request", "true")
    };
    let project = project_in(ctx, "beta", "Bakery").await;
    hx(request.post(&in_beta.at("/tasks")))
        .form(&task_form("Bake", project.id, ""))
        .await;
    let task = tasks::Entity::find().one(&ctx.db).await.unwrap().unwrap();
    hx(request.post(&in_beta.at("/tasks/views")))
        .form(&json!({ "name": "Mine", "scope": "mine" }))
        .await;
    let view = saved_views::Entity::find()
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    let general = conversations::Model::find_general(&ctx.db, beta.id)
        .await
        .unwrap();
    let bob = users::Model::find_by_email(&ctx.db, "bob@example.com")
        .await
        .unwrap();
    let message = messages::Model::create(
        &ctx.db,
        &general,
        bob.id,
        &messages::MessageParams {
            body: "Fresh bread".to_string(),
        },
    )
    .await
    .unwrap();
    let all = folders::Model::list_for_org(&ctx.db, beta.id)
        .await
        .unwrap();
    let folder = all
        .iter()
        .find(|s| !s.folder.is_logins())
        .unwrap()
        .folder
        .clone();
    let logins_folder = all
        .iter()
        .find(|s| s.folder.is_logins())
        .unwrap()
        .folder
        .clone();
    let mut pdf = b"%PDF-1.4\n".to_vec();
    pdf.resize(1024, b' ');
    hx(request.post(&in_beta.at(&format!("/documents/folders/{}/files", folder.id))))
        .multipart(MultipartForm::new().add_part("file", Part::bytes(pdf).file_name("menu.pdf")))
        .await;
    let doc = documents::Entity::find()
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    hx(request.post(&in_beta.at(&format!("/documents/folders/{}/logins", logins_folder.id))))
        .form(&[
            ("title", "Flour mill"),
            ("username", "orders@example.com"),
            ("url", "mill.example.com"),
            ("notes", ""),
            ("password", "pa55word-long"),
        ])
        .await;
    let login = logins::Entity::find().one(&ctx.db).await.unwrap().unwrap();
    let bob_row = memberships::Model::active_in(&ctx.db, beta.id, bob.id)
        .await
        .unwrap()
        .unwrap();
    for (what, org) in [
        (task.organisation_id, beta.id),
        (view.organisation_id, beta.id),
        (doc.organisation_id, beta.id),
        (login.organisation_id, beta.id),
    ] {
        assert_eq!(what, org);
    }
    BetaItems {
        paths: vec![
            format!("/tasks/{}", task.id),
            format!("/tasks/{}/edit", task.id),
            format!("/tasks?open={}", task.id),
            format!("/tasks/views/{}/edit", view.id),
            format!("/chat/{}", general.id),
            format!("/chat?c={}", general.id),
            format!("/chat/{}/threads/{}", general.id, message.id),
            format!("/documents/folders/{}", folder.id),
            format!("/documents/files/{}", doc.id),
            format!("/documents/logins/{}", login.id),
            format!("/members/{}/rotate", bob_row.id),
            format!("/roadmap/projects/{}/edit", project.id),
        ],
    }
}

/// Alice in Acme (owner, `current_org`) and Beta (admin), with one of each
/// item in Beta.
async fn alice_with_beta_items(
    request: &TestServer,
    ctx: &AppContext,
) -> (TestMember, memberships::Model, BetaItems) {
    let (alice, _, in_acme, in_beta) = alice_in_two_orgs(request, ctx).await;
    let mut admin = in_beta.clone().into_active_model();
    admin.role = ActiveValue::Set(memberships::role::ADMIN.to_string());
    let in_beta = admin.update(&ctx.db).await.unwrap();
    let items = beta_items(request, ctx, &alice).await;
    let alice = with_org(ctx, &alice, in_acme.organisation_id).await;
    (alice, in_beta, items)
}

#[tokio::test]
#[serial]
async fn old_item_links_follow_only_active_rows() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, in_beta, items) = alice_with_beta_items(&request, &ctx).await;
        for path in &items.paths {
            let res = open(&request, &alice, path).await;
            assert_see_other(&res, &format!("/o/beta{path}"), path);
        }
        for status in [
            memberships::status::PENDING,
            memberships::status::REMOVED,
            memberships::status::REJECTED,
        ] {
            let row = memberships::Entity::find_by_id(in_beta.id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
            set_status(&ctx, row, status).await;
            for path in &items.paths {
                let res = open(&request, &alice, path).await;
                assert_see_other(&res, &format!("/o/acme{path}"), &format!("{status} {path}"));
            }
        }

        // A super admin acting in Beta follows them there too.
        let admin = super_admin_cookie(&request, &ctx).await;
        let beta = organisations::Model::find_by_slug(&ctx.db, "beta")
            .await
            .unwrap();
        let both = format!("{}; acting_org={}", admin.1.to_str().unwrap(), beta.id);
        let acting = (admin.0.clone(), both.parse().unwrap());
        for path in &items.paths {
            let res = open(&request, &acting, path).await;
            assert_see_other(&res, &format!("/o/beta{path}"), &format!("acting {path}"));
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn an_item_in_a_strangers_org_looks_like_a_missing_id() {
    request::<App, _, _>(|request, ctx| async move {
        let (_alice, _, items) = alice_with_beta_items(&request, &ctx).await;
        let kreepa = join(&request, &ctx, "acme", "kreepa", "kreepa@example.com").await;
        let user = users::Model::find_by_email(&ctx.db, "kreepa@example.com")
            .await
            .unwrap();
        let pending = memberships::Model::default_for_user(&ctx.db, user.id)
            .await
            .unwrap()
            .unwrap();
        approve_as_owner(&ctx, &pending).await;
        for path in &items.paths {
            let res = open(&request, &kreepa, path).await;
            assert_see_other(&res, &format!("/o/acme{path}"), path);
            // The same as an id that exists nowhere.
            let missing = path.replace(|c: char| c.is_ascii_digit(), "9");
            let res = open(&request, &kreepa, &missing).await;
            assert_see_other(&res, &format!("/o/acme{missing}"), &missing);
        }
    })
    .await;
}

/// Rows in every table a tenant write can touch.
async fn row_counts(ctx: &AppContext) -> Vec<u64> {
    let db = &ctx.db;
    vec![
        tasks::Entity::find().count(db).await.unwrap(),
        task_notes::Entity::find().count(db).await.unwrap(),
        messages::Entity::find().count(db).await.unwrap(),
        conversations::Entity::find().count(db).await.unwrap(),
        folders::Entity::find().count(db).await.unwrap(),
        documents::Entity::find().count(db).await.unwrap(),
        logins::Entity::find().count(db).await.unwrap(),
        projects::Entity::find().count(db).await.unwrap(),
        meetings::Entity::find().count(db).await.unwrap(),
        saved_views::Entity::find().count(db).await.unwrap(),
        notifications::Entity::find().count(db).await.unwrap(),
        memberships::Entity::find().count(db).await.unwrap(),
        access_tokens::Entity::find().count(db).await.unwrap(),
        organisations::Entity::find().count(db).await.unwrap(),
    ]
}

#[tokio::test]
#[serial]
async fn every_bare_non_get_is_refused_and_saves_nothing() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let before = row_counts(&ctx).await;
        let routes = tenant_routes(&ctx, false);
        assert!(routes
            .iter()
            .any(|(m, p)| m == Method::POST && p == "/settings/claude/tokens"));
        let form = json!({
            "_org": alice.org_id.to_string(), "org_id": alice.org_id.to_string(),
            "title": "Nope", "name": "Nope", "body": "Nope", "organisation_name": "Nope",
        });
        for (method, path) in routes {
            if path == "/orgs/1/switch" {
                continue; // The old switch: CurrentUser, see orgs.rs.
            }
            let what = format!("{method} {path}");
            let res = request
                .method(method.clone(), &path)
                .add_header(alice.0.clone(), alice.1.clone())
                .add_header("x-org-id", alice.org_id.to_string())
                .add_header("hx-request", "true")
                .form(&form)
                .await;
            assert_eq!(res.status_code(), 409, "{what}: {}", res.text());
            assert_eq!(
                header(&res, "hx-redirect"),
                "/o/acme/dashboard?from=",
                "{what}"
            );
            assert_eq!(header(&res, "cache-control"), "no-store", "{what}");
            let res = request
                .method(method.clone(), &path)
                .add_header(alice.0.clone(), alice.1.clone())
                .form(&form)
                .await;
            assert_eq!(res.status_code(), 303, "{what} plain: {}", res.text());
            assert_eq!(
                header(&res, "location"),
                "/o/acme/dashboard?from=",
                "{what}"
            );
        }
        assert_eq!(row_counts(&ctx).await, before, "nothing was written");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_refused_tab_lands_in_the_org_it_was_showing() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _, in_acme, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let alice = with_org(&ctx, &alice, in_acme.organisation_id).await;
        let beta = in_beta.organisation_id;
        let post = |claim: String| {
            request
                .post("/tasks")
                .add_header(alice.0.clone(), alice.1.clone())
                .add_header("x-org-id", claim)
                .form(&json!({ "title": "Nope" }))
        };
        let res = post(beta.to_string()).await;
        assert_eq!(res.status_code(), 409);
        assert_eq!(header(&res, "hx-redirect"), "/o/beta/dashboard?from=");
        assert!(res.headers().get("x-org-gone").is_none());

        for claim in ["garbage".to_string(), "999999".to_string()] {
            let res = post(claim.clone()).await;
            assert_eq!(
                header(&res, "hx-redirect"),
                "/o/acme/dashboard?from=",
                "{claim}"
            );
        }
        // A stranger's org is never echoed.
        let (_, gamma) = organisations::Model::sign_up(
            &ctx.db,
            &organisations::SignupParams {
                organisation_name: "Gamma".to_string(),
                name: "gina".to_string(),
                email: "gina@example.com".to_string(),
                password: "correct-horse".to_string(),
            },
        )
        .await
        .unwrap();
        let res = post(gamma.id.to_string()).await;
        assert_eq!(header(&res, "hx-redirect"), "/o/acme/dashboard?from=");

        set_status(&ctx, in_beta, memberships::status::REMOVED).await;
        let res = post(beta.to_string()).await;
        assert_eq!(
            header(&res, "hx-redirect"),
            format!("/o/acme/dashboard?from={beta}")
        );
        assert_eq!(header(&res, "x-org-gone"), "1");
        // Home then shows PR 5's notice.
        let home = open(&request, &alice, &format!("/o/acme/dashboard?from={beta}")).await;
        assert!(home.text().contains("You no longer have access to Beta"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn bare_htmx_gets_and_fetches_are_refused() {
    request::<App, _, _>(|request, _ctx| async move {
        let alice = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let res = request
            .get("/tasks/list")
            .add_header(alice.0.clone(), alice.1.clone())
            .add_header("hx-request", "true")
            .await;
        assert_eq!(res.status_code(), 409);
        assert_eq!(header(&res, "hx-redirect"), "/o/acme/dashboard?from=");
        let res = request
            .get("/orgs/current")
            .add_header(alice.0.clone(), alice.1.clone())
            .add_header("x-org-id", alice.org_id.to_string())
            .await;
        assert_eq!(res.status_code(), 409);
        assert_eq!(header(&res, "hx-redirect"), "/o/acme/dashboard?from=");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn bare_redirects_stay_on_this_site() {
    request::<App, _, _>(|request, _ctx| async move {
        let alice = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        for path in [
            "/chat/%2F%2Fevil.com",
            "/tasks?x=//evil.com",
            "/tasks/%5C%5Cevil.com",
        ] {
            let res = open(&request, &alice, path).await;
            assert_eq!(res.status_code(), 303, "{path}");
            assert!(header(&res, "location").starts_with("/o/acme/"), "{path}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_bookmark_of_tasks_opens_the_last_org() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _, in_acme, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let res = open(&request, &alice, "/tasks").await;
        assert_see_other(&res, "/o/acme/tasks", "the default org");
        // Opening Beta by its address remembers it.
        let res = open(&request, &alice, "/o/beta/tasks").await;
        let set = super::multi_org::set_cookie(&res, "current_org").unwrap();
        assert!(set.starts_with(&format!("current_org={};", in_beta.organisation_id)));
        let in_beta_now = with_org(&ctx, &alice, in_beta.organisation_id).await;
        let res = open(&request, &in_beta_now, "/tasks").await;
        assert_see_other(&res, "/o/beta/tasks", "the last org");
        let back = with_org(&ctx, &alice, in_acme.organisation_id).await;
        let res = open(&request, &back, "/tasks?view=board").await;
        assert_see_other(&res, "/o/acme/tasks?view=board", "and back");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn old_email_links_still_land() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _, items) = alice_with_beta_items(&request, &ctx).await;
        let task = items.paths[2].clone(); // /tasks?open=N
        let res = open(&request, &alice, &task).await;
        assert_see_other(&res, &format!("/o/beta{task}"), "a mention in Beta");
        for path in [
            "/roadmap?lane=now",
            "/notifications",
            "/documents/activity",
            "/dashboard",
        ] {
            let res = open(&request, &alice, path).await;
            assert_see_other(&res, &format!("/o/acme{path}"), path);
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn signed_out_links_go_to_sign_in_as_today() {
    request::<App, _, _>(|request, _ctx| async move {
        sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        for path in [
            "/tasks?open=1",
            "/chat/1",
            "/o/acme/tasks",
            "/o/made-up-name/tasks",
        ] {
            let res = request.get(path).add_header("accept", "text/html").await;
            assert_eq!(res.status_code(), 303, "{path}");
            assert_eq!(header(&res, "location"), "/login", "{path}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_allow_page_is_still_bare() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _, _, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let client = register(&request, CLAUDE).await;
        let url = authorize_url(&client, CLAUDE);
        let page = open(&request, &alice, &url).await;
        assert_eq!(page.status_code(), 200, "{}", page.text());
        assert!(page.text().contains("Acme"));
        // It follows the last org until PR 3's picker.
        let in_beta = with_org(&ctx, &alice, in_beta.organisation_id).await;
        let page = open(&request, &in_beta, &url).await;
        assert!(page.text().contains("Beta"));
        let under_o = open(&request, &alice, &format!("/o/acme{url}")).await;
        assert_eq!(under_o.status_code(), 404);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_in_org_404_has_no_other_org_hint() {
    request::<App, _, _>(|request, _ctx| async move {
        let alice = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let res = open(&request, &alice, &alice.at("/tasks/999999")).await;
        assert_eq!(res.status_code(), 404);
        let html = res.text();
        assert!(html.contains("It may have been deleted, or the link may be wrong."));
        assert!(!html.contains("another of your organisations"));
    })
    .await;
}
