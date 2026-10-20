//! The organisation in the address: `/o/{slug}/...` picks the org for that
//! request, whatever the `current_org` cookie says. Alice is in Acme (owner)
//! and Beta (member), Kreepa only in Acme.
use axum::http::{HeaderValue, Method};
use axum_test::multipart::{MultipartForm, Part};
use collab::{
    app::App,
    data::settings::Settings,
    models::{access_tokens, documents, folders, memberships, organisations, tasks, users},
    views::links::TENANT_SECTIONS,
};
use loco_rs::{
    app::{AppContext, Hooks},
    prelude::*,
    TestServer,
};
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;
use serial_test::serial;

use super::{
    admin::super_admin_cookie,
    multi_org::{alice_in_two_orgs, set_cookie, set_status, with_org},
    prepare_data::{approve_as_owner, join, Cookie, TestMember},
    tasks::{project_in, task_form},
};

struct World {
    /// Alice, with `current_org` on Beta.
    alice: TestMember,
    kreepa: Cookie,
    acme: organisations::Model,
    beta: organisations::Model,
    in_beta: memberships::Model,
}

async fn world(request: &TestServer, ctx: &AppContext) -> World {
    let (alice, _, _, in_beta) = alice_in_two_orgs(request, ctx).await;
    let alice = with_org(ctx, &alice, in_beta.organisation_id).await;
    let kreepa = join(request, ctx, "acme", "kreepa", "kreepa@example.com").await;
    let user = users::Model::find_by_email(&ctx.db, "kreepa@example.com")
        .await
        .unwrap();
    let pending = memberships::Model::default_for_user(&ctx.db, user.id)
        .await
        .unwrap()
        .unwrap();
    approve_as_owner(ctx, &pending).await;
    World {
        alice,
        kreepa: kreepa.cookie,
        acme: organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap(),
        beta: organisations::Model::find_by_slug(&ctx.db, "beta")
            .await
            .unwrap(),
        in_beta,
    }
}

fn header(res: &axum_test::TestResponse, name: &str) -> String {
    res.headers()
        .get(name)
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default()
}

fn page(request: &TestServer, path: &str, cookie: &Cookie) -> axum_test::TestRequest {
    request
        .get(path)
        .add_header(cookie.0.clone(), cookie.1.clone())
        .add_header("accept", "text/html")
}

/// A cookie header with another cookie added to it.
fn plus(cookie: &Cookie, extra: &str) -> Cookie {
    let both = format!("{}; {extra}", cookie.1.to_str().unwrap());
    (cookie.0.clone(), HeaderValue::from_str(&both).unwrap())
}

/// Every route under a tenant section, with path parameters filled with `1`.
fn tenant_routes(ctx: &AppContext) -> Vec<(Method, String)> {
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
            all.push((method, path.clone()));
        }
    }
    all
}

#[tokio::test]
#[serial]
async fn path_parameters_reach_the_handlers() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let project = project_in(&ctx, "acme", "Launch").await;
        let res = request
            .post("/o/acme/tasks")
            .add_header(w.alice.0.clone(), w.alice.1.clone())
            .add_header("x-org-id", w.acme.id.to_string())
            .add_header("hx-request", "true")
            .form(&task_form("Write the brief", project.id, ""))
            .await;
        assert!(res.status_code().is_success(), "{}", res.text());
        let task = tasks::Entity::find().one(&ctx.db).await.unwrap().unwrap();

        let res = page(&request, &format!("/o/acme/tasks/{}", task.id), &w.alice).await;
        assert_eq!(res.status_code(), 200);
        assert!(res.text().contains("Write the brief"));
        let res = page(
            &request,
            &format!("/o/acme/roadmap/projects/{}/edit", project.id),
            &w.alice,
        )
        .await;
        assert_eq!(res.status_code(), 200);
        assert!(res.text().contains("Launch"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn two_tabs_write_into_their_own_org() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let in_acme = project_in(&ctx, "acme", "Acme launch").await;
        let in_beta = project_in(&ctx, "beta", "Beta launch").await;
        for (slug, org, project, title) in [
            ("acme", &w.acme, &in_acme, "Acme task"),
            ("beta", &w.beta, &in_beta, "Beta task"),
        ] {
            let res = request
                .post(&format!("/o/{slug}/tasks"))
                .add_header(w.alice.0.clone(), w.alice.1.clone())
                .add_header("x-org-id", org.id.to_string())
                .add_header("hx-request", "true")
                .form(&task_form(title, project.id, ""))
                .await;
            assert!(res.status_code().is_success(), "{slug}: {}", res.text());
            assert!(res.headers().get("hx-redirect").is_none(), "{slug}");
        }
        for (title, org) in [("Acme task", &w.acme), ("Beta task", &w.beta)] {
            let task = tasks::Entity::find()
                .filter(tasks::Column::Title.eq(title))
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(task.organisation_id, org.id, "{title}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_cookie_never_overrides_the_address() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        for (path, name, id) in [
            ("/o/acme/tasks", "Acme", w.acme.id),
            ("/o/beta/tasks", "Beta", w.beta.id),
        ] {
            let res = page(&request, path, &w.alice).await;
            assert_eq!(res.status_code(), 200, "{path}");
            let html = res.text();
            assert!(html.contains(&format!("Tasks · {name}")), "{path}");
            assert_eq!(super::prepare_data::page_org_id(&html), Some(id), "{path}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_strangers_org_and_a_made_up_slug_look_identical() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let admin = super_admin_cookie(&request, &ctx).await;
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
        let acting = plus(&admin, &format!("acting_org={}", gamma.id));
        let origin = Settings::from_context(&ctx).unwrap().app_url;
        let who = [("kreepa", &w.kreepa), ("admin", &admin), ("acting", &acting)];
        let routes = tenant_routes(&ctx);
        assert!(routes
            .iter()
            .any(|(m, p)| m == Method::POST && p == "/settings/claude/tokens"));
        for (name, cookie) in who {
            for (method, path) in &routes {
                let mut seen = Vec::new();
                for slug in ["beta", "made-up-name"] {
                    let url = format!("/o/{slug}{path}");
                    let shapes: Vec<axum_test::TestRequest> = if method == Method::GET {
                        vec![
                            page(&request, &url, cookie),
                            request
                                .get(&url)
                                .add_header(cookie.0.clone(), cookie.1.clone())
                                .add_header("hx-request", "true"),
                            request
                                .method(Method::HEAD, &url)
                                .add_header(cookie.0.clone(), cookie.1.clone()),
                        ]
                    } else {
                        vec![request
                            .method(method.clone(), &url)
                            .add_header(cookie.0.clone(), cookie.1.clone())
                            .add_header("origin", origin.clone())
                            .form(&json!({ "_org": w.beta.id.to_string(), "org_id": w.beta.id.to_string() }))]
                    };
                    let mut answers = Vec::new();
                    for req in shapes {
                        let res = req.await;
                        answers.push((
                            res.status_code(),
                            res.text(),
                            res.headers()
                                .get_all("set-cookie")
                                .iter()
                                .map(|v| v.to_str().unwrap().to_string())
                                .collect::<Vec<_>>(),
                            header(&res, "cache-control"),
                            header(&res, "hx-redirect"),
                        ));
                    }
                    seen.push(answers);
                }
                assert_eq!(seen[0], seen[1], "{name} {method} {path}");
                assert!(
                    !seen[0].iter().any(|a| a.1.contains("Beta")),
                    "{name} {method} {path} names Beta"
                );
                if method == Method::GET {
                    assert_eq!(seen[0][0].0, 404, "{name} {path}");
                    assert_eq!(seen[0][1].0, 409, "{name} {path}");
                    assert_eq!(seen[0][1].4, "/dashboard", "{name} {path}");
                }
            }
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn public_routes_are_404_under_o() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        for path in [
            "/login",
            "/signup",
            "/forgot",
            "/join/acme",
            "/admin",
            "/oauth/authorize",
            "/.well-known/oauth-authorization-server",
            "/_health",
            "/robots.txt",
            "/o/acme/dashboard",
        ] {
            let res = page(&request, &format!("/o/acme{path}"), &w.alice).await;
            assert_eq!(res.status_code(), 404, "{path}");
        }
        for path in [
            "/logout",
            "/mcp",
            "/account/sign-out-everywhere",
            "/admin/leave",
        ] {
            let res = request
                .post(&format!("/o/acme{path}"))
                .add_header(w.alice.0.clone(), w.alice.1.clone())
                .await;
            assert_eq!(res.status_code(), 404, "{path}");
        }
        // And they still work bare.
        assert_eq!(request.get("/login").await.status_code(), 200);
        assert_eq!(request.get("/join/acme").await.status_code(), 200);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn o_slug_alone_goes_to_home_for_get_only() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        for path in ["/o/beta", "/o/beta/"] {
            let res = page(&request, path, &w.alice).await;
            assert_eq!(res.status_code(), 303, "{path}");
            assert_eq!(header(&res, "location"), "/o/beta/dashboard");
            assert_eq!(header(&res, "cache-control"), "no-store");
            let res = request
                .post(path)
                .add_header(w.alice.0.clone(), w.alice.1.clone())
                .await;
            assert_eq!(res.status_code(), 404, "{path}");
        }
    })
    .await;
}

/// Adds Alice to a new org Gamma as a member.
async fn alice_in_gamma(ctx: &AppContext) -> organisations::Model {
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
    let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
        .await
        .unwrap();
    memberships::ActiveModel {
        user_id: ActiveValue::Set(alice.id),
        username: ActiveValue::Set("alice".to_string()),
        role: ActiveValue::Set(memberships::role::MEMBER.to_string()),
        status: ActiveValue::Set(memberships::status::ACTIVE.to_string()),
        ..Default::default()
    }
    .set_tenant(gamma.id)
    .unwrap()
    .insert(&ctx.db)
    .await
    .unwrap();
    gamma
}

#[tokio::test]
#[serial]
async fn a_pending_member_sees_that_orgs_waiting_page() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        set_status(&ctx, w.in_beta.clone(), memberships::status::PENDING).await;

        // One other org: "Open Acme" first, "Check again" second.
        let res = page(&request, "/o/beta/tasks?view=board", &w.alice).await;
        assert_eq!(res.status_code(), 403);
        let html = res.text();
        assert!(html.contains("Waiting for approval · Beta"));
        let open = html.find(r#"href="/o/acme/dashboard""#).expect("Open Acme");
        let again = html
            .find(r#"href="/o/beta/tasks?view=board""#)
            .expect("Check again reloads the address");
        assert!(open < again);
        assert!(html.contains("Open Acme"));
        assert!(html.contains("Signed in as alice"));
        assert!(!html.contains("Your other organisations"));
        assert!(set_cookie(&res, "current_org").is_none());

        // Two others: the list, the last used (the cookie's) first.
        let gamma = alice_in_gamma(&ctx).await;
        let in_gamma = with_org(&ctx, &w.alice, gamma.id).await;
        let html = page(&request, "/o/beta/tasks", &in_gamma).await.text();
        assert!(html.contains("Your other organisations"));
        let g = html.find(r#"href="/o/gamma/dashboard""#).unwrap();
        let a = html.find(r#"href="/o/acme/dashboard""#).unwrap();
        assert!(g < a, "last used first");
        assert!(html.contains("Last used"));
        assert!(
            html.contains(">owner<") && html.contains(">member<"),
            "roles"
        );

        // Over HTMX: to Beta's own Home, which draws this page.
        let res = request
            .get("/o/beta/tasks")
            .add_header(w.alice.0.clone(), w.alice.1.clone())
            .add_header("hx-request", "true")
            .await;
        assert_eq!(res.status_code(), 409);
        assert_eq!(header(&res, "hx-redirect"), "/o/beta/dashboard");
        assert_eq!(header(&res, "x-org-gone"), "1");
        assert_eq!(header(&res, "cache-control"), "no-store");

        for (status, title) in [
            (memberships::status::REMOVED, "You were removed from Beta"),
            (memberships::status::REJECTED, "Request declined"),
        ] {
            let row = memberships::Entity::find_by_id(w.in_beta.id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
            set_status(&ctx, row, status).await;
            let res = page(&request, "/o/beta/tasks", &in_gamma).await;
            assert_eq!(res.status_code(), 403, "{status}");
            let html = res.text();
            assert!(html.contains(title), "{status}");
            assert!(html.contains("Your other organisations"), "{status}");
            assert!(!html.contains("Check again"), "{status}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_signed_in_not_found_names_only_the_user() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let res = page(&request, "/o/beta/tasks", &w.kreepa).await;
        assert_eq!(res.status_code(), 404);
        let html = res.text();
        assert!(html.contains("We couldn't find that page"));
        assert!(html.contains("Signed in as <strong>kreepa@example.com</strong>"));
        assert!(html.contains("Use another account"));
        assert!(html.contains(r#"href="/dashboard""#));
        for org in ["Beta", "Acme", "data-org-id"] {
            assert!(!html.contains(org), "{org}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_in_org_404_goes_back_to_its_section() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let html = page(&request, "/o/acme/tasks/999999", &w.alice)
            .await
            .text();
        assert!(html.contains("We couldn't find that in Acme"));
        assert!(html.contains(r#"href="/o/acme/tasks" class="btn btn-primary"#));
        assert!(html.contains("Back to Tasks"));

        let res = page(&request, "/o/acme/documents/folders/999999", &w.alice).await;
        assert_eq!(res.status_code(), 404);
        assert!(res.text().contains("Back to Documents"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_super_admin_with_no_active_row_goes_to_admin() {
    request::<App, _, _>(|request, ctx| async move {
        let admin = super_admin_cookie(&request, &ctx).await;
        let user = users::Model::find_by_email(&ctx.db, "gurungraj26@gmail.com")
            .await
            .unwrap();
        let own = memberships::Model::default_for_user(&ctx.db, user.id)
            .await
            .unwrap()
            .unwrap();
        let org = organisations::Model::find_by_id(&ctx.db, own.organisation_id)
            .await
            .unwrap();
        set_status(&ctx, own, memberships::status::PENDING).await;
        let res = page(&request, &format!("/o/{}/tasks", org.slug), &admin).await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(header(&res, "location"), "/admin");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn acting_applies_only_to_the_acting_orgs_address() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let admin = super_admin_cookie(&request, &ctx).await;
        let acting = plus(&admin, &format!("acting_org={}", w.acme.id));
        let user = users::Model::find_by_email(&ctx.db, "gurungraj26@gmail.com")
            .await
            .unwrap();
        let own = memberships::Model::default_for_user(&ctx.db, user.id)
            .await
            .unwrap()
            .unwrap();
        let own = organisations::Model::find_by_id(&ctx.db, own.organisation_id)
            .await
            .unwrap();

        let res = page(&request, "/o/acme/tasks", &acting).await;
        assert_eq!(res.status_code(), 200);
        assert!(res.text().contains("as platform admin"));
        assert!(
            set_cookie(&res, "current_org").is_none(),
            "acting is never remembered"
        );

        let res = page(&request, &format!("/o/{}/tasks", own.slug), &acting).await;
        assert_eq!(res.status_code(), 200);
        assert!(!res.text().contains("as platform admin"));

        // Not acting there: the address never starts acting.
        let res = page(&request, "/o/beta/tasks", &acting).await;
        assert_eq!(res.status_code(), 404);
        let res = page(&request, "/o/acme/tasks", &admin).await;
        assert_eq!(res.status_code(), 404);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_claim_for_another_org_is_refused_to_the_address_orgs_home() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let project = project_in(&ctx, "acme", "Launch").await;
        let res = request
            .post("/o/acme/tasks")
            .add_header(w.alice.0.clone(), w.alice.1.clone())
            .add_header("x-org-id", w.beta.id.to_string())
            .add_header("hx-request", "true")
            .form(&task_form("Nope", project.id, ""))
            .await;
        assert_eq!(res.status_code(), 409);
        assert_eq!(
            header(&res, "hx-redirect"),
            format!("/o/acme/dashboard?from={}", w.beta.id)
        );
        let res = request
            .post("/o/acme/tasks")
            .add_header(w.alice.0.clone(), w.alice.1.clone())
            .form(&json!({ "_org": "garbage", "title": "Nope" }))
            .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(header(&res, "location"), "/o/acme/dashboard?from=");
        assert!(tasks::Entity::find().one(&ctx.db).await.unwrap().is_none());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_same_origin_page_load_remembers_the_org() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        for site in [Some("same-origin"), Some("none"), None] {
            let mut req = page(&request, "/o/acme/tasks", &w.alice);
            if let Some(site) = site {
                req = req.add_header("sec-fetch-site", site);
            }
            let res = req.await;
            let set = set_cookie(&res, "current_org").unwrap_or_else(|| panic!("{site:?}"));
            assert!(
                set.starts_with(&format!("current_org={};", w.acme.id)),
                "{set}"
            );
            for attr in ["HttpOnly", "SameSite=Lax", "Path=/", "Max-Age="] {
                assert!(set.contains(attr), "{set} {attr}");
            }
        }
        // Already there: nothing to set.
        let res = page(&request, "/o/beta/tasks", &w.alice).await;
        assert!(set_cookie(&res, "current_org").is_none());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_cross_site_navigation_sets_no_cookie() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        for site in ["cross-site", "same-site"] {
            let res = page(&request, "/o/acme/tasks", &w.alice)
                .add_header("sec-fetch-site", site)
                .await;
            assert_eq!(res.status_code(), 200, "{site}");
            assert!(set_cookie(&res, "current_org").is_none(), "{site}");
        }
        let res = request
            .get("/o/acme/tasks")
            .add_header(w.alice.0.clone(), w.alice.1.clone())
            .add_header("hx-request", "true")
            .add_header("x-org-id", w.acme.id.to_string())
            .await;
        assert!(set_cookie(&res, "current_org").is_none(), "htmx");
        let project = project_in(&ctx, "acme", "Launch").await;
        let res = request
            .post("/o/acme/tasks")
            .add_header(w.alice.0.clone(), w.alice.1.clone())
            .add_header("x-org-id", w.acme.id.to_string())
            .add_header("hx-request", "true")
            .form(&task_form("Saved", project.id, ""))
            .await;
        assert!(res.status_code().is_success());
        assert!(set_cookie(&res, "current_org").is_none(), "post");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_token_form_pin_is_checked_against_the_address() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let res = request
            .post("/o/acme/settings/claude/tokens")
            .add_header(w.alice.0.clone(), w.alice.1.clone())
            .form(&json!({ "name": "Laptop", "org_id": w.beta.id.to_string() }))
            .await;
        assert_eq!(res.status_code(), 409);
        assert!(!res.text().contains("collab_pat_"));
        let res = request
            .post("/o/acme/settings/claude/tokens")
            .add_header(w.alice.0.clone(), w.alice.1.clone())
            .form(&json!({ "name": "Laptop", "org_id": w.acme.id.to_string() }))
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        assert!(res.text().contains("collab_pat_"));
        let token = access_tokens::Entity::find()
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(token.organisation_id, w.acme.id);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_switch_is_404_under_an_org_address() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let origin = Settings::from_context(&ctx).unwrap().app_url;
        let res = request
            .post(&format!("/o/acme/orgs/{}/switch", w.acme.id))
            .add_header(w.alice.0.clone(), w.alice.1.clone())
            .add_header("origin", origin)
            .await;
        assert_eq!(res.status_code(), 404);
        assert!(res.headers().get("set-cookie").is_none());
    })
    .await;
}

/// `PathOrg` decides which org a request works in, so only the resolver (and
/// the old switch, to refuse itself) may read it.
#[test]
fn only_the_resolver_reads_path_org() {
    let allowed = [
        "src/initializers/org_path.rs",
        "src/extractors/current_member.rs",
        "src/controllers/orgs.rs",
    ];
    let mut offenders = Vec::new();
    let mut dirs = vec![std::path::PathBuf::from("src")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let name = path.to_string_lossy().replace('\\', "/");
                let text = std::fs::read_to_string(&path).unwrap();
                if text.contains("PathOrg") && !allowed.contains(&name.as_str()) {
                    offenders.push(name);
                }
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:?}");
    let orgs = std::fs::read_to_string("src/controllers/orgs.rs").unwrap();
    assert_eq!(
        orgs.matches("PathOrg").count(),
        2,
        "orgs.rs only imports it and refuses with it"
    );
}

#[tokio::test]
#[serial]
async fn an_upload_under_o_keeps_its_body_limit() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let company = folders::Model::list_for_org(&ctx.db, w.acme.id)
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.folder.name == "Company")
            .unwrap()
            .folder;
        // Bigger than Loco's 2 MB limit for every other route.
        let mut three_mb = b"%PDF-1.4\n".to_vec();
        three_mb.resize(3 * 1024 * 1024, b' ');
        let res = request
            .post(&format!("/o/acme/documents/folders/{}/files", company.id))
            .add_header(w.alice.0.clone(), w.alice.1.clone())
            .add_header("x-org-id", w.acme.id.to_string())
            .multipart(
                MultipartForm::new().add_part("file", Part::bytes(three_mb).file_name("big.pdf")),
            )
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let doc = documents::Entity::find()
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(doc.organisation_id, w.acme.id);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn robots_txt_disallows_app_paths_not_join() {
    request::<App, _, _>(|request, _ctx| async move {
        let res = request.get("/robots.txt").await;
        assert_eq!(res.status_code(), 200);
        assert!(header(&res, "content-type").starts_with("text/plain"));
        let body = res.text();
        for line in [
            "User-agent: *",
            "Disallow: /o/",
            "Disallow: /oauth/",
            "Disallow: /admin",
            "Disallow: /mcp",
        ] {
            assert!(body.lines().any(|l| l == line), "{line}");
        }
        assert!(!body.contains("/join"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn every_response_but_the_homepage_is_noindex() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let res = request.get("/").await;
        assert_eq!(res.status_code(), 200);
        assert!(res.headers().get("x-robots-tag").is_none());

        let company = folders::Model::list_for_org(&ctx.db, w.acme.id)
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.folder.name == "Company")
            .unwrap()
            .folder;
        let mut pdf = b"%PDF-1.4\n".to_vec();
        pdf.resize(1024, b' ');
        request
            .post(&format!("/o/acme/documents/folders/{}/files", company.id))
            .add_header(w.alice.0.clone(), w.alice.1.clone())
            .add_header("x-org-id", w.acme.id.to_string())
            .multipart(MultipartForm::new().add_part("file", Part::bytes(pdf).file_name("a.pdf")))
            .await;
        let doc = documents::Entity::find()
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();

        let responses = [
            page(&request, "/o/acme/tasks", &w.alice).await,
            page(&request, "/o/acme", &w.alice).await,
            page(&request, "/o/acme/tasks/999999", &w.alice).await,
            page(
                &request,
                &format!("/o/acme/documents/files/{}", doc.id),
                &w.alice,
            )
            .await,
            request.get("/login").await,
            request.get("/join/acme").await,
            request.get("/dashboard").await,
            request.get("/no-such-page").await,
        ];
        for res in responses {
            assert_eq!(
                header(&res, "x-robots-tag"),
                "noindex",
                "{}",
                res.status_code()
            );
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn every_response_has_referrer_policy_same_origin() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let responses = [
            request.get("/").await,
            request.get("/login").await,
            request.get("/robots.txt").await,
            page(&request, "/o/acme/tasks", &w.alice).await,
            page(&request, "/o/made-up-name/tasks", &w.alice).await,
            request.post("/logout").await,
        ];
        for res in responses {
            assert_eq!(
                header(&res, "referrer-policy"),
                "same-origin",
                "{}",
                res.status_code()
            );
        }
        // Nothing else sets one, so there is exactly one.
        let res = request.get("/").await;
        assert_eq!(res.headers().get_all("referrer-policy").iter().count(), 1);
    })
    .await;
}
