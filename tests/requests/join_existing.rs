//! Joining another organisation with the account you have. Alice owns Acme;
//! Bob owns Beta and shares its link, `/join/beta`.
use collab::{
    app::App,
    models::{access_tokens, memberships, notifications, organisations, users},
};
use loco_rs::{app::AppContext, testing::prelude::*, TestServer};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, IntoActiveModel, PaginatorTrait,
    QueryFilter,
};
use serde_json::json;
use serial_test::serial;

use super::{
    multi_org::{alice_in_two_orgs, clears, org_heading, set_cookie, with_org},
    prepare_data::{
        create_user, join, login, open_page, sign_up_member, AsMember, Cookie, TestMember,
        USER_PASSWORD,
    },
};

struct World {
    /// Alice's cookie, drawn for Acme.
    alice: TestMember,
    /// Bob's cookie, drawn for Beta.
    bob: TestMember,
    alice_id: i64,
    acme: i64,
    beta: i64,
}

async fn world(request: &TestServer, ctx: &AppContext) -> World {
    let alice = sign_up_member(request, "Acme", "alice", "alice@example.com").await;
    let bob = sign_up_member(request, "Beta", "bob", "bob@example.com").await;
    World {
        alice_id: user_id(ctx, "alice@example.com").await,
        acme: alice.org_id,
        beta: bob.org_id,
        alice,
        bob,
    }
}

async fn user_id(ctx: &AppContext, email: &str) -> i64 {
    users::Model::find_by_email(&ctx.db, email)
        .await
        .unwrap()
        .id
}

/// A page load. A bare app address is followed into the org it redirects
/// to, as a browser follows it.
async fn get(request: &TestServer, who: Option<&Cookie>, path: &str) -> axum_test::TestResponse {
    match who {
        Some(cookie) if !path.starts_with("/join/") => open_page(request, cookie, path).await.0,
        Some(cookie) => {
            request
                .get(path)
                .add_header(cookie.0.clone(), cookie.1.clone())
                .await
        }
        None => request.get(path).await,
    }
}

/// "Ask to join" from `who`'s browser, as a plain form.
async fn ask(
    request: &TestServer,
    who: &Cookie,
    slug: &str,
    name: &str,
) -> axum_test::TestResponse {
    request
        .post(&format!("/join/{slug}/request"))
        .add_header(who.0.clone(), who.1.clone())
        .form(&json!({ "name": name }))
        .await
}

async fn row(ctx: &AppContext, org_id: i64, user_id: i64) -> Option<memberships::Model> {
    memberships::Model::in_org_for_user(&ctx.db, org_id, user_id)
        .await
        .unwrap()
}

async fn membership_count(ctx: &AppContext) -> u64 {
    memberships::Entity::find().count(&ctx.db).await.unwrap()
}

fn header(res: &axum_test::TestResponse, name: &str) -> String {
    res.headers()
        .get(name)
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default()
}

fn assert_guarded(res: &axum_test::TestResponse, what: &str) {
    assert_eq!(header(res, "x-frame-options"), "DENY", "{what}");
    assert_eq!(
        header(res, "content-security-policy"),
        "frame-ancestors 'none'",
        "{what}"
    );
    assert_eq!(header(res, "cache-control"), "no-store", "{what}");
}

/// A members-page action by `who`, over HTMX, as the page sends it.
async fn manage(
    request: &TestServer,
    who: &TestMember,
    action: &str,
    id: i64,
) -> axum_test::TestResponse {
    request
        .post(&who.at(&format!("/members/{id}/{action}")))
        .as_member(who)
        .add_header("HX-Request", "true")
        .await
}

fn toast(res: &axum_test::TestResponse) -> String {
    header(res, "HX-Trigger")
}

/// Alice asks to join Beta, and Bob approves.
async fn alice_joins_beta(request: &TestServer, ctx: &AppContext, w: &World) -> memberships::Model {
    let res = ask(request, &w.alice, "beta", "alice").await;
    assert_eq!(res.status_code(), 303, "{}", res.text());
    let pending = row(ctx, w.beta, w.alice_id).await.unwrap();
    let res = manage(request, &w.bob, "approve", pending.id).await;
    assert_eq!(res.status_code(), 200, "{}", res.text());
    row(ctx, w.beta, w.alice_id).await.unwrap()
}

fn deliveries(ctx: &AppContext) -> Vec<String> {
    ctx.mailer
        .as_ref()
        .unwrap()
        .deliveries()
        .messages
        .iter()
        .map(|m| m.replace("=\r\n", "").replace("=3D", "="))
        .collect()
}

fn approval_emails(ctx: &AppContext) -> Vec<String> {
    deliveries(ctx)
        .into_iter()
        .filter(|m| m.contains("Subject: Your join request was approved"))
        .collect()
}

#[tokio::test]
#[serial]
async fn signed_in_join_page_asks_only_for_a_username() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        // Alice is also "ally" in Gamma, where this browser is working.
        sign_up_member(&request, "Gamma", "gail", "gail@example.com").await;
        let gamma = organisations::Model::find_by_slug(&ctx.db, "gamma")
            .await
            .unwrap();
        let ally = memberships::ActiveModel {
            user_id: ActiveValue::Set(w.alice_id),
            username: ActiveValue::Set("ally".to_string()),
            role: ActiveValue::Set(memberships::role::MEMBER.to_string()),
            status: ActiveValue::Set(memberships::status::ACTIVE.to_string()),
            ..Default::default()
        };
        loco_rs::prelude::TenantActiveModelExt::set_tenant(ally, gamma.id)
            .unwrap()
            .insert(&ctx.db)
            .await
            .unwrap();

        let page = get(&request, Some(&w.alice), "/join/beta").await;
        assert_eq!(page.status_code(), 200);
        let html = page.text();
        assert!(html.contains("Ask to join Beta"));
        assert!(html.contains("Signed in as <strong>alice@example.com</strong>"));
        assert!(html.contains("Not you? Sign out"));
        assert!(html.contains(r#"action="/join/beta/request""#));
        assert!(!html.contains(r#"name="email""#));
        assert!(!html.contains(r#"name="password""#));
        assert!(!html.contains("data-org-id"), "no org claim on this page");
        assert!(
            html.contains(r#"name="name" value="alice""#),
            "from her default org"
        );
        assert!(html.contains("Nothing from Acme is shared with Beta."));

        let in_gamma = with_org(&ctx, &w.alice, gamma.id).await;
        let html = get(&request, Some(&in_gamma), "/join/beta").await.text();
        assert!(
            html.contains(r#"name="name" value="ally""#),
            "from this browser's org"
        );
        assert!(html.contains("Nothing from Gamma is shared with Beta."));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn join_pages_cannot_be_framed() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        assert_guarded(&get(&request, None, "/join/beta").await, "signed out");
        assert_guarded(
            &get(&request, Some(&w.alice), "/join/beta").await,
            "signed in",
        );
        let res = ask(&request, &w.alice, "beta", "bob").await;
        assert_eq!(res.status_code(), 422);
        assert_guarded(&res, "a refused ask");
        let res = request
            .post("/join/beta")
            .form(&json!({ "name": "x", "email": "new@example.com", "password": USER_PASSWORD }))
            .await;
        assert_eq!(res.status_code(), 422);
        assert_guarded(&res, "a refused new account");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_join_page_writes_nothing() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let before = membership_count(&ctx).await;
        get(&request, None, "/join/beta").await;
        get(&request, Some(&w.alice), "/join/beta").await;
        request
            .get("/join/beta")
            .add_header(w.alice.0.clone(), w.alice.1.clone())
            .add_header("Purpose", "prefetch")
            .await;
        assert_eq!(membership_count(&ctx).await, before);
        assert!(row(&ctx, w.beta, w.alice_id).await.is_none());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn signed_in_join_page_says_already_a_member() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _, _, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        // This browser is in Acme.
        let html = get(&request, Some(&alice), "/join/beta").await.text();
        assert!(html.contains("You're already in Beta"));
        assert!(html.contains("You're a member there as alice."));
        assert!(html
            .contains(r#"href="/o/beta/dashboard" class="btn btn-primary btn-lg">Open Beta</a>"#));
        assert!(!html.contains("/switch"));
        let _ = in_beta.organisation_id;
        assert!(!html.contains(r#"action="/join/beta/request""#));

        // Already in Beta here: straight to Home.
        let in_beta_tab = with_org(&ctx, &alice, in_beta.organisation_id).await;
        let res = get(&request, Some(&in_beta_tab), "/join/beta").await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(header(&res, "location"), "/o/beta/dashboard");

        // Asking changes nothing and comes back here.
        let res = ask(&request, &alice, "beta", "alicia").await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(header(&res, "location"), "/join/beta");
        let after = memberships::Model::find_in_org(&ctx.db, in_beta.organisation_id, in_beta.id)
            .await
            .unwrap();
        assert_eq!(after.username, "alice");
        assert_eq!(after.status, "active");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn signed_in_join_page_says_already_waiting() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        ask(&request, &w.alice, "beta", "alicia").await;

        let html = get(&request, Some(&w.alice), "/join/beta").await.text();
        assert!(html.contains("You've already asked to join"));
        assert!(html.contains(", as alicia."));
        assert!(!html.contains(r#"action="/join/beta/request""#));

        // Asking again changes nothing, the username included.
        let res = ask(&request, &w.alice, "beta", "alison").await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(
            header(&res, "location"),
            format!("/o/acme/dashboard?requested={}", w.beta)
        );
        assert_eq!(
            row(&ctx, w.beta, w.alice_id).await.unwrap().username,
            "alicia"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_join_page_shows_the_removed_and_declined_notes() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let joined = alice_joins_beta(&request, &ctx, &w).await;
        let res = manage(&request, &w.bob, "remove", joined.id).await;
        assert_eq!(res.status_code(), 200, "{}", res.text());

        let html = get(&request, Some(&w.alice), "/join/beta").await.text();
        assert!(html.contains("You were removed from Beta on "), "{html}");
        assert!(html.contains("You can ask to join again."));
        assert!(html.contains(r#"action="/join/beta/request""#));
        assert!(!html.contains("bob"), "never who removed her");

        ask(&request, &w.alice, "beta", "alice").await;
        let res = manage(&request, &w.bob, "reject", joined.id).await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let html = get(&request, Some(&w.alice), "/join/beta").await.text();
        assert!(
            html.contains("Your last request was declined on "),
            "{html}"
        );
        assert!(html.contains("You can ask again."));
        assert!(html.contains(r#"action="/join/beta/request""#));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_cross_origin_join_request_is_refused() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let res = request
            .post("/join/beta/request")
            .add_header(w.alice.0.clone(), w.alice.1.clone())
            .add_header("Origin", "https://evil.example")
            .form(&json!({ "name": "alice" }))
            .await;
        assert_eq!(res.status_code(), 403);
        assert!(row(&ctx, w.beta, w.alice_id).await.is_none());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn joining_while_signed_in_keeps_the_current_org() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let in_acme = with_org(&ctx, &w.alice, w.acme).await;
        let res = ask(&request, &in_acme, "beta", "alice").await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(set_cookie(&res, "current_org"), None);
        assert_eq!(set_cookie(&res, "auth_token"), None);

        let home = get(&request, Some(&in_acme), "/dashboard").await;
        assert_eq!(home.status_code(), 200);
        assert_eq!(org_heading(&home.text()), "Acme");
        assert_eq!(
            row(&ctx, w.beta, w.alice_id).await.unwrap().status,
            "pending"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn joining_lands_on_home_with_the_request_notice() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let res = ask(&request, &w.alice, "beta", "alice").await;
        assert_eq!(res.status_code(), 303);
        let to = header(&res, "location");
        assert_eq!(to, format!("/o/acme/dashboard?requested={}", w.beta));

        let home = get(&request, Some(&w.alice), &to).await;
        assert_eq!(home.status_code(), 200);
        let html = home.text();
        assert!(html.contains("You've asked to join Beta."));
        assert!(html.contains("Until then it shows in your organisations as waiting."));
        assert_eq!(header(&home, "cache-control"), "no-store");
        let plain = get(&request, Some(&w.alice), "/dashboard").await.text();
        assert!(!plain.contains("You've asked to join"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_request_notice_names_only_an_org_you_asked_to_join() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        sign_up_member(&request, "Gamma", "gail", "gail@example.com").await;
        let gamma = organisations::Model::find_by_slug(&ctx.db, "gamma")
            .await
            .unwrap();
        ask(&request, &w.alice, "gamma", "alice").await;
        memberships::Model::reject(
            &ctx.db,
            gamma.id,
            memberships::Approver {
                user_id: user_id(&ctx, "gail@example.com").await,
                acting: false,
            },
            row(&ctx, gamma.id, w.alice_id).await.unwrap().id,
        )
        .await
        .unwrap();

        for (what, value) in [
            ("her own active org", w.acme.to_string()),
            ("an org she was declined by", gamma.id.to_string()),
            ("someone else's org", w.beta.to_string()),
            ("no such org", "987654".to_string()),
            ("not a number", "beta".to_string()),
        ] {
            let html = get(
                &request,
                Some(&w.alice),
                &format!("/dashboard?requested={value}"),
            )
            .await
            .text();
            assert!(!html.contains("data-requested-notice"), "{what}");
            assert!(!html.contains("asked to join"), "{what}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_taken_username_redraws_the_form() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let res = ask(&request, &w.alice, "beta", "BOB").await;
        assert_eq!(res.status_code(), 422);
        let html = res.text();
        assert!(html.contains("That username is already used in Beta."));
        assert!(html.contains(r#"name="name" value="BOB""#));
        assert!(row(&ctx, w.beta, w.alice_id).await.is_none());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_pending_cap_shows_above_the_form() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        for n in 1..=5 {
            let owner = format!("owner{n}");
            sign_up_member(
                &request,
                &format!("Org {n}"),
                &owner,
                &format!("{owner}@example.com"),
            )
            .await;
            assert_eq!(
                ask(&request, &w.alice, &format!("org-{n}"), "alice")
                    .await
                    .status_code(),
                303
            );
        }
        let res = ask(&request, &w.alice, "beta", "alice").await;
        assert_eq!(res.status_code(), 422);
        let html = res.text();
        let alert = html
            .find(r#"class="alert alert-error m-0 mt-6""#)
            .expect("a page alert");
        let form = html.find(r#"action="/join/beta/request""#).unwrap();
        assert!(alert < form, "above the form");
        assert!(html[alert..form].contains("already asked to join 5 organisations."));
        assert!(
            !html.contains("aria-invalid"),
            "the username field stays normal"
        );
        assert!(html.contains(r#"data-busy-label="Asking…""#));
        assert!(row(&ctx, w.beta, w.alice_id).await.is_none());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn joining_needs_a_session() {
    request::<App, _, _>(|request, ctx| async move {
        world(&request, &ctx).await;
        let before = membership_count(&ctx).await;
        let res = request
            .post("/join/beta/request")
            .form(&json!({ "name": "stranger" }))
            .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(header(&res, "location"), "/login");
        assert_eq!(membership_count(&ctx).await, before);
    })
    .await;
}

/// A super admin with their own org (from the task), and `acting_org` set.
fn acting_in(admin: &Cookie, org_id: i64) -> Cookie {
    (
        admin.0.clone(),
        format!("{}; acting_org={org_id}", admin.1.to_str().unwrap())
            .parse()
            .unwrap(),
    )
}

#[tokio::test]
#[serial]
async fn asking_while_acting_ends_acting() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let admin = super::admin::super_admin_cookie(&request, &ctx).await;
        let acting = acting_in(&admin, w.acme);

        let html = get(&request, Some(&acting), "/join/beta").await.text();
        assert!(html.contains("You're acting in <strong>Acme</strong>."));
        assert!(html.contains("This request comes from your own account"));
        assert!(html.contains("Asking ends acting."));
        assert!(html.contains("Nothing from your other organisations is shared with Beta."));

        let res = ask(&request, &acting, "beta", "raj").await;
        assert_eq!(res.status_code(), 303, "{}", res.text());
        assert!(clears(&res, "acting_org"));
        let raj = user_id(&ctx, "gurungraj26@gmail.com").await;
        assert_eq!(row(&ctx, w.beta, raj).await.unwrap().status, "pending");
        assert!(row(&ctx, w.acme, raj).await.is_none(), "nothing in Acme");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_super_admin_with_only_a_pending_row_lands_on_admin() {
    request::<App, _, _>(|request, ctx| async move {
        world(&request, &ctx).await;
        create_user(&ctx, "root@example.com", "root")
            .await
            .into_active_model()
            .make_super_admin(&ctx.db)
            .await
            .unwrap();
        let root = login(&request, "root@example.com", USER_PASSWORD).await;
        let res = ask(&request, &root, "beta", "root").await;
        assert_eq!(res.status_code(), 303);

        let res = get(&request, Some(&root), &header(&res, "location")).await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(header(&res, "location"), "/admin");
        let res = get(&request, Some(&root), "/dashboard").await;
        assert_eq!(header(&res, "location"), "/admin");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_switcher_shows_the_request_as_waiting() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        ask(&request, &w.alice, "beta", "alice").await;
        let res = request
            .get(&w.alice.at("/orgs"))
            .as_member(&w.alice)
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 200);
        let html = res.text();
        let waiting = html.find("Waiting for approval").expect("a waiting row");
        let beta = html.find(">Beta<").expect("Beta is listed");
        assert!(beta < waiting, "Beta's row is the waiting one");
        assert!(!html.contains(&format!("/orgs/{}/switch", w.beta)));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn after_approval_alice_switches_to_beta_without_signing_in() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let before = users::Model::find_by_id(&ctx.db, w.alice_id).await.unwrap();
        let switch = |cookie: TestMember| {
            let request = &request;
            async move {
                request
                    .post(&format!("/orgs/{}/switch", w.beta))
                    .add_header(cookie.0.clone(), cookie.1.clone())
                    .await
            }
        };
        ask(&request, &w.alice, "beta", "alice").await;
        assert_eq!(switch(w.alice.clone()).await.status_code(), 404);

        alice_joins_beta(&request, &ctx, &w).await;
        let res = switch(w.alice.clone()).await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(header(&res, "location"), "/o/beta/dashboard");
        let in_beta = with_org(&ctx, &w.alice, w.beta).await;
        let home = get(&request, Some(&in_beta), "/dashboard").await;
        assert_eq!(org_heading(&home.text()), "Beta");
        let after = users::Model::find_by_id(&ctx.db, w.alice_id).await.unwrap();
        assert_eq!(after.session_version, before.session_version);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn after_approval_the_waiting_page_opens_the_org() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        // Carol's only org removes her; she asks to join again.
        let carol = join(&request, &ctx, "beta", "carol", "carol@example.com").await;
        let carol_id = user_id(&ctx, "carol@example.com").await;
        let first = row(&ctx, w.beta, carol_id).await.unwrap();
        manage(&request, &w.bob, "approve", first.id).await;
        manage(&request, &w.bob, "remove", first.id).await;
        let res = ask(&request, &carol, "beta", "carol").await;
        assert_eq!(res.status_code(), 303);

        let res = get(&request, Some(&carol), "/dashboard").await;
        assert_eq!(res.status_code(), 403);
        assert!(res.text().contains("Waiting for approval"));
        manage(&request, &w.bob, "approve", first.id).await;
        let res = get(&request, Some(&carol), "/dashboard").await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(org_heading(&res.text()), "Beta");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn rejoin_after_removal_waits_for_approval() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let joined = alice_joins_beta(&request, &ctx, &w).await;
        manage(&request, &w.bob, "remove", joined.id).await;

        let res = ask(&request, &w.alice, "beta", "alicia").await;
        assert_eq!(res.status_code(), 303);
        let again = row(&ctx, w.beta, w.alice_id).await.unwrap();
        assert_eq!(again.id, joined.id);
        assert_eq!(again.status, "pending");
        let in_beta = with_org(&ctx, &w.alice, w.beta).await;
        let home = get(&request, Some(&in_beta), "/dashboard").await;
        assert_eq!(org_heading(&home.text()), "Acme", "Beta stays closed");

        manage(&request, &w.bob, "approve", joined.id).await;
        let home = get(&request, Some(&in_beta), "/dashboard").await;
        assert_eq!(org_heading(&home.text()), "Beta");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn rejoin_after_decline_waits_for_approval() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        ask(&request, &w.alice, "beta", "alice").await;
        let first = row(&ctx, w.beta, w.alice_id).await.unwrap();
        manage(&request, &w.bob, "reject", first.id).await;
        assert_eq!(
            row(&ctx, w.beta, w.alice_id).await.unwrap().status,
            "rejected"
        );

        ask(&request, &w.alice, "beta", "alice").await;
        let again = row(&ctx, w.beta, w.alice_id).await.unwrap();
        assert_eq!(again.id, first.id);
        assert_eq!(again.status, "pending");
        manage(&request, &w.bob, "approve", first.id).await;
        assert_eq!(
            row(&ctx, w.beta, w.alice_id).await.unwrap().status,
            "active"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_rejoined_member_has_no_old_claude_tokens() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let joined = alice_joins_beta(&request, &ctx, &w).await;
        let (_, token) = access_tokens::Model::create_personal(
            &ctx.db,
            w.beta,
            w.alice_id,
            &access_tokens::PersonalParams {
                name: "Laptop".to_string(),
            },
            super::mcp::RESOURCE,
        )
        .await
        .unwrap();
        manage(&request, &w.bob, "remove", joined.id).await;
        ask(&request, &w.alice, "beta", "alice").await;
        manage(&request, &w.bob, "approve", joined.id).await;

        assert_eq!(
            row(&ctx, w.beta, w.alice_id).await.unwrap().status,
            "active"
        );
        let usable = access_tokens::Model::find_usable(&ctx.db, &token, super::mcp::RESOURCE)
            .await
            .unwrap();
        assert!(usable.is_none(), "the old token stays revoked");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_rotate_page_survives_a_rejoin() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let joined = alice_joins_beta(&request, &ctx, &w).await;
        manage(&request, &w.bob, "remove", joined.id).await;
        let rotate = format!("/members/{}/rotate", joined.id);
        let status = || async { get(&request, Some(&w.bob), &rotate).await.status_code() };
        assert_eq!(status().await, 200, "removed");

        ask(&request, &w.alice, "beta", "alice").await;
        assert_eq!(status().await, 200, "asked again");
        manage(&request, &w.bob, "reject", joined.id).await;
        assert_eq!(status().await, 200, "declined");
        ask(&request, &w.alice, "beta", "alice").await;
        manage(&request, &w.bob, "approve", joined.id).await;
        assert_eq!(status().await, 404, "approved");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_tab_for_an_org_you_are_pending_in_gets_the_gone_header() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let joined = alice_joins_beta(&request, &ctx, &w).await;
        manage(&request, &w.bob, "remove", joined.id).await;
        ask(&request, &w.alice, "beta", "alice").await;

        // A tab still drawn for Beta.
        let tab = with_org(&ctx, &w.alice, w.beta).await;
        let res = request
            .get(&tab.at("/orgs/current"))
            .as_member(&tab)
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 409);
        assert_eq!(header(&res, "X-Org-Gone"), "1");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn sign_in_to_join_returns_to_the_join_page() {
    request::<App, _, _>(|request, ctx| async move {
        world(&request, &ctx).await;
        let html = get(&request, None, "/join/beta").await.text();
        assert!(html.contains(r#"href="/login?next=/join/beta""#));
        assert!(html.contains("Sign in to join Beta"));

        let html = get(&request, None, "/login?next=/join/beta").await.text();
        assert!(html.contains(r#"name="next" value="/join/beta""#));
        assert!(html.contains("ask to join <strong class=\"text-text\">Beta</strong>"));
        // An unknown slug shows the plain page, never the query text.
        let html = get(&request, None, "/login?next=/join/nowhere").await.text();
        assert!(!html.contains("ask to join"));
        assert!(!html.contains("nowhere</strong>"));

        let res = request
            .post("/login")
            .form(&json!({ "email": "alice@example.com", "password": USER_PASSWORD, "next": "/join/beta" }))
            .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(header(&res, "location"), "/join/beta");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn forgot_password_keeps_the_join_page() {
    request::<App, _, _>(|request, ctx| async move {
        world(&request, &ctx).await;
        let login = get(&request, None, "/login?next=/join/beta").await.text();
        assert!(login.contains(r#"href="/forgot?next=%2Fjoin%2Fbeta""#));
        let forgot = get(&request, None, "/forgot?next=/join/beta").await.text();
        assert!(forgot.contains(r#"name="next" value="/join/beta""#));
        let bad = get(&request, None, "/forgot?next=//evil.example")
            .await
            .text();
        assert!(!bad.contains(r#"name="next""#));

        let res = request
            .post("/forgot")
            .form(&json!({ "email": "alice@example.com", "next": "/join/beta" }))
            .await;
        assert_eq!(res.status_code(), 200);
        let token = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap()
            .reset_token
            .unwrap();
        let mail = deliveries(&ctx).join("\n");
        assert!(
            mail.contains(&format!("/reset/{token}?next=%2Fjoin%2Fbeta")),
            "{mail}"
        );

        let reset = get(&request, None, &format!("/reset/{token}?next=/join/beta"))
            .await
            .text();
        assert!(reset.contains(r#"name="next" value="/join/beta""#));
        let res = request
            .post(&format!("/reset/{token}"))
            .form(&json!({
                "password": "new-password-1",
                "password_confirmation": "new-password-1",
                "next": "/join/beta",
            }))
            .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(
            header(&res, "location"),
            "/login?reset=1&next=%2Fjoin%2Fbeta"
        );
        let login = get(&request, None, &header(&res, "location")).await.text();
        assert!(login.contains(r#"name="next" value="/join/beta""#));

        // A bad next is dropped at each step.
        request
            .post("/forgot")
            .form(&json!({ "email": "bob@example.com", "next": "https://evil.example" }))
            .await;
        let token = users::Model::find_by_email(&ctx.db, "bob@example.com")
            .await
            .unwrap()
            .reset_token
            .unwrap();
        let mail = deliveries(&ctx).join("\n");
        assert!(
            mail.contains(&format!("/reset/{token}\r\n"))
                || mail.contains(&format!("/reset/{token}\n"))
        );
        assert!(!mail.contains("evil.example"));
        let reset = get(&request, None, &format!("/reset/{token}?next=/join/Beta"))
            .await
            .text();
        assert!(!reset.contains(r#"name="next""#));
        let res = request
            .post(&format!("/reset/{token}"))
            .form(&json!({
                "password": "new-password-1",
                "password_confirmation": "new-password-1",
                "next": "/join/beta?x=1",
            }))
            .await;
        assert_eq!(header(&res, "location"), "/login?reset=1");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn sign_in_to_join_keeps_the_sign_in_limits() {
    request::<App, _, _>(|request, ctx| async move {
        world(&request, &ctx).await;
        let sign_in = |password: &'static str| {
            let request = &request;
            async move {
                request
                    .post("/login")
                    .form(&json!({ "email": "alice@example.com", "password": password, "next": "/join/beta" }))
                    .await
            }
        };
        for _ in 0..5 {
            let res = sign_in("wrong-password").await;
            assert_eq!(res.status_code(), 422);
            assert!(res.text().contains(r#"name="next" value="/join/beta""#));
        }
        let res = sign_in(USER_PASSWORD).await;
        assert_eq!(res.status_code(), 429, "the right password is refused too");
        assert_eq!(header(&res, "location"), "");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_new_account_form_never_attaches_to_an_existing_account() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let before = users::Model::find_by_id(&ctx.db, w.alice_id).await.unwrap();
        let res = request
            .post("/join/beta")
            .form(&json!({ "name": "alice", "email": "alice@example.com", "password": "another-password" }))
            .await;
        assert_eq!(res.status_code(), 422);
        let html = res.text();
        assert!(html.contains("An account with this email already exists. Sign in to join Beta."));
        assert!(html.contains(r#"href="/login?next=/join/beta""#));
        assert_eq!(set_cookie(&res, "auth_token"), None);
        assert!(row(&ctx, w.beta, w.alice_id).await.is_none());
        let after = users::Model::find_by_id(&ctx.db, w.alice_id).await.unwrap();
        assert_eq!(after.password, before.password);
        login(&request, "alice@example.com", USER_PASSWORD).await;
    })
    .await;
}

#[tokio::test]
#[serial]
async fn approval_emails_the_person() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let bob = sign_up_member(&request, "Bakers & Co", "bob", "bob@example.com").await;
        let slug = organisations::Model::find_by_id(&ctx.db, bob.org_id)
            .await
            .unwrap()
            .slug;
        ask(&request, &alice, &slug, "ally").await;
        assert!(approval_emails(&ctx).is_empty());
        let alice_id = user_id(&ctx, "alice@example.com").await;
        let pending = row(&ctx, bob.org_id, alice_id).await.unwrap();
        manage(&request, &bob, "approve", pending.id).await;

        let mails = approval_emails(&ctx);
        assert_eq!(mails.len(), 1);
        let mail = &mails[0];
        assert!(mail.contains("To: alice@example.com"), "{mail}");
        let subject = mail.lines().find(|l| l.starts_with("Subject:")).unwrap();
        assert!(!subject.contains("Bakers"), "{subject}");
        assert!(
            mail.contains("Bakers &amp; Co"),
            "escaped in the HTML: {mail}"
        );
        assert!(mail.contains("Hi ally"));
        assert!(!mail.contains("Acme"));
        assert!(!mail.contains("bob"), "never names the approver");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn approval_emails_stop_at_twenty_a_day_per_org() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let beta = organisations::Model::find_by_id(&ctx.db, w.beta)
            .await
            .unwrap();
        let mut ids = Vec::new();
        for n in 1..=21 {
            let user = create_user(&ctx, &format!("p{n}@example.com"), &format!("person{n}")).await;
            let outcome = memberships::Model::request_to_join(
                &ctx.db,
                &beta,
                user.id,
                &memberships::JoinParams {
                    name: format!("person{n}"),
                },
            )
            .await
            .unwrap();
            let memberships::JoinOutcome::Requested(m) = outcome else {
                panic!("{outcome:?}");
            };
            ids.push(m.id);
        }
        for id in &ids {
            let res = manage(&request, &w.bob, "approve", *id).await;
            assert_eq!(res.status_code(), 200);
        }
        assert_eq!(approval_emails(&ctx).len(), 20);
        let last = memberships::Model::find_in_org(&ctx.db, w.beta, ids[20])
            .await
            .unwrap();
        assert_eq!(last.status, "active", "the 21st is still approved");
    })
    .await;
}

/// Asks `who` into Beta as `name` straight on the model, and returns the row.
async fn pending_in(ctx: &AppContext, org_id: i64, user_id: i64, name: &str) -> i64 {
    let org = organisations::Model::find_by_id(&ctx.db, org_id)
        .await
        .unwrap();
    let outcome = memberships::Model::request_to_join(
        &ctx.db,
        &org,
        user_id,
        &memberships::JoinParams {
            name: name.to_string(),
        },
    )
    .await
    .unwrap();
    match outcome {
        memberships::JoinOutcome::Requested(m) => m.id,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
#[serial]
async fn rejoining_again_and_again_stops_the_emails() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        for cycle in 1..=5 {
            let id = pending_in(&ctx, w.beta, w.alice_id, "alice").await;
            let res = manage(&request, &w.bob, "approve", id).await;
            assert!(toast(&res).contains("alice approved."), "cycle {cycle}");
            manage(&request, &w.bob, "remove", id).await;
        }
        assert_eq!(
            approval_emails(&ctx).len(),
            3,
            "at most 3 a day to one person, however often the row is approved"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn declining_rejoined_rows_does_not_reset_the_org_count() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let mut people = Vec::new();
        for n in 1..=20 {
            let user = create_user(&ctx, &format!("p{n}@example.com"), &format!("person{n}")).await;
            let id = pending_in(&ctx, w.beta, user.id, &format!("person{n}")).await;
            manage(&request, &w.bob, "approve", id).await;
            people.push((user.id, id));
        }
        assert_eq!(approval_emails(&ctx).len(), 20);
        // Five of them are removed, ask again and are declined.
        for (user_id, id) in &people[..5] {
            manage(&request, &w.bob, "remove", *id).await;
            let again = pending_in(&ctx, w.beta, *user_id, &format!("back{user_id}")).await;
            assert_eq!(again, *id);
            manage(&request, &w.bob, "reject", *id).await;
        }
        let newcomer = create_user(&ctx, "late@example.com", "late").await;
        let id = pending_in(&ctx, w.beta, newcomer.id, "late").await;
        manage(&request, &w.bob, "approve", id).await;
        assert_eq!(approval_emails(&ctx).len(), 20, "still at the org's limit");
        assert_eq!(
            row(&ctx, w.beta, newcomer.id).await.unwrap().status,
            "active"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_join_request_emails_and_notifies_nobody() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let mails = deliveries(&ctx).len();
        ask(&request, &w.alice, "beta", "alice").await;
        assert_eq!(deliveries(&ctx).len(), mails);
        let notes = notifications::Entity::find().count(&ctx.db).await.unwrap();
        assert_eq!(notes, 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn an_acting_super_admin_approves_others_but_not_themselves() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let admin = super::admin::super_admin_cookie(&request, &ctx).await;
        let raj = user_id(&ctx, "gurungraj26@gmail.com").await;
        ask(&request, &admin, "beta", "raj").await;
        ask(&request, &w.alice, "beta", "alice").await;
        let acting = TestMember::new(acting_in(&admin, w.beta), w.beta, "beta");

        let own = row(&ctx, w.beta, raj).await.unwrap();
        let res = manage(&request, &acting, "approve", own.id).await;
        assert!(
            toast(&res).contains("You can't approve your own request."),
            "{}",
            toast(&res)
        );
        assert_eq!(row(&ctx, w.beta, raj).await.unwrap().status, "pending");

        let alices = row(&ctx, w.beta, w.alice_id).await.unwrap();
        let res = manage(&request, &acting, "approve", alices.id).await;
        assert!(toast(&res).contains("alice approved."), "{}", toast(&res));
        assert_eq!(
            row(&ctx, w.beta, w.alice_id).await.unwrap().status,
            "active"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_stale_members_page_cannot_approve_a_removed_member() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let joined = alice_joins_beta(&request, &ctx, &w).await;
        manage(&request, &w.bob, "remove", joined.id).await;

        let res = manage(&request, &w.bob, "approve", joined.id).await;
        assert!(
            toast(&res).contains("Only pending requests can be approved."),
            "{}",
            toast(&res)
        );
        assert_eq!(
            row(&ctx, w.beta, w.alice_id).await.unwrap().status,
            "removed"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_waiting_list_shows_only_this_orgs_details() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up_member(&request, "Acme", "acmealice", "alice@example.com").await;
        let bob = sign_up_member(&request, "Beta", "bob", "bob@example.com").await;
        let alice_id = user_id(&ctx, "alice@example.com").await;
        ask(&request, &alice, "beta", "ally").await;
        let first = row(&ctx, bob.org_id, alice_id).await.unwrap();
        manage(&request, &bob, "approve", first.id).await;
        manage(&request, &bob, "remove", first.id).await;
        ask(&request, &alice, "beta", "ally").await;

        let html = get(&request, Some(&bob), "/members").await.text();
        let start = html.find(&format!(r#"id="member-{}""#, first.id)).unwrap();
        let end = start + html[start..].find("</form>\n  </div>").unwrap();
        let waiting = &html[start..end];
        assert!(waiting.contains("ally"));
        assert!(waiting.contains("alice@example.com"));
        assert!(waiting.contains("Removed on "), "{waiting}");
        assert!(!waiting.contains("acmealice"));
        assert!(!waiting.contains("Acme"));
        assert!(!waiting.contains("bob"), "no names of who decided");
        assert!(html.contains(
            "People who use it join with a new or existing account, then wait for you or another admin to let them in."
        ));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_waiting_row_that_was_removed_links_to_the_rotate_page() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let joined = alice_joins_beta(&request, &ctx, &w).await;
        manage(&request, &w.bob, "remove", joined.id).await;
        ask(&request, &w.alice, "beta", "alice").await;
        // Carol was declined, so her line has no link.
        let carol = join(&request, &ctx, "beta", "carol", "carol@example.com").await;
        let carol_row = row(&ctx, w.beta, user_id(&ctx, "carol@example.com").await)
            .await
            .unwrap();
        manage(&request, &w.bob, "reject", carol_row.id).await;
        ask(&request, &carol, "beta", "carol").await;

        let html = get(&request, Some(&w.bob), "/members").await.text();
        let link = format!(r#"href="/o/beta/members/{}/rotate""#, joined.id);
        assert!(html.contains(&link), "{html}");
        assert!(html.contains("Change these at the source"));
        assert!(!html.contains(&format!("/members/{}/rotate", carol_row.id)));
        assert_eq!(
            get(
                &request,
                Some(&w.bob),
                &format!("/members/{}/rotate", joined.id)
            )
            .await
            .status_code(),
            200
        );

        // A Beta admin who didn't remove her can't open the page, so no link.
        let dan = join(&request, &ctx, "beta", "dan", "dan@example.com").await;
        let dan_row = row(&ctx, w.beta, user_id(&ctx, "dan@example.com").await)
            .await
            .unwrap();
        manage(&request, &w.bob, "approve", dan_row.id).await;
        let mut admin = row(&ctx, w.beta, dan_row.user_id)
            .await
            .unwrap()
            .into_active_model();
        admin.role = ActiveValue::Set(memberships::role::ADMIN.to_string());
        admin.update(&ctx.db).await.unwrap();
        let html = get(&request, Some(&dan), "/members").await.text();
        assert!(html.contains("Removed on "));
        assert!(!html.contains(&link));
    })
    .await;
}

// The pending row's role is left alone until approval; a former owner or
// admin who asks again grants nothing while waiting.
#[tokio::test]
#[serial]
async fn a_waiting_former_admin_has_no_access() {
    request::<App, _, _>(|request, ctx| async move {
        let w = world(&request, &ctx).await;
        let joined = alice_joins_beta(&request, &ctx, &w).await;
        let mut admin = joined.clone().into_active_model();
        admin.role = ActiveValue::Set(memberships::role::ADMIN.to_string());
        admin.update(&ctx.db).await.unwrap();
        manage(&request, &w.bob, "remove", joined.id).await;
        ask(&request, &w.alice, "beta", "alice").await;
        assert_eq!(row(&ctx, w.beta, w.alice_id).await.unwrap().role, "admin");

        let in_beta = TestMember::new(
            with_org(&ctx, &w.alice, w.beta).await.cookie,
            w.beta,
            "beta",
        );
        let res = request
            .get(&in_beta.at("/dashboard"))
            .add_header(in_beta.0.clone(), in_beta.1.clone())
            .await;
        assert_eq!(res.status_code(), 403);
        assert!(res.text().contains("Waiting for approval · Beta"));
        let bob_id = user_id(&ctx, "bob@example.com").await;
        let bob_row = row(&ctx, w.beta, bob_id).await.unwrap();
        request
            .post(&in_beta.at(&format!("/members/{}/remove", bob_row.id)))
            .as_member(&in_beta)
            .await;
        assert_eq!(
            row(&ctx, w.beta, bob_id).await.unwrap().status,
            "active",
            "no admin rights in Beta"
        );
        let pending = memberships::Entity::find()
            .filter(memberships::Column::OrganisationId.eq(w.beta))
            .filter(memberships::Column::Status.eq("pending"))
            .count(&ctx.db)
            .await
            .unwrap();
        assert_eq!(pending, 1);
    })
    .await;
}
