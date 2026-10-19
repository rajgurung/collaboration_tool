//! A person with memberships in two organisations.
use collab::{
    app::App,
    models::{
        access_tokens::{self, PersonalParams},
        folders, login_events, logins, memberships, organisations, users,
    },
};
use loco_rs::{app::AppContext, prelude::*};
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait, IntoActiveModel, QueryFilter};
use serde_json::json;
use serial_test::serial;

use super::{
    mcp::{rpc, tool, RESOURCE},
    prepare_data::{join, sign_up},
    tasks::project_in,
};

/// Alice owns Acme, then becomes an active member of Beta (owned by Bob).
/// Returns Alice's id and her two memberships, Acme's first.
async fn alice_in_two_orgs(
    request: &loco_rs::TestServer,
    ctx: &AppContext,
) -> (
    (axum::http::HeaderName, axum::http::HeaderValue),
    i64,
    memberships::Model,
    memberships::Model,
) {
    let cookie = sign_up(request, "Acme", "alice", "alice@example.com").await;
    sign_up(request, "Beta", "bob", "bob@example.com").await;
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

async fn set_status(ctx: &AppContext, membership: memberships::Model, status: &str) {
    let mut active = membership.into_active_model();
    active.status = ActiveValue::Set(status.to_string());
    active.update(&ctx.db).await.unwrap();
}

/// The organisation name in the page's top bar.
fn org_heading(html: &str) -> &str {
    let start = html.find("<h1").expect("a heading");
    let text = &html[start..];
    let text = &text[text.find('>').unwrap() + 1..];
    &text[..text.find("</h1>").unwrap()]
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
        join(&request, "acme", "carol", "carol@example.com").await;
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
