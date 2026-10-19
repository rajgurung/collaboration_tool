//! A person with memberships in two organisations. The database still allows
//! only one membership per user, so each test drops that unique index first.
//! Every test boots a freshly migrated database, so this does not leak.
use collab::{
    app::App,
    models::{
        access_tokens::{self, PersonalParams},
        login_events, memberships, organisations, users,
    },
};
use loco_rs::{app::AppContext, prelude::*};
use sea_orm::{ActiveModelTrait, ActiveValue, ConnectionTrait, EntityTrait, QueryFilter};
use serde_json::json;
use serial_test::serial;

use super::{
    mcp::{rpc, tool, RESOURCE},
    prepare_data::sign_up,
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
    ctx.db
        .execute_unprepared(r#"DROP INDEX "idx-memberships-user_id""#)
        .await
        .unwrap();
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
async fn for_user_lists_every_membership_and_the_oldest_is_the_default() {
    request::<App, _, _>(|request, ctx| async move {
        let (_, alice, in_acme, in_beta) = alice_in_two_orgs(&request, &ctx).await;

        let all = memberships::Model::for_user(&ctx.db, alice).await.unwrap();
        let ids: Vec<i64> = all.iter().map(|m| m.id).collect();
        assert_eq!(ids, vec![in_acme.id, in_beta.id], "oldest first");

        let default = memberships::Model::default_for_user(&ctx.db, alice)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(default.id, in_acme.id);

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
async fn pages_use_the_oldest_organisation() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, _, _) = alice_in_two_orgs(&request, &ctx).await;
        let res = request
            .get("/dashboard")
            .add_header(cookie.0.clone(), cookie.1.clone())
            .await;
        assert_eq!(res.status_code(), 200);
        let html = res.text();
        assert!(html.contains("Acme"), "shows Acme");
        assert!(!html.contains("Beta"), "not Beta");
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
        let mut removed: memberships::ActiveModel = in_beta.into();
        removed.status = ActiveValue::Set(memberships::status::REMOVED.to_string());
        removed.update(&ctx.db).await.unwrap();
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
