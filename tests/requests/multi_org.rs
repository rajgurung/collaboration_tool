//! A person with memberships in two organisations, and the `current_org`
//! cookie that picks between them.
use axum::http::HeaderValue;
use collab::{
    app::App,
    models::{
        access_tokens::{self, PersonalParams},
        folders, login_events, logins, memberships, organisations, tasks, users,
    },
};
use loco_rs::{app::AppContext, prelude::*, TestServer};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, EntityTrait, IntoActiveModel, PaginatorTrait,
    QueryFilter,
};
use serde_json::json;
use serial_test::serial;

use super::{
    mcp::{rpc, tool, RESOURCE},
    prepare_data::{join, sign_up_member, AsMember, TestMember, USER_PASSWORD},
    tasks::{project_in, task_form},
};

/// Alice owns Acme, then becomes an active member of Beta (owned by Bob).
/// Returns Alice's id and her two memberships, Acme's first.
pub(super) async fn alice_in_two_orgs(
    request: &loco_rs::TestServer,
    ctx: &AppContext,
) -> (TestMember, i64, memberships::Model, memberships::Model) {
    let cookie = sign_up_member(request, "Acme", "alice", "alice@example.com").await;
    sign_up_member(request, "Beta", "bob", "bob@example.com").await;
    let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
        .await
        .unwrap();
    let beta = organisations::Model::find_by_slug(&ctx.db, "beta")
        .await
        .unwrap();
    let in_acme = memberships::Model::default_for_user(&ctx.db, alice.id)
        .await
        .unwrap()
        .unwrap();
    let in_beta = memberships::ActiveModel {
        user_id: ActiveValue::Set(alice.id),
        username: ActiveValue::Set("alice".to_string()),
        role: ActiveValue::Set(memberships::role::MEMBER.to_string()),
        status: ActiveValue::Set(memberships::status::ACTIVE.to_string()),
        approved_at: ActiveValue::Set(Some(chrono::Utc::now().into())),
        ..Default::default()
    }
    .set_tenant(beta.id)
    .unwrap()
    .insert(&ctx.db)
    .await
    .unwrap();
    (cookie, alice.id, in_acme, in_beta)
}

pub(super) async fn set_status(ctx: &AppContext, membership: memberships::Model, status: &str) {
    let mut active = membership.into_active_model();
    active.status = ActiveValue::Set(status.to_string());
    active.update(&ctx.db).await.unwrap();
}

/// The organisation name in the page's top bar.
/// With two orgs it is the switcher's button, so only its text counts.
pub(super) fn org_heading(html: &str) -> String {
    let start = html.find("<h1").expect("a heading");
    let text = &html[start..];
    let text = &text[text.find('>').unwrap() + 1..];
    let inner = &text[..text.find("</h1>").unwrap()];
    let mut plain = String::new();
    let mut in_tag = false;
    for c in inner.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => plain.push(c),
            _ => {}
        }
    }
    plain.trim().to_string()
}

/// The session cookie plus `current_org=<value>`, as one request header.
/// Its pages are drawn for that org when `value` is an org id, else for the
/// default one.
pub(super) fn with_org(cookie: &TestMember, value: impl std::fmt::Display) -> TestMember {
    let value = value.to_string();
    let both = format!("{}; current_org={value}", cookie.1.to_str().unwrap());
    TestMember::new(
        (cookie.0.clone(), HeaderValue::from_str(&both).unwrap()),
        value.parse().unwrap_or(cookie.org_id),
    )
}

/// The response's `Set-Cookie` for this cookie name, if any.
pub(super) fn set_cookie(res: &axum_test::TestResponse, name: &str) -> Option<String> {
    res.headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .find(|c| c.starts_with(&format!("{name}=")))
}

/// Whether the response clears this cookie.
pub(super) fn clears(res: &axum_test::TestResponse, name: &str) -> bool {
    set_cookie(res, name)
        .is_some_and(|c| c.starts_with(&format!("{name}=;")) && c.contains("Max-Age=0"))
}

pub(super) async fn switch(
    request: &TestServer,
    cookie: &TestMember,
    org_id: impl std::fmt::Display,
) -> axum_test::TestResponse {
    request
        .post(&format!("/orgs/{org_id}/switch"))
        .add_header(cookie.0.clone(), cookie.1.clone())
        .await
}

async fn dashboard(request: &TestServer, cookie: &TestMember) -> axum_test::TestResponse {
    request
        .get("/dashboard")
        .add_header(cookie.0.clone(), cookie.1.clone())
        .await
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

async fn project_names(request: &loco_rs::TestServer, token: &str) -> Vec<String> {
    let (error, body) = tool(request, token, "list_projects", json!({})).await;
    assert!(!error, "{body}");
    body["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
#[serial]
async fn for_user_is_oldest_first_and_the_default_prefers_active() {
    request::<App, _, _>(|request, ctx| async move {
        let (_, alice, in_acme, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        // Beta was inserted second but is made the older one, so the order
        // comes from created_at and not from insertion order.
        let mut older = in_beta.clone().into_active_model();
        older.created_at = ActiveValue::Set(in_acme.created_at - chrono::Duration::days(1));
        let in_beta = older.update(&ctx.db).await.unwrap();

        let all = memberships::Model::for_user(&ctx.db, alice).await.unwrap();
        let ids: Vec<i64> = all.iter().map(|m| m.id).collect();
        assert_eq!(ids, vec![in_beta.id, in_acme.id], "oldest first");

        let default = memberships::Model::default_for_user(&ctx.db, alice)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(default.id, in_beta.id, "both active: the oldest");
        let by_user = memberships::Model::default_by_user(&ctx.db).await.unwrap();
        assert_eq!(by_user[&alice].id, in_beta.id);

        // Removed from the older one: the active one wins.
        set_status(&ctx, in_beta, memberships::status::REMOVED).await;
        let default = memberships::Model::default_for_user(&ctx.db, alice)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(default.id, in_acme.id, "active before older");
        let by_user = memberships::Model::default_by_user(&ctx.db).await.unwrap();
        assert_eq!(by_user[&alice].id, in_acme.id);

        assert!(memberships::Model::for_user(&ctx.db, 999_999)
            .await
            .unwrap()
            .is_empty());
        assert!(memberships::Model::default_for_user(&ctx.db, 999_999)
            .await
            .unwrap()
            .is_none());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn pages_use_the_oldest_active_organisation() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, in_acme, _) = alice_in_two_orgs(&request, &ctx).await;
        let res = request
            .get("/dashboard")
            .add_header(cookie.0.clone(), cookie.1.clone())
            .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(org_heading(&res.text()), "Acme");

        // Removed from Acme, still active in Beta: Beta, not the removed page.
        set_status(&ctx, in_acme, memberships::status::REMOVED).await;
        let res = request
            .get("/dashboard")
            .add_header(cookie.0.clone(), cookie.1.clone())
            .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(org_heading(&res.text()), "Beta");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_claude_token_works_in_its_own_organisation_only() {
    request::<App, _, _>(|request, ctx| async move {
        let (_, alice, in_acme, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        project_in(&ctx, "acme", "Acme launch").await;
        project_in(&ctx, "beta", "Beta launch").await;
        let acme_token = token_in(&ctx, in_acme.organisation_id, alice).await;
        let beta_token = token_in(&ctx, in_beta.organisation_id, alice).await;

        // The Beta token works although Alice's default org is Acme.
        assert_eq!(project_names(&request, &beta_token).await, ["Beta launch"]);
        assert_eq!(project_names(&request, &acme_token).await, ["Acme launch"]);

        // Removed from Beta: the Beta token stops, the Acme one keeps working.
        set_status(&ctx, in_beta, memberships::status::REMOVED).await;
        assert_eq!(
            rpc(&request, &beta_token, "tools/list", json!({}))
                .await
                .status,
            401
        );
        assert_eq!(project_names(&request, &acme_token).await, ["Acme launch"]);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn sign_out_everywhere_is_logged_in_every_active_organisation() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, alice, in_acme, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let res = request
            .post("/account/sign-out-everywhere")
            .add_header(cookie.0.clone(), cookie.1.clone())
            .await;
        assert_eq!(res.status_code(), 303);

        let mut orgs: Vec<i64> = login_events::Entity::find()
            .filter(login_events::Column::UserId.eq(alice))
            .filter(login_events::Column::Action.eq(login_events::action::SIGN_OUT_EVERYWHERE))
            .all(&ctx.db)
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.organisation_id)
            .collect();
        orgs.sort_unstable();
        let mut expected = vec![in_acme.organisation_id, in_beta.organisation_id];
        expected.sort_unstable();
        assert_eq!(orgs, expected);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_claude_token_for_a_pending_membership_is_refused() {
    request::<App, _, _>(|request, ctx| async move {
        let (_, alice, in_acme, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        project_in(&ctx, "acme", "Acme launch").await;
        let beta_token = token_in(&ctx, in_beta.organisation_id, alice).await;
        let acme_token = token_in(&ctx, in_acme.organisation_id, alice).await;
        set_status(&ctx, in_beta, memberships::status::PENDING).await;

        assert_eq!(
            rpc(&request, &beta_token, "tools/list", json!({}))
                .await
                .status,
            401
        );
        assert_eq!(project_names(&request, &acme_token).await, ["Acme launch"]);
    })
    .await;
}

/// Adds a login row to the organisation's Logins folder. Only its existence
/// matters here, so the secrets are placeholders.
async fn a_login_in(ctx: &AppContext, org_id: i64) {
    let folder = folders::Model::list_for_org(&ctx.db, org_id)
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.folder.name == "Logins")
        .unwrap()
        .folder;
    logins::ActiveModel {
        folder_id: ActiveValue::Set(folder.id),
        title: ActiveValue::Set("Bank".to_string()),
        username: ActiveValue::Set("office@example.com".to_string()),
        url: ActiveValue::Set(String::new()),
        notes: ActiveValue::Set(String::new()),
        password_ct: ActiveValue::Set(vec![0]),
        password_nonce: ActiveValue::Set(vec![0]),
        dek_version: ActiveValue::Set(1),
        ..Default::default()
    }
    .set_tenant(org_id)
    .unwrap()
    .insert(&ctx.db)
    .await
    .unwrap();
}

#[tokio::test]
#[serial]
async fn a_password_reset_tells_the_owners_of_every_active_organisation() {
    request::<App, _, _>(|request, ctx| async move {
        let (_, _, in_acme, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        // Alice owns Acme, so another owner is needed there to be told.
        join(&request, &ctx, "acme", "carol", "carol@example.com").await;
        let carol = users::Model::find_by_email(&ctx.db, "carol@example.com")
            .await
            .unwrap();
        let mut owner = memberships::Model::default_for_user(&ctx.db, carol.id)
            .await
            .unwrap()
            .unwrap()
            .into_active_model();
        owner.role = ActiveValue::Set(memberships::role::OWNER.to_string());
        owner.status = ActiveValue::Set(memberships::status::ACTIVE.to_string());
        owner.update(&ctx.db).await.unwrap();
        // Admins see every login, so Alice can see logins in both.
        let mut admin = in_beta.into_active_model();
        admin.role = ActiveValue::Set(memberships::role::ADMIN.to_string());
        let in_beta = admin.update(&ctx.db).await.unwrap();
        a_login_in(&ctx, in_acme.organisation_id).await;
        a_login_in(&ctx, in_beta.organisation_id).await;

        request
            .post("/forgot")
            .form(&json!({ "email": "alice@example.com" }))
            .await;
        let token = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap()
            .reset_token
            .unwrap();
        let res = request
            .post(&format!("/reset/{token}"))
            .form(&json!({
                "password": "new-password-1",
                "password_confirmation": "new-password-1",
            }))
            .await;
        assert_eq!(res.status_code(), 303);

        let notices: Vec<String> = ctx
            .mailer
            .as_ref()
            .unwrap()
            .deliveries()
            .messages
            .iter()
            .map(|m| m.replace("=\r\n", ""))
            .filter(|m| m.contains("alice's password was changed"))
            .collect();
        assert_eq!(notices.len(), 2, "one owner in each organisation");
        let to_carol = notices
            .iter()
            .find(|m| m.contains("To: carol@example.com"))
            .unwrap();
        assert!(to_carol.contains("in Acme") && !to_carol.contains("Beta"));
        let to_bob = notices
            .iter()
            .find(|m| m.contains("To: bob@example.com"))
            .unwrap();
        assert!(to_bob.contains("in Beta") && !to_bob.contains("Acme"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn switching_sets_the_cookie_and_pages_follow_it() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, in_acme, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let (acme, beta) = (in_acme.organisation_id, in_beta.organisation_id);

        let res = switch(&request, &cookie, beta).await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(res.headers()["location"], "/dashboard?switched=1");
        let set = set_cookie(&res, "current_org").unwrap();
        assert!(set.starts_with(&format!("current_org={beta};")), "{set}");
        let res = dashboard(&request, &with_org(&cookie, beta)).await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(org_heading(&res.text()), "Beta");

        let res = switch(&request, &with_org(&cookie, beta), acme).await;
        let set = set_cookie(&res, "current_org").unwrap();
        assert!(set.starts_with(&format!("current_org={acme};")), "{set}");
        let res = dashboard(&request, &with_org(&cookie, acme)).await;
        assert_eq!(org_heading(&res.text()), "Acme");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn switching_over_htmx_returns_hx_redirect_with_the_cookies() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, _, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let res = request
            .post(&format!("/orgs/{}/switch", in_beta.organisation_id))
            .add_header(cookie.0.clone(), cookie.1.clone())
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(res.headers()["HX-Redirect"], "/dashboard?switched=1");
        let set = set_cookie(&res, "current_org").unwrap();
        assert!(set.starts_with(&format!("current_org={};", in_beta.organisation_id)));
        assert!(clears(&res, "acting_org"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_current_org_cookie_is_http_only_lax_and_lasts_a_session() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, _, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let res = switch(&request, &cookie, in_beta.organisation_id).await;
        let set = set_cookie(&res, "current_org").unwrap();
        let session = ctx.config.get_jwt_config().unwrap().expiration;
        assert!(set.contains("HttpOnly"), "{set}");
        assert!(set.contains("SameSite=Lax"), "{set}");
        assert!(set.contains("Path=/"), "{set}");
        assert!(set.contains(&format!("Max-Age={session}")), "{set}");
        assert!(!set.contains("Secure"), "no Secure in test: {set}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_valid_cookie_sends_no_clearing_cookie() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, _, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let res = dashboard(&request, &with_org(&cookie, in_beta.organisation_id)).await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(set_cookie(&res, "current_org"), None);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn switching_to_an_org_you_are_not_active_in_is_a_404() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, _, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        sign_up_member(&request, "Globex", "carol", "carol@example.com").await;
        let globex = organisations::Model::find_by_slug(&ctx.db, "globex")
            .await
            .unwrap();
        for id in [globex.id, 999_999] {
            let res = switch(&request, &cookie, id).await;
            assert_eq!(res.status_code(), 404, "org {id}");
            assert_eq!(set_cookie(&res, "current_org"), None);
        }
        let beta = in_beta.organisation_id;
        let mut membership = in_beta;
        for status in [
            memberships::status::PENDING,
            memberships::status::REJECTED,
            memberships::status::REMOVED,
        ] {
            let mut active = membership.into_active_model();
            active.status = ActiveValue::Set(status.to_string());
            membership = active.update(&ctx.db).await.unwrap();
            let res = switch(&request, &cookie, beta).await;
            assert_eq!(res.status_code(), 404, "{status}");
            assert_eq!(set_cookie(&res, "current_org"), None);
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn switching_needs_a_signed_in_user() {
    request::<App, _, _>(|request, ctx| async move {
        let (_, _, _, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let res = request
            .post(&format!("/orgs/{}/switch", in_beta.organisation_id))
            .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(res.headers()["location"], "/login");
        assert_eq!(set_cookie(&res, "current_org"), None);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_forged_cookie_falls_back_and_is_cleared() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, _, _) = alice_in_two_orgs(&request, &ctx).await;
        sign_up_member(&request, "Globex", "carol", "carol@example.com").await;
        let globex = organisations::Model::find_by_slug(&ctx.db, "globex")
            .await
            .unwrap();
        let res = dashboard(&request, &with_org(&cookie, globex.id)).await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(org_heading(&res.text()), "Acme");
        assert!(clears(&res, "current_org"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_garbage_cookie_falls_back_and_is_cleared() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, _, _) = alice_in_two_orgs(&request, &ctx).await;
        let res = dashboard(&request, &with_org(&cookie, "abc")).await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(org_heading(&res.text()), "Acme");
        assert!(clears(&res, "current_org"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_cookie_for_a_deleted_org_falls_back() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, _, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let beta = in_beta.organisation_id;
        ctx.db
            .execute_unprepared(&format!("DELETE FROM organisations WHERE id = {beta}"))
            .await
            .unwrap();
        let res = dashboard(&request, &with_org(&cookie, beta)).await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(org_heading(&res.text()), "Acme");
        assert!(clears(&res, "current_org"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_cookie_for_an_org_you_were_removed_from_falls_back() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, in_acme, _) = alice_in_two_orgs(&request, &ctx).await;
        let acme = in_acme.organisation_id;
        set_status(&ctx, in_acme, memberships::status::REMOVED).await;
        let res = dashboard(&request, &with_org(&cookie, acme)).await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(org_heading(&res.text()), "Beta");
        assert!(clears(&res, "current_org"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_waiting_page_shows_only_without_an_active_membership() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, in_acme, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let acme = in_acme.organisation_id;
        set_status(&ctx, in_acme, memberships::status::PENDING).await;
        let res = dashboard(&request, &with_org(&cookie, acme)).await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(org_heading(&res.text()), "Beta");
        assert!(clears(&res, "current_org"));

        set_status(&ctx, in_beta, memberships::status::REMOVED).await;
        let res = dashboard(&request, &with_org(&cookie, acme)).await;
        assert_eq!(res.status_code(), 403);
        assert!(res.text().contains("Acme"), "the Acme waiting page");
        assert!(clears(&res, "current_org"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn with_a_beta_cookie_acme_tasks_404_and_an_acme_tab_is_refused() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, in_acme, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let acme_project = project_in(&ctx, "acme", "Acme launch").await;
        let beta_project = project_in(&ctx, "beta", "Beta launch").await;
        let res = request
            .post("/tasks")
            .as_member(&cookie)
            .form(&task_form("Acme task", acme_project.id, ""))
            .await;
        assert!(res.status_code().is_redirection(), "{}", res.text());
        let acme_task = task_titled(&ctx, "Acme task").await;
        assert_eq!(acme_task.organisation_id, in_acme.organisation_id);

        let in_beta_cookie = with_org(&cookie, in_beta.organisation_id);
        let res = request
            .get(&format!("/tasks/{}", acme_task.id))
            .add_header(in_beta_cookie.0.clone(), in_beta_cookie.1.clone())
            .await;
        assert_eq!(res.status_code(), 404);

        // A form drawn for Acme, posted with the Beta cookie, is refused and
        // saves nothing anywhere.
        let res = request
            .post("/tasks")
            .add_header(in_beta_cookie.0.clone(), in_beta_cookie.1.clone())
            .form(&json!({
                "title": "Acme tab task",
                "project_id": acme_project.id.to_string(),
                "_org": in_acme.organisation_id.to_string(),
            }))
            .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(
            res.headers()["location"],
            format!("/dashboard?from={}", in_acme.organisation_id)
        );
        assert_eq!(
            tasks::Entity::find()
                .filter(tasks::Column::Title.eq("Acme tab task"))
                .count(&ctx.db)
                .await
                .unwrap(),
            0
        );

        // A form drawn for Beta goes to Beta.
        let res = request
            .post("/tasks")
            .as_member(&in_beta_cookie)
            .form(&task_form("Beta task", beta_project.id, ""))
            .await;
        assert!(res.status_code().is_redirection(), "{}", res.text());
        let beta_task = task_titled(&ctx, "Beta task").await;
        assert_eq!(beta_task.organisation_id, in_beta.organisation_id);
    })
    .await;
}

async fn task_titled(ctx: &AppContext, title: &str) -> tasks::Model {
    tasks::Entity::find()
        .filter(tasks::Column::Title.eq(title))
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[serial]
async fn an_acme_token_acts_in_acme_whatever_the_cookie() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, alice, in_acme, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        project_in(&ctx, "acme", "Acme launch").await;
        project_in(&ctx, "beta", "Beta launch").await;
        let acme_token = token_in(&ctx, in_acme.organisation_id, alice).await;
        let in_beta_cookie = with_org(&cookie, in_beta.organisation_id);
        let res = request
            .post("/mcp")
            .add_header("authorization", format!("Bearer {acme_token}"))
            .add_header("accept", "application/json, text/event-stream")
            .add_header(in_beta_cookie.0.clone(), in_beta_cookie.1.clone())
            .json(&json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "list_projects", "arguments": {} },
            }))
            .await;
        assert_eq!(res.status_code(), 200);
        let text = res.text();
        assert!(text.contains("Acme launch"), "{text}");
        assert!(!text.contains("Beta launch"), "{text}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn switching_ends_an_unlock_of_logins() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, alice, in_acme, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let res = unlock(&request, &cookie).await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let unlocked = users::Model::find_by_id(&ctx.db, alice).await.unwrap();
        assert_eq!(
            unlocked.secrets_unlocked_org_id,
            Some(in_acme.organisation_id)
        );

        switch(&request, &cookie, in_beta.organisation_id).await;
        let after = users::Model::find_by_id(&ctx.db, alice).await.unwrap();
        assert_eq!(after.secrets_unlocked_until, None);
        assert_eq!(after.secrets_unlocked_org_id, None);
        assert!(!after.secrets_unlocked(
            unlocked.secrets_unlocked_sid.as_deref(),
            in_acme.organisation_id
        ));
    })
    .await;
}

async fn unlock(request: &TestServer, cookie: &TestMember) -> axum_test::TestResponse {
    request
        .post("/documents/logins/unlock")
        .as_member(cookie)
        .form(&[("password", USER_PASSWORD)])
        .await
}

async fn reveal(request: &TestServer, cookie: &TestMember, id: i64) -> axum_test::TestResponse {
    request
        .post(&format!("/documents/logins/{id}/reveal"))
        .as_member(cookie)
        .form(&[("field", "password")])
        .await
}

#[tokio::test]
#[serial]
async fn an_acme_unlock_does_not_reveal_beta_logins() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, _, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let beta = in_beta.organisation_id;
        // Admins see every login in Beta.
        let mut admin = in_beta.into_active_model();
        admin.role = ActiveValue::Set(memberships::role::ADMIN.to_string());
        admin.update(&ctx.db).await.unwrap();
        let in_beta_cookie = with_org(&cookie, beta);
        let folder = folders::Model::list_for_org(&ctx.db, beta)
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.folder.name == "Logins")
            .unwrap()
            .folder;
        let res = request
            .post(&format!("/documents/folders/{}/logins", folder.id))
            .as_member(&in_beta_cookie)
            .form(&[
                ("title", "Beta bank"),
                ("username", "office@example.com"),
                ("password", "beta-secret-123"),
            ])
            .await;
        assert_eq!(res.status_code(), 303, "{}", res.text());
        let login = logins::Entity::find().one(&ctx.db).await.unwrap().unwrap();

        // Unlocked in Acme (no cookie), then the cookie edited to Beta.
        assert_eq!(unlock(&request, &cookie).await.status_code(), 200);
        let res = reveal(&request, &in_beta_cookie, login.id).await;
        assert_eq!(res.status_code(), 423, "{}", res.text());
        assert!(!res.text().contains("beta-secret-123"));

        // Unlocked in Beta itself, the same reveal works.
        assert_eq!(unlock(&request, &in_beta_cookie).await.status_code(), 200);
        let res = reveal(&request, &in_beta_cookie, login.id).await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        assert!(res.text().contains("beta-secret-123"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn logout_clears_current_org_and_acting_org() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, _, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let in_beta_cookie = with_org(&cookie, in_beta.organisation_id);
        let res = request
            .post("/logout")
            .add_header(in_beta_cookie.0.clone(), in_beta_cookie.1.clone())
            .await;
        assert_eq!(res.status_code(), 303);
        assert!(clears(&res, "auth_token"));
        assert!(clears(&res, "current_org"));
        assert!(clears(&res, "acting_org"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn signing_in_clears_a_leftover_current_org() {
    request::<App, _, _>(|request, ctx| async move {
        let (_, _, _, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let res = request
            .post("/login")
            .add_header(
                "cookie",
                format!("current_org={}; acting_org=1", in_beta.organisation_id),
            )
            .form(&json!({ "email": "alice@example.com", "password": USER_PASSWORD }))
            .await;
        assert_eq!(res.status_code(), 303);
        assert!(set_cookie(&res, "auth_token").is_some());
        assert!(clears(&res, "current_org"));
        assert!(clears(&res, "acting_org"));

        // Signing up on a shared browser clears them too.
        let res = request
            .post("/signup")
            .add_header("cookie", format!("current_org={}", in_beta.organisation_id))
            .form(&json!({
                "organisation_name": "Globex",
                "name": "carol",
                "email": "carol@example.com",
                "password": USER_PASSWORD,
            }))
            .await;
        assert_eq!(res.status_code(), 303);
        assert!(clears(&res, "current_org"));
        assert!(clears(&res, "acting_org"));

        // And so does joining.
        let res = request
            .post("/join/acme")
            .add_header(
                "cookie",
                format!("current_org={}; acting_org=1", in_beta.organisation_id),
            )
            .form(&json!({
                "name": "dave",
                "email": "dave@example.com",
                "password": USER_PASSWORD,
            }))
            .await;
        assert_eq!(res.status_code(), 303, "{}", res.text());
        assert!(clears(&res, "current_org"));
        assert!(clears(&res, "acting_org"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn sign_out_everywhere_keeps_this_browsers_org() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, _, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let beta = in_beta.organisation_id;
        let in_beta_cookie = with_org(&cookie, beta);
        let res = request
            .post("/account/sign-out-everywhere")
            .add_header(in_beta_cookie.0.clone(), in_beta_cookie.1.clone())
            .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(set_cookie(&res, "current_org"), None);

        let fresh = TestMember::new(
            super::prepare_data::cookie_from(&res.headers().clone()),
            cookie.org_id,
        );
        let res = dashboard(&request, &with_org(&fresh, beta)).await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(org_heading(&res.text()), "Beta");
    })
    .await;
}
