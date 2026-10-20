use std::time::Duration;

use collab::{
    app::App,
    models::{
        access_tokens::{self, CodeExchange, Issued, PersonalParams},
        login_events::Client,
        memberships, oauth_clients,
        oauth_codes::{self, Consent},
        organisations::{self, SignupParams},
        users,
    },
};
use loco_rs::{app::AppContext, prelude::TenantActiveModelExt, testing::prelude::*};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, IntoActiveModel, PaginatorTrait,
    QueryFilter, QuerySelect, TransactionTrait,
};
use serial_test::serial;

macro_rules! configure_insta {
    ($($expr:expr),*) => {
        let mut settings = insta::Settings::clone_current();
        settings.set_prepend_module_to_snapshot(false);
        let _guard = settings.bind_to_scope();
    };
}

/// The `Memberships` entity matches the table its migration created.
///
/// Selecting every column is the cheapest assertion that says something true:
/// it fails if the migration and the entity disagree — a renamed column, a
/// type that does not round-trip, a migration that never ran — which is the
/// most common way a generated model breaks. Extend it as the model grows.
#[tokio::test]
#[serial]
async fn can_query_memberships() {
    configure_insta!();

    let boot = boot_test::<App>().await.unwrap();
    seed::<App>(&boot.app_context).await.unwrap();

    // The query is the assertion. Bind the result and compare it once you have
    // seed data to compare against, e.g.:
    //
    // let items = memberships::Entity::find().all(&boot.app_context.db).await.unwrap();
    // assert_debug_snapshot!(items);
    memberships::Entity::find()
        .all(&boot.app_context.db)
        .await
        .expect("`memberships` should be queryable — entity and migration must agree");
}

async fn sign_up(ctx: &AppContext, org: &str, owner: &str) -> (i64, i64) {
    let (user, org) = organisations::Model::sign_up(
        &ctx.db,
        &SignupParams {
            organisation_name: org.to_string(),
            name: owner.to_string(),
            email: format!("{owner}@example.com"),
            password: "correct-horse".to_string(),
        },
    )
    .await
    .unwrap();
    (user.id, org.id)
}

async fn add_member(
    ctx: &AppContext,
    org_id: i64,
    user_id: i64,
    username: &str,
) -> Result<memberships::Model, sea_orm::DbErr> {
    memberships::ActiveModel {
        user_id: ActiveValue::Set(user_id),
        username: ActiveValue::Set(username.to_string()),
        role: ActiveValue::Set(memberships::role::MEMBER.to_string()),
        status: ActiveValue::Set(memberships::status::ACTIVE.to_string()),
        ..Default::default()
    }
    .set_tenant(org_id)
    .unwrap()
    .insert(&ctx.db)
    .await
}

#[tokio::test]
#[serial]
async fn one_membership_per_organisation_per_user() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, acme) = sign_up(ctx, "Acme", "alice").await;
    let (_, beta) = sign_up(ctx, "Beta", "bob").await;

    // A different username, so only the (organisation, user) index can object.
    let err = add_member(ctx, acme, alice, "alice2").await.unwrap_err();
    assert!(
        err.to_string().contains("idx-memberships-org-user"),
        "{err}"
    );

    let in_beta = add_member(ctx, beta, alice, "alice").await.unwrap();
    assert_eq!(in_beta.organisation_id, beta);
    let all = memberships::Model::for_user(&ctx.db, alice).await.unwrap();
    assert_eq!(all.len(), 2);
}

async fn add_with_status(ctx: &AppContext, org_id: i64, user_id: i64, status: &str) {
    let membership = add_member(ctx, org_id, user_id, "alice").await.unwrap();
    let mut membership: memberships::ActiveModel = membership.into();
    membership.status = ActiveValue::Set(status.to_string());
    membership.update(&ctx.db).await.unwrap();
}

#[tokio::test]
#[serial]
async fn switcher_counts_counts_active_and_pending_only() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _) = sign_up(ctx, "Acme", "alice").await;
    assert_eq!(
        memberships::Model::switcher_counts(&ctx.db, alice)
            .await
            .unwrap(),
        (1, 0)
    );
    for (name, status) in [
        ("Beta", memberships::status::ACTIVE),
        ("Gamma", memberships::status::PENDING),
        ("Delta", memberships::status::REJECTED),
        ("Epsilon", memberships::status::REMOVED),
    ] {
        let (_, org) = sign_up(ctx, name, &name.to_lowercase()).await;
        add_with_status(ctx, org, alice, status).await;
    }
    assert_eq!(
        memberships::Model::switcher_counts(&ctx.db, alice)
            .await
            .unwrap(),
        (2, 1)
    );
    assert_eq!(
        memberships::Model::switcher_counts(&ctx.db, 999_999)
            .await
            .unwrap(),
        (0, 0)
    );
}

#[tokio::test]
#[serial]
async fn for_user_with_orgs_sorts_active_by_name_then_pending() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _) = sign_up(ctx, "Northwind Bakery", "alice").await;
    for (name, status) in [
        ("zen scale", memberships::status::ACTIVE),
        ("Himalayan Ritual", memberships::status::PENDING),
        ("Acme Studio", memberships::status::ACTIVE),
        ("Brightside", memberships::status::PENDING),
        ("Declined Co", memberships::status::REJECTED),
        ("Removed Co", memberships::status::REMOVED),
    ] {
        let owner = name.replace(' ', "").to_lowercase();
        let (_, org) = sign_up(ctx, name, &owner).await;
        add_with_status(ctx, org, alice, status).await;
    }
    let names: Vec<(String, String)> = memberships::Model::for_user_with_orgs(&ctx.db, alice)
        .await
        .unwrap()
        .into_iter()
        .map(|(m, org)| (org.name, m.status))
        .collect();
    let expected = [
        ("Acme Studio", "active"),
        ("Northwind Bakery", "active"),
        ("zen scale", "active"),
        ("Brightside", "pending"),
        ("Himalayan Ritual", "pending"),
    ];
    assert_eq!(
        names,
        expected
            .iter()
            .map(|(n, s)| (n.to_string(), s.to_string()))
            .collect::<Vec<_>>()
    );
}

pub(super) const RESOURCE: &str = "http://localhost:5150/mcp";
const CLAUDE: &str = "https://claude.ai/api/mcp/auth_callback";
const VERIFIER: &str = "a-fairly-long-pkce-verifier-that-is-at-least-43-characters-long";

/// Alice owns Acme and is an active member of Beta, which Bob owns.
pub(super) struct TwoOrgs {
    pub alice: i64,
    pub bob: i64,
    pub acme: i64,
    pub beta: i64,
    pub in_beta: memberships::Model,
}

pub(super) async fn alice_in_two_orgs(ctx: &AppContext) -> TwoOrgs {
    let (alice, acme) = sign_up(ctx, "Acme", "alice").await;
    let (bob, beta) = sign_up(ctx, "Beta", "bob").await;
    let in_beta = add_member(ctx, beta, alice, "alice").await.unwrap();
    TwoOrgs {
        alice,
        bob,
        acme,
        beta,
        in_beta,
    }
}

fn client() -> Client {
    Client {
        ip: "127.0.0.1".to_string(),
        user_agent: "test".to_string(),
    }
}

async fn remove_alice_from_beta(ctx: &AppContext, orgs: &TwoOrgs) {
    memberships::Model::remove(ctx, orgs.beta, orgs.bob, orgs.in_beta.id, &client())
        .await
        .unwrap();
}

pub(super) async fn personal_token(ctx: &AppContext, org_id: i64, user_id: i64) -> String {
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

pub(super) async fn claude_client(ctx: &AppContext) -> oauth_clients::Model {
    oauth_clients::Model::register(
        &ctx.db,
        &oauth_clients::RegisterParams {
            client_name: Some("Claude".to_string()),
            redirect_uris: vec![CLAUDE.to_string()],
        },
    )
    .await
    .unwrap()
}

/// An unused authorization code for `user_id` in `org_id`.
pub(super) async fn code_for(
    ctx: &AppContext,
    client: &oauth_clients::Model,
    org_id: i64,
    user_id: i64,
) -> String {
    let challenge = oauth_codes::challenge_for(VERIFIER);
    oauth_codes::Model::issue(
        &ctx.db,
        &Consent {
            client,
            org_id,
            user_id,
            redirect_uri: CLAUDE,
            code_challenge: &challenge,
            resource: RESOURCE,
            scope: access_tokens::SCOPE,
        },
    )
    .await
    .unwrap()
    .1
}

pub(super) async fn exchange(
    db: &sea_orm::DatabaseConnection,
    client: &oauth_clients::Model,
    code: &str,
) -> Option<Issued> {
    access_tokens::Model::exchange_code(
        db,
        &CodeExchange {
            code,
            code_verifier: VERIFIER,
            redirect_uri: CLAUDE,
            resource: RESOURCE,
            client,
        },
    )
    .await
    .unwrap()
}

/// OAuth tokens for `user_id` in `org_id`.
pub(super) async fn oauth_tokens(ctx: &AppContext, org_id: i64, user_id: i64) -> Issued {
    let client = claude_client(ctx).await;
    let code = code_for(ctx, &client, org_id, user_id).await;
    exchange(&ctx.db, &client, &code).await.unwrap()
}

pub(super) async fn usable(ctx: &AppContext, token: &str) -> bool {
    access_tokens::Model::find_usable(&ctx.db, token, RESOURCE)
        .await
        .unwrap()
        .is_some()
}

/// A second connection pool to the test database. The app's pool has one
/// connection, so calls through it never overlap; races need two.
pub(super) async fn second_connection(ctx: &AppContext) -> sea_orm::DatabaseConnection {
    sea_orm::Database::connect(&ctx.config.database.uri)
        .await
        .unwrap()
}

/// How long the racing transaction holds its lock before committing.
pub(super) const HOLD: Duration = Duration::from_millis(500);

#[tokio::test]
#[serial]
async fn removal_keeps_the_session_version() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    let before = users::Model::find_by_id(&ctx.db, orgs.alice).await.unwrap();

    remove_alice_from_beta(ctx, &orgs).await;

    let after = users::Model::find_by_id(&ctx.db, orgs.alice).await.unwrap();
    assert_eq!(after.session_version, before.session_version);
    assert!(
        memberships::Model::active_in(&ctx.db, orgs.acme, orgs.alice)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
#[serial]
async fn removal_ends_an_unlock_for_that_org() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    assert!(
        users::Model::unlock_secrets(&ctx.db, orgs.alice, "sid-1", orgs.beta)
            .await
            .unwrap()
    );

    remove_alice_from_beta(ctx, &orgs).await;

    let alice = users::Model::find_by_id(&ctx.db, orgs.alice).await.unwrap();
    assert!(!alice.secrets_unlocked(Some("sid-1"), orgs.beta));
    assert_eq!(alice.secrets_unlocked_org_id, None);
    assert_eq!(alice.secrets_unlocked_sid, None);
}

#[tokio::test]
#[serial]
async fn removal_keeps_an_unlock_for_another_org() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    assert!(
        users::Model::unlock_secrets(&ctx.db, orgs.alice, "sid-1", orgs.acme)
            .await
            .unwrap()
    );

    remove_alice_from_beta(ctx, &orgs).await;

    let alice = users::Model::find_by_id(&ctx.db, orgs.alice).await.unwrap();
    assert!(alice.secrets_unlocked(Some("sid-1"), orgs.acme));
}

#[tokio::test]
#[serial]
async fn removal_revokes_tokens_in_that_org_only() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    let claude = claude_client(ctx).await;
    let acme_token = personal_token(ctx, orgs.acme, orgs.alice).await;
    let beta_token = personal_token(ctx, orgs.beta, orgs.alice).await;
    let acme_oauth = oauth_tokens(ctx, orgs.acme, orgs.alice).await;
    let beta_oauth = oauth_tokens(ctx, orgs.beta, orgs.alice).await;
    let acme_code = code_for(ctx, &claude, orgs.acme, orgs.alice).await;
    let beta_code = code_for(ctx, &claude, orgs.beta, orgs.alice).await;

    remove_alice_from_beta(ctx, &orgs).await;

    assert!(!usable(ctx, &beta_token).await);
    assert!(!usable(ctx, &beta_oauth.access_token).await);
    assert!(exchange(&ctx.db, &claude, &beta_code).await.is_none());
    assert!(usable(ctx, &acme_token).await);
    assert!(usable(ctx, &acme_oauth.access_token).await);
    assert!(
        exchange(&ctx.db, &claude, &acme_code).await.is_some(),
        "Acme's unused code still works"
    );
}

#[tokio::test]
#[serial]
async fn in_org_for_user_returns_any_status() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    let (_, gamma) = sign_up(ctx, "Gamma", "gail").await;
    let (_, delta) = sign_up(ctx, "Delta", "dina").await;
    add_with_status(ctx, gamma, orgs.alice, memberships::status::PENDING).await;
    remove_alice_from_beta(ctx, &orgs).await;

    let status = |org_id| async move {
        memberships::Model::in_org_for_user(&ctx.db, org_id, orgs.alice)
            .await
            .unwrap()
            .map(|m| m.status)
    };
    assert_eq!(status(orgs.acme).await.as_deref(), Some("active"));
    assert_eq!(status(orgs.beta).await.as_deref(), Some("removed"));
    assert_eq!(status(gamma).await.as_deref(), Some("pending"));
    assert_eq!(status(delta).await, None);
}

#[tokio::test]
#[serial]
async fn a_personal_token_is_refused_for_a_removed_membership() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    remove_alice_from_beta(ctx, &orgs).await;

    let refused = access_tokens::Model::create_personal(
        &ctx.db,
        orgs.beta,
        orgs.alice,
        &PersonalParams {
            name: "Laptop".to_string(),
        },
        RESOURCE,
    )
    .await;
    assert!(refused.is_err());
    let beta_tokens = access_tokens::Entity::find()
        .filter(access_tokens::Column::OrganisationId.eq(orgs.beta))
        .filter(access_tokens::Column::UserId.eq(orgs.alice))
        .count(&ctx.db)
        .await
        .unwrap();
    assert_eq!(beta_tokens, 0);
}

/// A removal of `membership_id` on another connection: it holds the row
/// `FOR UPDATE` for [`HOLD`], then commits it as removed.
async fn held_removal(other: &sea_orm::DatabaseConnection, membership_id: i64) {
    let txn = other.begin().await.unwrap();
    let row = memberships::Entity::find_by_id(membership_id)
        .lock_exclusive()
        .one(&txn)
        .await
        .unwrap()
        .unwrap();
    tokio::time::sleep(HOLD).await;
    let mut removed = row.into_active_model();
    removed.status = ActiveValue::Set(memberships::status::REMOVED.to_string());
    removed.update(&txn).await.unwrap();
    txn.commit().await.unwrap();
}

/// Starts after the held removal has its lock.
async fn after_the_lock<T>(work: impl std::future::Future<Output = T>) -> T {
    tokio::time::sleep(HOLD / 5).await;
    work.await
}

async fn live_beta_tokens(ctx: &AppContext, orgs: &TwoOrgs) -> u64 {
    access_tokens::Entity::find()
        .filter(access_tokens::Column::OrganisationId.eq(orgs.beta))
        .filter(access_tokens::Column::RevokedAt.is_null())
        .count(&ctx.db)
        .await
        .unwrap()
}

// Each of the next four races one way of minting against a removal that
// holds the membership `FOR UPDATE`. Each waits, then sees the removal and is
// refused. Without its `FOR SHARE` read it would see the old, active row and
// go ahead.

#[tokio::test]
#[serial]
async fn a_personal_token_waits_for_a_removal_in_progress() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    let other = second_connection(ctx).await;

    let params = PersonalParams {
        name: "Laptop".to_string(),
    };
    let mint =
        access_tokens::Model::create_personal(&ctx.db, orgs.beta, orgs.alice, &params, RESOURCE);
    let ((), minted) = tokio::join!(held_removal(&other, orgs.in_beta.id), after_the_lock(mint));
    assert!(minted.is_err(), "no personal token");
    assert_eq!(live_beta_tokens(ctx, &orgs).await, 0);
}

#[tokio::test]
#[serial]
async fn a_code_exchange_waits_for_a_removal_in_progress() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    let claude = claude_client(ctx).await;
    let code = code_for(ctx, &claude, orgs.beta, orgs.alice).await;
    let other = second_connection(ctx).await;

    let ((), exchanged) = tokio::join!(
        held_removal(&other, orgs.in_beta.id),
        after_the_lock(exchange(&ctx.db, &claude, &code))
    );
    assert!(exchanged.is_none(), "no tokens for the code");
    assert_eq!(live_beta_tokens(ctx, &orgs).await, 0);
}

#[tokio::test]
#[serial]
async fn a_refresh_waits_for_a_removal_in_progress() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    let claude = claude_client(ctx).await;
    let code = code_for(ctx, &claude, orgs.beta, orgs.alice).await;
    let earlier = exchange(&ctx.db, &claude, &code).await.unwrap();
    let other = second_connection(ctx).await;

    let refresh = access_tokens::Model::refresh(&ctx.db, &earlier.refresh_token, &claude, RESOURCE);
    let ((), refreshed) = tokio::join!(
        held_removal(&other, orgs.in_beta.id),
        after_the_lock(refresh)
    );
    assert!(refreshed.unwrap().is_none(), "no refreshed tokens");
    assert_eq!(live_beta_tokens(ctx, &orgs).await, 0);
}

#[tokio::test]
#[serial]
async fn an_unlock_waits_for_a_removal_in_progress() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    let other = second_connection(ctx).await;

    let unlock = users::Model::unlock_secrets(&ctx.db, orgs.alice, "sid-1", orgs.beta);
    let ((), unlocked) = tokio::join!(
        held_removal(&other, orgs.in_beta.id),
        after_the_lock(unlock)
    );
    assert!(!unlocked.unwrap(), "no unlock");
    let alice = users::Model::find_by_id(&ctx.db, orgs.alice).await.unwrap();
    assert_eq!(alice.secrets_unlocked_org_id, None);
}

/// Bob removes Alice from Beta while, on another connection, Alice removes
/// Bob from Acme. Each removal's `removed_by_id` takes FOR KEY SHARE on the
/// remover's account row. The other connection then locks Bob's row FOR
/// UPDATE, as `remove` used to; `remove` must take only FOR NO KEY UPDATE on
/// Alice's row, which doesn't wait on that key share, or the two deadlock and
/// one fails.
#[tokio::test]
#[serial]
async fn removals_the_other_way_round_do_not_deadlock() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    let bob_in_acme = add_member(ctx, orgs.acme, orgs.bob, "bob").await.unwrap();
    let other = second_connection(ctx).await;

    let alice_removes_bob = async {
        let txn = other.begin().await.unwrap();
        let row = memberships::Entity::find_by_id(bob_in_acme.id)
            .lock_exclusive()
            .one(&txn)
            .await
            .unwrap()
            .unwrap();
        let mut removed = row.into_active_model();
        removed.status = ActiveValue::Set(memberships::status::REMOVED.to_string());
        removed.removed_by_id = ActiveValue::Set(Some(orgs.alice));
        removed.update(&txn).await.unwrap();
        tokio::time::sleep(HOLD).await;
        users::Entity::find_by_id(orgs.bob)
            .lock_exclusive()
            .one(&txn)
            .await?;
        txn.commit().await
    };
    let bob_removes_alice = async {
        tokio::time::sleep(HOLD / 5).await;
        memberships::Model::remove(ctx, orgs.beta, orgs.bob, orgs.in_beta.id, &client()).await
    };
    let (theirs, ours) = tokio::join!(alice_removes_bob, bob_removes_alice);
    assert!(ours.is_ok(), "{ours:?}");
    assert!(theirs.is_ok(), "{theirs:?}");
}
