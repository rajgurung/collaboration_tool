use collab::{app::App, models::users};
use loco_rs::{boot::run_task, task, testing::prelude::*};
use sea_orm::IntoActiveModel;
use serial_test::serial;

use super::{
    multi_org::{clears, set_cookie},
    prepare_data::{login, open_page, sign_up_member, AsMember},
};

#[tokio::test]
#[serial]
async fn admin_is_hidden_from_normal_users() {
    request::<App, _, _>(|request, _ctx| async move {
        let cookie = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let res = request
            .get("/admin")
            .add_header(cookie.0.clone(), cookie.1.clone())
            .await;
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

        let (res, _) = open_page(&request, &cookie, "/dashboard").await;
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
) -> super::prepare_data::Cookie {
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
        sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        super::prepare_data::join(&request, &ctx, "acme", "bob", "bob@example.com").await;
        let admin = super_admin_cookie(&request, &ctx).await;
        let body = request
            .get("/admin")
            .add_header(admin.0.clone(), admin.1.clone())
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
        sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
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
        assert_eq!(enter.headers()["location"], "/o/acme/dashboard");
        let page = request
            .get("/o/acme/dashboard")
            .add_header("cookie", both.clone())
            .await;
        assert_eq!(page.status_code(), 200);
        let body = page.text();
        assert!(body.contains("Acme"));
        assert!(body.contains("as platform admin"));

        // Acting as owner: the admin can see Acme's members page.
        let members = request
            .get("/o/acme/members")
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
        let alice = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        sign_up_member(&request, "Globex", "gina", "gina@example.com").await;
        let globex = collab::models::organisations::Model::find_by_slug(&ctx.db, "globex")
            .await
            .unwrap();

        let forged = format!("{}; acting_org={}", alice.1.to_str().unwrap(), globex.id);
        let page = request
            .get("/o/acme/dashboard")
            .add_header("cookie", forged.clone())
            .await;
        assert_eq!(page.status_code(), 200);
        assert!(page.text().contains("Acme"));
        assert!(!page.text().contains("Globex"));
        let page = request
            .get("/o/globex/dashboard")
            .add_header("cookie", forged)
            .await;
        assert_eq!(page.status_code(), 404);

        for path in [
            format!("/admin/orgs/{}/enter", globex.id),
            "/admin/leave".to_string(),
        ] {
            let res = request.post(&alice.to(&path)).as_member(&alice).await;
            assert_eq!(res.status_code(), 404, "{path}");
        }
    })
    .await;
}

async fn org_id(ctx: &loco_rs::app::AppContext, slug: &str) -> i64 {
    collab::models::organisations::Model::find_by_slug(&ctx.db, slug)
        .await
        .unwrap()
        .id
}

#[tokio::test]
#[serial]
async fn acting_wins_over_current_org() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let admin = super_admin_cookie(&request, &ctx).await;
        let (acme, own) = (
            org_id(&ctx, "acme").await,
            org_id(&ctx, "himalayan-ritual").await,
        );
        let both = format!(
            "{}; current_org={own}; acting_org={acme}",
            admin.1.to_str().unwrap()
        );
        let page = request
            .get("/o/acme/dashboard")
            .add_header("cookie", both)
            .await;
        assert_eq!(page.status_code(), 200);
        let body = page.text();
        assert!(body.contains("as platform admin"));
        assert!(body.contains("Acme"));
        assert!(set_cookie(&page, "current_org").is_none());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn acting_with_a_stale_current_org_sends_no_clearing_cookie() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        sign_up_member(&request, "Globex", "gina", "gina@example.com").await;
        let admin = super_admin_cookie(&request, &ctx).await;
        let (acme, globex) = (org_id(&ctx, "acme").await, org_id(&ctx, "globex").await);
        // The admin is not a member of Globex, so current_org is stale.
        let both = format!(
            "{}; current_org={globex}; acting_org={acme}",
            admin.1.to_str().unwrap()
        );
        let page = request
            .get("/o/acme/dashboard")
            .add_header("cookie", both)
            .await;
        assert_eq!(page.status_code(), 200);
        assert!(page.text().contains("as platform admin"));
        assert!(set_cookie(&page, "current_org").is_none());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn switching_ends_acting() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let admin = super_admin_cookie(&request, &ctx).await;
        let (acme, own) = (
            org_id(&ctx, "acme").await,
            org_id(&ctx, "himalayan-ritual").await,
        );
        let acting = format!("{}; acting_org={acme}", admin.1.to_str().unwrap());
        let res = request
            .post(&format!("/orgs/{own}/switch"))
            .add_header("cookie", acting)
            .await;
        assert_eq!(res.status_code(), 303);
        assert!(clears(&res, "acting_org"));
        let set = set_cookie(&res, "current_org").unwrap();
        assert!(set.starts_with(&format!("current_org={own};")), "{set}");

        // Acting does not make the admin a member: switching into Acme is a 404.
        let res = request
            .post(&format!("/orgs/{acme}/switch"))
            .add_header(admin.0.clone(), admin.1.clone())
            .await;
        assert_eq!(res.status_code(), 404);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_super_admin_without_a_membership_gets_the_clearing_cookie_on_the_htmx_redirect() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let acme = org_id(&ctx, "acme").await;
        super::prepare_data::create_user(&ctx, "root@example.com", "root")
            .await
            .into_active_model()
            .make_super_admin(&ctx.db)
            .await
            .unwrap();
        let root = login(
            &request,
            "root@example.com",
            super::prepare_data::USER_PASSWORD,
        )
        .await;
        let stale = format!("{}; current_org={acme}", root.1.to_str().unwrap());
        let res = request
            .get("/dashboard")
            .add_header("cookie", stale)
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(res.headers()["HX-Redirect"], "/admin");
        assert!(clears(&res, "current_org"));
    })
    .await;
}

/// The admin page belongs to no org: no badge polls a bare address, and no
/// notification stream opens, so nothing bounces the admin to Home.
#[tokio::test]
#[serial]
async fn the_admin_page_polls_no_bare_badges() {
    request::<App, _, _>(|request, ctx| async move {
        let admin = super_admin_cookie(&request, &ctx).await;
        let page = request
            .get("/admin")
            .add_header(admin.0.clone(), admin.1.clone())
            .await;
        assert_eq!(page.status_code(), 200);
        let html = page.text();
        assert!(!html.contains("/notifications/unread"), "{html}");
        assert!(!html.contains("/chat/unread"), "{html}");
        // app.ts opens the stream only on a page with an org and a badge.
        assert!(!html.contains("data-org-id="), "{html}");
    })
    .await;
}
