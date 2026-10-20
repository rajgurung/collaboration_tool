//! Removal from one organisation. Alice is in Acme and Beta; Bob owns Beta
//! and removes her. Only Beta stops: she stays signed in, and Acme keeps
//! working.
use collab::{
    app::App,
    models::{
        access_tokens::{self, PersonalParams},
        folders, login_events, logins, memberships, notifications, oauth_codes, organisations,
        tasks, users,
    },
};
use loco_rs::{app::AppContext, testing::prelude::*, TestServer};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, IntoActiveModel, PaginatorTrait,
    QueryFilter,
};
use serde_json::json;
use serial_test::serial;

use super::{
    mcp::{rpc, tool, RESOURCE},
    multi_org::{alice_in_two_orgs, clears, org_heading, set_status, switch, with_org},
    oauth::{post_decision, register, CLAUDE},
    prepare_data::{
        approve_as_owner, join, login, sign_up_member, AsMember, TestMember, USER_PASSWORD,
    },
    tasks::{project_in, task_form},
};

struct Removal {
    /// Alice's cookie, drawn for Acme (her default).
    alice: TestMember,
    /// The same cookie with `current_org` = Beta: a Beta tab.
    in_beta: TestMember,
    alice_id: i64,
    acme: i64,
    beta: i64,
    beta_membership: memberships::Model,
}

/// Alice in Acme and Beta, before the removal.
async fn setup(request: &TestServer, ctx: &AppContext) -> Removal {
    let (alice, alice_id, in_acme, in_beta) = alice_in_two_orgs(request, ctx).await;
    Removal {
        in_beta: with_org(&alice, in_beta.organisation_id),
        alice,
        alice_id,
        acme: in_acme.organisation_id,
        beta: in_beta.organisation_id,
        beta_membership: in_beta,
    }
}

async fn bob(request: &TestServer, beta: i64) -> TestMember {
    TestMember::new(login(request, "bob@example.com", USER_PASSWORD).await, beta)
}

/// Bob removes `membership` from Beta through the Members page.
async fn remove(request: &TestServer, bob: &TestMember, membership: &memberships::Model) {
    let res = request
        .post(&format!("/members/{}/remove", membership.id))
        .as_member(bob)
        .add_header("HX-Request", "true")
        .await;
    assert_eq!(res.status_code(), 200, "{}", res.text());
}

async fn bob_removes_alice(request: &TestServer, r: &Removal) {
    let bob = bob(request, r.beta).await;
    remove(request, &bob, &r.beta_membership).await;
}

fn header(res: &axum_test::TestResponse, name: &str) -> Option<String> {
    res.headers()
        .get(name)
        .map(|v| v.to_str().unwrap().to_string())
}

async fn get(request: &TestServer, who: &TestMember, path: &str) -> axum_test::TestResponse {
    request
        .get(path)
        .add_header(who.0.clone(), who.1.clone())
        .await
}

async fn task_count(ctx: &AppContext) -> u64 {
    tasks::Entity::find().count(&ctx.db).await.unwrap()
}

/// An HTMX task create from `who`'s tab.
async fn htmx_task(
    request: &TestServer,
    ctx: &AppContext,
    who: &TestMember,
) -> axum_test::TestResponse {
    let project = project_in(ctx, "beta", "Launch").await;
    request
        .post("/tasks")
        .as_member(who)
        .add_header("HX-Request", "true")
        .form(&task_form("Sneaky", project.id, ""))
        .await
}

#[tokio::test]
#[serial]
async fn removal_from_beta_keeps_alice_signed_in_to_acme() {
    request::<App, _, _>(|request, ctx| async move {
        let r = setup(&request, &ctx).await;
        let before = users::Model::find_by_id(&ctx.db, r.alice_id).await.unwrap();
        bob_removes_alice(&request, &r).await;

        let res = get(&request, &r.alice, "/dashboard").await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(org_heading(&res.text()), "Acme");
        let in_acme = with_org(&r.alice, r.acme);
        assert_eq!(get(&request, &in_acme, "/tasks").await.status_code(), 200);
        let project = project_in(&ctx, "acme", "Shop").await;
        let res = request
            .post("/tasks")
            .as_member(&in_acme)
            .form(&task_form("Restock", project.id, ""))
            .await;
        assert_eq!(res.status_code(), 303, "Acme still takes writes");
        let after = users::Model::find_by_id(&ctx.db, r.alice_id).await.unwrap();
        assert_eq!(after.session_version, before.session_version);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn after_removal_a_beta_cookie_falls_back_and_is_cleared() {
    request::<App, _, _>(|request, ctx| async move {
        let r = setup(&request, &ctx).await;
        bob_removes_alice(&request, &r).await;

        let res = get(&request, &r.in_beta, "/dashboard").await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(org_heading(&res.text()), "Acme");
        assert!(clears(&res, "current_org"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_beta_tab_is_refused_with_the_gone_header() {
    request::<App, _, _>(|request, ctx| async move {
        let r = setup(&request, &ctx).await;
        bob_removes_alice(&request, &r).await;
        let before = task_count(&ctx).await;

        let res = htmx_task(&request, &ctx, &r.in_beta).await;
        assert_eq!(res.status_code(), 409);
        assert_eq!(
            header(&res, "HX-Redirect"),
            Some(format!("/dashboard?from={}", r.beta))
        );
        assert_eq!(header(&res, "X-Org-Gone").as_deref(), Some("1"));
        assert_eq!(header(&res, "cache-control").as_deref(), Some("no-store"));
        assert_eq!(task_count(&ctx).await, before, "nothing saved");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_plain_beta_form_is_refused_after_removal() {
    request::<App, _, _>(|request, ctx| async move {
        let r = setup(&request, &ctx).await;
        let project = project_in(&ctx, "beta", "Launch").await;
        bob_removes_alice(&request, &r).await;
        let before = task_count(&ctx).await;

        let mut form = task_form("Sneaky", project.id, "");
        form["_org"] = json!(r.beta.to_string());
        let res = request
            .post("/tasks")
            .add_header(r.in_beta.0.clone(), r.in_beta.1.clone())
            .form(&form)
            .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(
            header(&res, "location"),
            Some(format!("/dashboard?from={}", r.beta))
        );
        // The header comes with every refusal. A plain form's browser follows
        // the 303 and never reads it; Home names the org from `?from=`.
        assert_eq!(header(&res, "X-Org-Gone").as_deref(), Some("1"));
        assert_eq!(task_count(&ctx).await, before, "nothing saved");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_home_notice_names_beta_after_removal() {
    request::<App, _, _>(|request, ctx| async move {
        let r = setup(&request, &ctx).await;
        bob_removes_alice(&request, &r).await;

        let res = get(&request, &r.alice, &format!("/dashboard?from={}", r.beta)).await;
        assert_eq!(header(&res, "cache-control").as_deref(), Some("no-store"));
        let page = res.text();
        assert!(
            page.contains("You no longer have access to Beta. You&#x27;re now in Acme.")
                || page.contains("You no longer have access to Beta. You're now in Acme."),
            "{page}"
        );
        assert!(page.contains("An owner or admin removed you. Nothing from this tab was saved."));
        assert!(
            !page.contains(&format!("/orgs/{}/switch", r.beta)),
            "no Back"
        );
        assert!(!page.contains("This tab was showing another organisation"));
    })
    .await;
}

/// The notice for a `?from=` that names no org this user was removed from.
fn assert_nameless(page: &str, what: &str) {
    assert!(
        page.contains("This tab was showing another organisation."),
        "{what}"
    );
    // The layout's banner templates hold their own copy, so only the
    // notice's own second line is checked.
    assert!(!page.contains("An owner or admin removed you"), "{what}");
}

#[tokio::test]
#[serial]
async fn the_home_notice_does_not_name_an_org_you_were_never_removed_from() {
    request::<App, _, _>(|request, ctx| async move {
        let r = setup(&request, &ctx).await;
        // Beta pending, Gamma declined, Delta someone else's, and a deleted id.
        set_status(
            &ctx,
            r.beta_membership.clone(),
            memberships::status::PENDING,
        )
        .await;
        sign_up_member(&request, "Gamma", "gail", "gail@example.com").await;
        sign_up_member(&request, "Delta", "dina", "dina@example.com").await;
        let gamma = organisations::Model::find_by_slug(&ctx.db, "gamma")
            .await
            .unwrap();
        let delta = organisations::Model::find_by_slug(&ctx.db, "delta")
            .await
            .unwrap();
        let declined = memberships::ActiveModel {
            user_id: ActiveValue::Set(r.alice_id),
            username: ActiveValue::Set("alice".to_string()),
            role: ActiveValue::Set(memberships::role::MEMBER.to_string()),
            status: ActiveValue::Set(memberships::status::REJECTED.to_string()),
            ..Default::default()
        };
        loco_rs::prelude::TenantActiveModelExt::set_tenant(declined, gamma.id)
            .unwrap()
            .insert(&ctx.db)
            .await
            .unwrap();

        for (what, id) in [
            ("pending", r.beta),
            ("declined", gamma.id),
            ("someone else's", delta.id),
            ("deleted", 987_654),
        ] {
            let page = get(&request, &r.alice, &format!("/dashboard?from={id}"))
                .await
                .text();
            assert_nameless(&page, what);
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_stale_tab_for_an_org_you_are_still_in_has_no_gone_header() {
    request::<App, _, _>(|request, ctx| async move {
        let r = setup(&request, &ctx).await;
        // The browser is in Acme; the tab was drawn for Beta, still active.
        let stale = TestMember::new(with_org(&r.alice, r.acme).cookie, r.beta);
        let res = htmx_task(&request, &ctx, &stale).await;
        assert_eq!(res.status_code(), 409);
        assert_eq!(header(&res, "X-Org-Gone"), None);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn someone_elses_removal_is_never_named() {
    request::<App, _, _>(|request, ctx| async move {
        let r = setup(&request, &ctx).await;
        // Carol is removed from Gamma, where Alice has no row at all.
        let gail = sign_up_member(&request, "Gamma", "gail", "gail@example.com").await;
        join(&request, &ctx, "gamma", "carol", "carol@example.com").await;
        let carol = users::Model::find_by_email(&ctx.db, "carol@example.com")
            .await
            .unwrap();
        let carol_m = approve_as_owner(
            &ctx,
            &memberships::Model::default_for_user(&ctx.db, carol.id)
                .await
                .unwrap()
                .unwrap(),
        )
        .await;
        remove(&request, &gail, &carol_m).await;
        let gamma = gail.org_id;

        // A forged claim for Gamma is refused, but never as a removal.
        let forged = TestMember::new(r.alice.cookie.clone(), gamma);
        let res = htmx_task(&request, &ctx, &forged).await;
        assert_eq!(res.status_code(), 409);
        assert_eq!(header(&res, "X-Org-Gone"), None);
        let page = get(&request, &r.alice, &format!("/dashboard?from={gamma}"))
            .await
            .text();
        assert_nameless(&page, "someone else's removal");
        assert!(!page.contains("Gamma"), "{page}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_switch_in_one_browser_keeps_an_unlock_in_another() {
    request::<App, _, _>(|request, ctx| async move {
        let r = setup(&request, &ctx).await;
        let laptop = TestMember::new(
            login(&request, "alice@example.com", USER_PASSWORD).await,
            r.acme,
        );
        let phone = TestMember::new(
            login(&request, "alice@example.com", USER_PASSWORD).await,
            r.acme,
        );
        let res = request
            .post("/documents/logins/unlock")
            .as_member(&laptop)
            .form(&[("password", USER_PASSWORD)])
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let unlocked = users::Model::find_by_id(&ctx.db, r.alice_id).await.unwrap();
        let sid = unlocked.secrets_unlocked_sid.clone().unwrap();

        assert_eq!(switch(&request, &phone, r.beta).await.status_code(), 303);
        let alice = users::Model::find_by_id(&ctx.db, r.alice_id).await.unwrap();
        assert!(
            alice.secrets_unlocked(Some(&sid), r.acme),
            "the laptop's unlock"
        );

        assert_eq!(switch(&request, &laptop, r.beta).await.status_code(), 303);
        let alice = users::Model::find_by_id(&ctx.db, r.alice_id).await.unwrap();
        assert!(
            !alice.secrets_unlocked(Some(&sid), r.acme),
            "its own switch"
        );
    })
    .await;
}

async fn token_in(ctx: &AppContext, org_id: i64, user_id: i64) -> String {
    access_tokens::Model::create_personal(
        &ctx.db,
        org_id,
        user_id,
        &PersonalParams {
            name: "Laptop".to_string(),
        },
        RESOURCE,
    )
    .await
    .unwrap()
    .1
}

#[tokio::test]
#[serial]
async fn a_beta_token_stops_and_an_acme_token_keeps_working() {
    request::<App, _, _>(|request, ctx| async move {
        let r = setup(&request, &ctx).await;
        let in_acme = token_in(&ctx, r.acme, r.alice_id).await;
        let in_beta = token_in(&ctx, r.beta, r.alice_id).await;
        bob_removes_alice(&request, &r).await;

        let refused = rpc(&request, &in_beta, "tools/list", json!({})).await;
        assert_eq!(refused.status, 401);
        let (error, body) = tool(&request, &in_acme, "list_projects", json!({})).await;
        assert!(!error, "{body}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_beta_allow_page_is_refused_after_removal() {
    request::<App, _, _>(|request, ctx| async move {
        let r = setup(&request, &ctx).await;
        let client_id = register(&request, CLAUDE).await;
        bob_removes_alice(&request, &r).await;

        let res = post_decision(
            &request,
            &r.in_beta.cookie,
            &client_id,
            CLAUDE,
            "allow",
            Some(&r.beta.to_string()),
        )
        .await;
        assert_ne!(res.status_code(), 302, "nothing granted");
        assert!(res.text().contains("Start again from Claude"));
        assert_eq!(oauth_codes::Entity::find().count(&ctx.db).await.unwrap(), 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn beta_logins_cannot_be_revealed_after_removal() {
    request::<App, _, _>(|request, ctx| async move {
        let r = setup(&request, &ctx).await;
        // Alice is an admin of Beta, so she can add and see its logins.
        let mut admin = r.beta_membership.clone().into_active_model();
        admin.role = ActiveValue::Set(memberships::role::ADMIN.to_string());
        let in_beta_m = admin.update(&ctx.db).await.unwrap();
        let folder = folders::Model::list_for_org(&ctx.db, r.beta)
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.folder.name == "Logins")
            .unwrap()
            .folder;
        let res = request
            .post(&format!("/documents/folders/{}/logins", folder.id))
            .as_member(&r.in_beta)
            .form(&[
                ("title", "Bank"),
                ("username", "office@example.com"),
                ("password", "example-pass-123"),
            ])
            .await;
        assert_eq!(res.status_code(), 303, "{}", res.text());
        let login = logins::Entity::find().one(&ctx.db).await.unwrap().unwrap();
        let res = request
            .post("/documents/logins/unlock")
            .as_member(&r.in_beta)
            .form(&[("password", USER_PASSWORD)])
            .await;
        assert_eq!(res.status_code(), 200);
        let reveal = |who: TestMember| {
            let request = &request;
            async move {
                request
                    .post(&format!("/documents/logins/{}/reveal", login.id))
                    .as_member(&who)
                    .form(&[("field", "password")])
                    .await
            }
        };
        assert_eq!(reveal(r.in_beta.clone()).await.status_code(), 200);

        let bob = bob(&request, r.beta).await;
        remove(&request, &bob, &in_beta_m).await;

        let res = reveal(r.in_beta.clone()).await;
        assert_eq!(res.status_code(), 409);
        assert!(!res.text().contains("example-pass-123"));
        let alice = users::Model::find_by_id(&ctx.db, r.alice_id).await.unwrap();
        assert_eq!(alice.secrets_unlocked_until, None);
        assert_eq!(alice.secrets_unlocked_sid, None);
        assert_eq!(alice.secrets_unlocked_org_id, None);
    })
    .await;
}

/// Carol's only org is Beta.
async fn carol_only_in_beta(request: &TestServer, ctx: &AppContext) -> (TestMember, TestMember) {
    let bob = sign_up_member(request, "Beta", "bob", "bob@example.com").await;
    let carol = join(request, ctx, "beta", "carol", "carol@example.com").await;
    let carol_id = users::Model::find_by_email(&ctx.db, "carol@example.com")
        .await
        .unwrap()
        .id;
    let carol_m = approve_as_owner(
        ctx,
        &memberships::Model::default_for_user(&ctx.db, carol_id)
            .await
            .unwrap()
            .unwrap(),
    )
    .await;
    remove(request, &bob, &carol_m).await;
    (bob, carol)
}

#[tokio::test]
#[serial]
async fn removal_from_your_only_org_shows_the_removed_page() {
    request::<App, _, _>(|request, ctx| async move {
        let (_, carol) = carol_only_in_beta(&request, &ctx).await;
        let res = get(&request, &carol, "/dashboard").await;
        assert_eq!(res.status_code(), 403);
        assert!(res.text().contains("You were removed from Beta"));
        // Still signed in: the page offers Sign out, and switching is still
        // answered for her (a 404, as she has no org to switch to).
        assert!(res.text().contains("Sign out"));
        let res = request
            .post("/orgs/1/switch")
            .add_header(carol.0.clone(), carol.1.clone())
            .await;
        assert_eq!(res.status_code(), 404, "not sent to sign in");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn htmx_with_no_active_org_is_sent_home() {
    request::<App, _, _>(|request, ctx| async move {
        let (_, carol) = carol_only_in_beta(&request, &ctx).await;
        let res = request
            .get("/notifications/unread")
            .as_member(&carol)
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 409);
        assert_eq!(header(&res, "HX-Redirect").as_deref(), Some("/dashboard"));
        assert_eq!(header(&res, "X-Org-Gone").as_deref(), Some("1"));
        assert_eq!(header(&res, "cache-control").as_deref(), Some("no-store"));
        // A plain page load gets the page itself.
        assert_eq!(get(&request, &carol, "/tasks").await.status_code(), 403);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn removal_is_logged_in_beta_only() {
    request::<App, _, _>(|request, ctx| async move {
        let r = setup(&request, &ctx).await;
        bob_removes_alice(&request, &r).await;

        let events = login_events::Entity::find().all(&ctx.db).await.unwrap();
        let removed: Vec<_> = events
            .iter()
            .filter(|e| e.action == login_events::action::MEMBER_REMOVED)
            .collect();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].organisation_id, r.beta);
        assert_eq!(removed[0].username, "bob");
        assert_eq!(removed[0].detail.as_deref(), Some("alice"));
        assert!(events.iter().all(|e| e.organisation_id != r.acme));
        assert!(events
            .iter()
            .all(|e| e.action != login_events::action::SIGN_OUT_EVERYWHERE));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn removal_sends_no_email_and_no_notification() {
    request::<App, _, _>(|request, ctx| async move {
        let r = setup(&request, &ctx).await;
        let bob = bob(&request, r.beta).await;
        let mail_before = ctx.mailer.as_ref().unwrap().deliveries().count;
        let notices_before = notifications::Entity::find().count(&ctx.db).await.unwrap();

        remove(&request, &bob, &r.beta_membership).await;

        assert_eq!(ctx.mailer.as_ref().unwrap().deliveries().count, mail_before);
        assert_eq!(
            notifications::Entity::find().count(&ctx.db).await.unwrap(),
            notices_before
        );
        let theirs = notifications::Entity::find()
            .filter(notifications::Column::UserId.eq(r.alice_id))
            .count(&ctx.db)
            .await
            .unwrap();
        assert_eq!(theirs, 0);
    })
    .await;
}
