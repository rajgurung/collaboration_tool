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

// Asking to join with an existing account.

async fn org(ctx: &AppContext, id: i64) -> organisations::Model {
    organisations::Model::find_by_id(&ctx.db, id).await.unwrap()
}

fn ask(name: &str) -> memberships::JoinParams {
    memberships::JoinParams {
        name: name.to_string(),
    }
}

/// Alice owns Acme; Bob owns Beta. Alice is not in Beta.
async fn alice_and_beta(ctx: &AppContext) -> (i64, i64, i64, organisations::Model) {
    let (alice, acme) = sign_up(ctx, "Acme", "alice").await;
    let (bob, beta) = sign_up(ctx, "Beta", "bob").await;
    (alice, acme, bob, org(ctx, beta).await)
}

async fn request(
    db: &sea_orm::DatabaseConnection,
    org: &organisations::Model,
    user_id: i64,
    name: &str,
) -> memberships::JoinOutcome {
    memberships::Model::request_to_join(db, org, user_id, &ask(name))
        .await
        .unwrap()
}

fn requested(outcome: memberships::JoinOutcome) -> memberships::Model {
    match outcome {
        memberships::JoinOutcome::Requested(m) => m,
        other => panic!("expected Requested, got {other:?}"),
    }
}

async fn rows_for(ctx: &AppContext, org_id: i64, user_id: i64) -> Vec<memberships::Model> {
    memberships::Entity::find()
        .filter(memberships::Column::OrganisationId.eq(org_id))
        .filter(memberships::Column::UserId.eq(user_id))
        .all(&ctx.db)
        .await
        .unwrap()
}

fn name_error(err: &loco_rs::model::ModelError) -> String {
    collab::views::forms::field_errors(err)
        .and_then(|e| e.get("name").cloned())
        .unwrap_or_else(|| format!("not a name error: {err:?}"))
}

#[tokio::test]
#[serial]
async fn request_to_join_adds_a_pending_member_row() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _, _, beta) = alice_and_beta(ctx).await;

    let row = requested(request(&ctx.db, &beta, alice, "alice2").await);
    assert_eq!(row.status, "pending");
    assert_eq!(row.role, "member");
    assert_eq!(row.username, "alice2");
    assert_eq!(row.organisation_id, beta.id);
    assert_eq!(rows_for(ctx, beta.id, alice).await.len(), 1);
}

#[tokio::test]
#[serial]
async fn request_to_join_marks_a_new_row_welcomed() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _, bob, beta) = alice_and_beta(ctx).await;

    let row = requested(request(&ctx.db, &beta, alice, "alice").await);
    assert!(row.welcomed_at.is_some(), "a new row is already welcomed");

    // A rejoined row keeps its own value, here none.
    let (carol, _) = sign_up(ctx, "Gamma", "carol").await;
    let carol_in_beta = add_member(ctx, beta.id, carol, "carol").await.unwrap();
    assert!(carol_in_beta.welcomed_at.is_none());
    memberships::Model::remove(ctx, beta.id, bob, carol_in_beta.id, &client())
        .await
        .unwrap();
    let again = requested(request(&ctx.db, &beta, carol, "carol").await);
    assert_eq!(again.id, carol_in_beta.id);
    assert!(again.welcomed_at.is_none());

    // A brand-new account still gets the welcome pop-up.
    let (_, fresh) = memberships::Model::join_new_account(
        &ctx.db,
        &beta,
        &users::RegisterParams {
            email: "dan@example.com".to_string(),
            password: "correct-horse".to_string(),
            name: "dan".to_string(),
        },
    )
    .await
    .unwrap();
    assert!(fresh.welcomed_at.is_none());
}

#[tokio::test]
#[serial]
async fn request_to_join_when_pending_changes_nothing() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _, _, beta) = alice_and_beta(ctx).await;
    let first = requested(request(&ctx.db, &beta, alice, "alice").await);

    match request(&ctx.db, &beta, alice, "alicia").await {
        memberships::JoinOutcome::AlreadyPending(m) => assert_eq!(m.id, first.id),
        other => panic!("{other:?}"),
    }
    let rows = rows_for(ctx, beta.id, alice).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].username, "alice", "the username stays");
    assert_eq!(rows[0].updated_at, first.updated_at);
}

#[tokio::test]
#[serial]
async fn request_to_join_when_active_says_already_a_member() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    let beta = org(ctx, orgs.beta).await;

    match request(&ctx.db, &beta, orgs.alice, "alicia").await {
        memberships::JoinOutcome::AlreadyMember(m) => assert_eq!(m.id, orgs.in_beta.id),
        other => panic!("{other:?}"),
    }
    let rows = rows_for(ctx, orgs.beta, orgs.alice).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].username, "alice");
    assert_eq!(rows[0].status, "active");
}

#[tokio::test]
#[serial]
async fn a_double_submit_returns_already_pending() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _, _, beta) = alice_and_beta(ctx).await;
    let other = second_connection(ctx).await;

    let (one, two) = tokio::join!(
        request(&ctx.db, &beta, alice, "alice"),
        request(&other, &beta, alice, "alice"),
    );
    let outcomes = [one, two];
    let made = outcomes
        .iter()
        .filter(|o| matches!(o, memberships::JoinOutcome::Requested(_)))
        .count();
    let waiting = outcomes
        .iter()
        .filter(|o| matches!(o, memberships::JoinOutcome::AlreadyPending(_)))
        .count();
    assert_eq!((made, waiting), (1, 1), "{outcomes:?}");
    assert_eq!(rows_for(ctx, beta.id, alice).await.len(), 1);
}

#[tokio::test]
#[serial]
async fn request_to_join_refuses_a_locked_account() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _, _, beta) = alice_and_beta(ctx).await;
    users::Model::lock_account(ctx, alice).await.unwrap();

    let err = memberships::Model::request_to_join(&ctx.db, &beta, alice, &ask("alice"))
        .await
        .unwrap_err();
    assert_eq!(
        name_error(&err),
        "Your account is locked. Reset your password to unlock it."
    );
    assert!(rows_for(ctx, beta.id, alice).await.is_empty());
}

#[tokio::test]
#[serial]
async fn rejoin_after_removal_updates_the_same_row() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    let mut admin = orgs.in_beta.clone().into_active_model();
    admin.role = ActiveValue::Set(memberships::role::ADMIN.to_string());
    admin.approved_by_id = ActiveValue::Set(Some(orgs.bob));
    admin.update(&ctx.db).await.unwrap();
    remove_alice_from_beta(ctx, &orgs).await;
    let removed = rows_for(ctx, orgs.beta, orgs.alice).await.remove(0);

    let beta = org(ctx, orgs.beta).await;
    let row = requested(request(&ctx.db, &beta, orgs.alice, "alicia").await);

    assert_eq!(rows_for(ctx, orgs.beta, orgs.alice).await.len(), 1);
    assert_eq!(row.id, removed.id);
    assert_eq!(row.status, "pending");
    assert_eq!(row.username, "alicia");
    assert_eq!(row.role, "admin", "the role is history until approval");
    assert_eq!(row.removed_at, removed.removed_at);
    assert_eq!(row.removed_by_id, removed.removed_by_id);
    assert_eq!(row.approved_by_id, removed.approved_by_id);
    assert_eq!(row.approved_at, removed.approved_at);
}

#[tokio::test]
#[serial]
async fn request_to_join_never_writes_active() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _, _, beta) = alice_and_beta(ctx).await;
    for start in [
        None,
        Some(memberships::status::PENDING),
        Some(memberships::status::REJECTED),
        Some(memberships::status::REMOVED),
    ] {
        memberships::Entity::delete_many()
            .filter(memberships::Column::OrganisationId.eq(beta.id))
            .filter(memberships::Column::UserId.eq(alice))
            .exec(&ctx.db)
            .await
            .unwrap();
        if let Some(status) = start {
            add_with_status(ctx, beta.id, alice, status).await;
        }
        request(&ctx.db, &beta, alice, "alice").await;
        let rows = rows_for(ctx, beta.id, alice).await;
        assert_eq!(rows.len(), 1, "{start:?}");
        assert_eq!(rows[0].status, "pending", "{start:?}");
    }
}

#[tokio::test]
#[serial]
async fn request_to_join_checks_the_username_in_that_org_only() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _, _, beta) = alice_and_beta(ctx).await;

    // "bob" is taken in Beta, in any case.
    let err = memberships::Model::request_to_join(&ctx.db, &beta, alice, &ask("BOB"))
        .await
        .unwrap_err();
    assert_eq!(name_error(&err), "That username is already used in Beta.");
    assert!(rows_for(ctx, beta.id, alice).await.is_empty());

    // "alice" is hers in Acme, which doesn't count in Beta.
    requested(request(&ctx.db, &beta, alice, "alice").await);
}

#[tokio::test]
#[serial]
async fn request_to_join_refuses_a_bad_or_reserved_username() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _, _, beta) = alice_and_beta(ctx).await;
    for (name, message) in [
        (
            "1abc",
            "Start with a letter and use 3 to 30 letters or numbers.",
        ),
        (
            "ab",
            "Start with a letter and use 3 to 30 letters or numbers.",
        ),
        ("Everyone", "That name is reserved."),
    ] {
        let err = memberships::Model::request_to_join(&ctx.db, &beta, alice, &ask(name))
            .await
            .unwrap_err();
        assert_eq!(name_error(&err), message, "{name}");
    }
    assert!(rows_for(ctx, beta.id, alice).await.is_empty());
}

/// `n` organisations owned by others, named "Org 1" and up.
async fn other_orgs(ctx: &AppContext, n: usize) -> Vec<organisations::Model> {
    let mut orgs = Vec::new();
    for i in 1..=n {
        let (_, id) = sign_up(ctx, &format!("Org {i}"), &format!("owner{i}")).await;
        orgs.push(org(ctx, id).await);
    }
    orgs
}

const CAP_MESSAGE: &str = "You're waiting to hear from 5 organisations already. Ask again once one of them lets you in or declines.";

#[tokio::test]
#[serial]
async fn request_to_join_stops_at_five_pending_requests() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _) = sign_up(ctx, "Acme", "alice").await;
    let orgs = other_orgs(ctx, 6).await;
    for org in &orgs[..5] {
        requested(request(&ctx.db, org, alice, "alice").await);
    }
    let err = memberships::Model::request_to_join(&ctx.db, &orgs[5], alice, &ask("alice"))
        .await
        .unwrap_err();
    assert_eq!(name_error(&err), CAP_MESSAGE);
    assert!(rows_for(ctx, orgs[5].id, alice).await.is_empty());
}

#[tokio::test]
#[serial]
async fn two_requests_at_once_do_not_pass_the_pending_cap() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _) = sign_up(ctx, "Acme", "alice").await;
    let orgs = other_orgs(ctx, 6).await;
    for org in &orgs[..4] {
        requested(request(&ctx.db, org, alice, "alice").await);
    }
    let other = second_connection(ctx).await;

    let params = ask("alice");
    let (one, two) = tokio::join!(
        memberships::Model::request_to_join(&ctx.db, &orgs[4], alice, &params),
        memberships::Model::request_to_join(&other, &orgs[5], alice, &params),
    );
    assert_eq!(
        [&one, &two].iter().filter(|r| r.is_ok()).count(),
        1,
        "{one:?} {two:?}"
    );
    let waiting = memberships::Entity::find()
        .filter(memberships::Column::UserId.eq(alice))
        .filter(memberships::Column::Status.eq(memberships::status::PENDING))
        .count(&ctx.db)
        .await
        .unwrap();
    assert_eq!(waiting, memberships::MAX_PENDING_REQUESTS);
}

#[tokio::test]
#[serial]
async fn the_org_cap_does_not_apply_to_joining() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _, _, beta) = alice_and_beta(ctx).await;
    let owner = users::Model::find_by_id(&ctx.db, alice).await.unwrap();
    for n in 2..=organisations::MAX_ORGS_CREATED {
        let txn = ctx.db.begin().await.unwrap();
        organisations::Model::create_with_owner(&txn, &format!("Mine {n}"), &owner, "alice")
            .await
            .unwrap();
        txn.commit().await.unwrap();
    }
    assert_eq!(
        organisations::Model::created_count(&ctx.db, alice)
            .await
            .unwrap(),
        organisations::MAX_ORGS_CREATED
    );
    requested(request(&ctx.db, &beta, alice, "alice").await);
}

#[tokio::test]
#[serial]
async fn default_for_user_prefers_pending_over_removed() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (dan, _) = sign_up(ctx, "Dan Co", "dan").await;
    let (_, gamma) = sign_up(ctx, "Gamma", "gail").await;
    let (_, delta) = sign_up(ctx, "Delta", "dina").await;
    let (_, epsilon) = sign_up(ctx, "Epsilon", "eve").await;
    // Dan's only active org goes; older removed and declined rows, then a
    // newer pending one.
    let own = memberships::Model::default_for_user(&ctx.db, dan)
        .await
        .unwrap()
        .unwrap();
    add_with_status(ctx, gamma, dan, memberships::status::REMOVED).await;
    add_with_status(ctx, delta, dan, memberships::status::REJECTED).await;
    add_with_status(ctx, epsilon, dan, memberships::status::PENDING).await;
    let mut own = own.into_active_model();
    own.status = ActiveValue::Set(memberships::status::REMOVED.to_string());
    own.update(&ctx.db).await.unwrap();

    let default = memberships::Model::default_for_user(&ctx.db, dan)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(default.organisation_id, epsilon);
    let by_user = memberships::Model::default_by_user(&ctx.db).await.unwrap();
    assert_eq!(by_user[&dan].organisation_id, epsilon);
}

#[test]
fn history_lines() {
    use chrono::TimeZone;
    let tz = chrono_tz::Pacific::Auckland;
    let at = |s: &str| chrono::DateTime::parse_from_rfc3339(s).unwrap();
    let year = chrono::Utc::now().format("%Y").to_string();
    // 11pm UTC on 2 October is 3 October in Auckland.
    let removed = at(&format!("{year}-10-02T23:00:00Z"));
    let declined = at(&format!("{year}-10-14T01:00:00Z"));
    let approved = at(&format!("{year}-09-01T09:00:00Z"));
    let base = memberships::Model {
        created_at: approved,
        updated_at: approved,
        id: 1,
        username: "lena".to_string(),
        role: "member".to_string(),
        status: "pending".to_string(),
        approved_at: None,
        organisation_id: 1,
        user_id: 2,
        approved_by_id: None,
        guide_dismissed_at: None,
        welcomed_at: None,
        removed_at: None,
        removed_by_id: None,
    };
    let line = |m: &memberships::Model| m.history().map(|h| h.line(tz));

    // A first request: none.
    assert_eq!(line(&base), None);
    // Removed, then asked again.
    let was_removed = memberships::Model {
        approved_at: Some(approved),
        approved_by_id: Some(3),
        removed_at: Some(removed),
        removed_by_id: Some(3),
        ..base.clone()
    };
    assert_eq!(line(&was_removed).as_deref(), Some("Removed on 3 October"));
    // Declined.
    let was_declined = memberships::Model {
        approved_at: Some(declined),
        approved_by_id: Some(3),
        status: "rejected".to_string(),
        ..base.clone()
    };
    assert_eq!(
        line(&was_declined).as_deref(),
        Some("Declined on 14 October")
    );
    // Declined after a removal: the later date.
    let both = memberships::Model {
        approved_at: Some(declined),
        ..was_removed.clone()
    };
    assert_eq!(line(&both).as_deref(), Some("Declined on 14 October"));
    // Declined before declines had dates.
    let old = memberships::Model {
        approved_by_id: Some(3),
        ..base.clone()
    };
    assert_eq!(line(&old).as_deref(), Some("Declined before"));
    // Active rows have none.
    let active = memberships::Model {
        status: "active".to_string(),
        ..was_removed
    };
    assert_eq!(active.history(), None);
    // Another year shows the year.
    let last_year = chrono::Utc.with_ymd_and_hms(2020, 5, 1, 12, 0, 0).unwrap();
    let older = memberships::Model {
        approved_at: Some(last_year.fixed_offset()),
        ..was_declined
    };
    assert_eq!(line(&older).as_deref(), Some("Declined on 2 May 2020"));
}

async fn conversation(ctx: &AppContext, org_id: i64, kind: &str, key: &str) {
    collab::models::conversations::ActiveModel {
        kind: ActiveValue::Set(kind.to_string()),
        dm_key: ActiveValue::Set(Some(key.to_string())),
        name: ActiveValue::Set((kind == "group").then(|| "Group".to_string())),
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
async fn dms_of_never_over_matches() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (_, acme) = sign_up(ctx, "Acme", "alice").await;
    let (_, beta) = sign_up(ctx, "Beta", "bob").await;
    for key in ["dm:1:11", "dm:11:21", "dm:5:21", "dm:1:21"] {
        conversation(ctx, acme, "dm", key).await;
    }
    conversation(ctx, beta, "dm", "dm:1:5").await;
    conversation(ctx, acme, "group", "dm:1:5").await;

    let keys = |user_id: i64| async move {
        let mut keys: Vec<String> =
            collab::models::conversations::Model::dms_of(&ctx.db, acme, user_id)
                .await
                .unwrap()
                .into_iter()
                .filter_map(|c| c.dm_key)
                .collect();
        keys.sort();
        keys
    };
    assert_eq!(keys(1).await, ["dm:1:11", "dm:1:21"]);
    assert_eq!(keys(11).await, ["dm:11:21", "dm:1:11"]);
    assert_eq!(keys(21).await, ["dm:11:21", "dm:1:21", "dm:5:21"]);
    assert!(keys(2).await.is_empty());
}

// Approving and declining.

fn by(user_id: i64) -> memberships::Approver {
    memberships::Approver {
        user_id,
        acting: false,
    }
}

async fn approve(
    db: &sea_orm::DatabaseConnection,
    org_id: i64,
    approver: memberships::Approver,
    id: i64,
) -> Result<memberships::Model, String> {
    memberships::Model::approve(db, org_id, approver, id)
        .await
        .map_err(|e| e.to_string())
}

async fn reload(ctx: &AppContext, id: i64) -> memberships::Model {
    memberships::Entity::find_by_id(id)
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap()
}

/// Alice's Beta row set straight to `status`, as if removal or approval had
/// happened without its side effects, so approval's own work can be seen.
async fn set_beta_status(ctx: &AppContext, orgs: &TwoOrgs, status: &str) -> memberships::Model {
    let mut row = reload(ctx, orgs.in_beta.id).await.into_active_model();
    row.status = ActiveValue::Set(status.to_string());
    row.update(&ctx.db).await.unwrap()
}

#[tokio::test]
#[serial]
async fn approve_changes_only_a_pending_row() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    for status in ["active", "removed", "rejected"] {
        let before = set_beta_status(ctx, &orgs, status).await;
        let err = approve(&ctx.db, orgs.beta, by(orgs.bob), orgs.in_beta.id)
            .await
            .unwrap_err();
        assert_eq!(err, "Only pending requests can be approved.", "{status}");
        assert_eq!(reload(ctx, orgs.in_beta.id).await, before, "{status}");
    }
    // Not in this organisation: not found.
    let err = memberships::Model::approve(&ctx.db, orgs.acme, by(orgs.alice), orgs.in_beta.id)
        .await
        .unwrap_err();
    assert!(matches!(err, loco_rs::model::ModelError::EntityNotFound));
}

#[tokio::test]
#[serial]
async fn two_approves_at_once_change_one_row() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    set_beta_status(ctx, &orgs, "pending").await;
    let other = second_connection(ctx).await;

    let (one, two) = tokio::join!(
        approve(&ctx.db, orgs.beta, by(orgs.bob), orgs.in_beta.id),
        approve(&other, orgs.beta, by(orgs.bob), orgs.in_beta.id),
    );
    let refused: Vec<_> = [&one, &two]
        .into_iter()
        .filter_map(|r| r.clone().err())
        .collect();
    assert_eq!(
        refused,
        ["Only pending requests can be approved."],
        "{one:?} {two:?}"
    );
    assert_eq!(reload(ctx, orgs.in_beta.id).await.status, "active");
}

#[tokio::test]
#[serial]
async fn approve_rechecks_the_approver() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    // Carol is a Beta admin, then demoted in another tab.
    let (carol, _) = sign_up(ctx, "Gamma", "carol").await;
    let carol_in_beta = add_member(ctx, orgs.beta, carol, "carol").await.unwrap();
    set_beta_status(ctx, &orgs, "pending").await;

    let err = approve(&ctx.db, orgs.beta, by(carol), orgs.in_beta.id)
        .await
        .unwrap_err();
    assert_eq!(err, "Only owners and admins can approve.");
    let err = memberships::Model::reject(&ctx.db, orgs.beta, by(carol), orgs.in_beta.id)
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "Only owners and admins can decline.");
    assert_eq!(reload(ctx, orgs.in_beta.id).await.status, "pending");

    let mut admin = carol_in_beta.into_active_model();
    admin.role = ActiveValue::Set(memberships::role::ADMIN.to_string());
    admin.update(&ctx.db).await.unwrap();
    approve(&ctx.db, orgs.beta, by(carol), orgs.in_beta.id)
        .await
        .unwrap();

    // An acting super admin has no membership here and isn't checked.
    let (dan, _) = sign_up(ctx, "Delta", "dan").await;
    let (erin, _) = sign_up(ctx, "Epsilon", "erin").await;
    let erin_in_beta = add_member(ctx, orgs.beta, erin, "erin").await.unwrap();
    let mut pending = erin_in_beta.into_active_model();
    pending.status = ActiveValue::Set("pending".to_string());
    let pending = pending.update(&ctx.db).await.unwrap();
    let acting = memberships::Approver {
        user_id: dan,
        acting: true,
    };
    let approved = approve(&ctx.db, orgs.beta, acting, pending.id)
        .await
        .unwrap();
    assert_eq!(approved.approved_by_id, Some(dan));
}

#[tokio::test]
#[serial]
async fn approve_waits_for_the_approvers_removal() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    let (carol, _) = sign_up(ctx, "Gamma", "carol").await;
    let carol_in_beta = add_member(ctx, orgs.beta, carol, "carol").await.unwrap();
    let mut admin = carol_in_beta.clone().into_active_model();
    admin.role = ActiveValue::Set(memberships::role::ADMIN.to_string());
    admin.update(&ctx.db).await.unwrap();
    set_beta_status(ctx, &orgs, "pending").await;
    let other = second_connection(ctx).await;

    // Carol is being removed on another connection while she approves.
    let ((), approved) = tokio::join!(
        held_removal(&other, carol_in_beta.id),
        after_the_lock(approve(&ctx.db, orgs.beta, by(carol), orgs.in_beta.id))
    );
    assert_eq!(approved.unwrap_err(), "Only owners and admins can approve.");
    assert_eq!(reload(ctx, orgs.in_beta.id).await.status, "pending");
}

#[tokio::test]
#[serial]
async fn nobody_approves_their_own_request() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    set_beta_status(ctx, &orgs, "pending").await;
    // Only an acting super admin could get this far with their own request.
    let acting = memberships::Approver {
        user_id: orgs.alice,
        acting: true,
    };
    let err = approve(&ctx.db, orgs.beta, acting, orgs.in_beta.id)
        .await
        .unwrap_err();
    assert_eq!(err, "You can't approve your own request.");
    assert_eq!(reload(ctx, orgs.in_beta.id).await.status, "pending");
}

#[tokio::test]
#[serial]
async fn approve_sets_member_and_clears_removal() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    let mut admin = orgs.in_beta.clone().into_active_model();
    admin.role = ActiveValue::Set(memberships::role::ADMIN.to_string());
    admin.update(&ctx.db).await.unwrap();
    remove_alice_from_beta(ctx, &orgs).await;
    let beta = org(ctx, orgs.beta).await;
    requested(request(&ctx.db, &beta, orgs.alice, "alice").await);

    let approved = approve(&ctx.db, orgs.beta, by(orgs.bob), orgs.in_beta.id)
        .await
        .unwrap();
    assert_eq!(approved.status, "active");
    assert_eq!(
        approved.role, "member",
        "a former admin comes back a member"
    );
    assert_eq!(approved.removed_at, None);
    assert_eq!(approved.removed_by_id, None);
    assert_eq!(approved.approved_by_id, Some(orgs.bob));
    assert!(approved.approved_at.is_some());
    assert_eq!(approved.history(), None);
}

#[tokio::test]
#[serial]
async fn approve_revokes_tokens_and_codes_in_that_org_only() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    let claude = claude_client(ctx).await;
    let acme_token = personal_token(ctx, orgs.acme, orgs.alice).await;
    let beta_token = personal_token(ctx, orgs.beta, orgs.alice).await;
    let acme_code = code_for(ctx, &claude, orgs.acme, orgs.alice).await;
    let beta_code = code_for(ctx, &claude, orgs.beta, orgs.alice).await;
    set_beta_status(ctx, &orgs, "pending").await;

    approve(&ctx.db, orgs.beta, by(orgs.bob), orgs.in_beta.id)
        .await
        .unwrap();

    assert!(!usable(ctx, &beta_token).await);
    assert!(exchange(&ctx.db, &claude, &beta_code).await.is_none());
    assert!(usable(ctx, &acme_token).await);
    assert!(exchange(&ctx.db, &claude, &acme_code).await.is_some());
}

#[tokio::test]
#[serial]
async fn approve_ends_an_unlock_for_that_org_only() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    assert!(
        users::Model::unlock_secrets(&ctx.db, orgs.alice, "sid-1", orgs.beta)
            .await
            .unwrap()
    );
    set_beta_status(ctx, &orgs, "pending").await;
    approve(&ctx.db, orgs.beta, by(orgs.bob), orgs.in_beta.id)
        .await
        .unwrap();
    let alice = users::Model::find_by_id(&ctx.db, orgs.alice).await.unwrap();
    assert!(!alice.secrets_unlocked(Some("sid-1"), orgs.beta));

    // An unlock in Acme is left alone.
    assert!(
        users::Model::unlock_secrets(&ctx.db, orgs.alice, "sid-1", orgs.acme)
            .await
            .unwrap()
    );
    set_beta_status(ctx, &orgs, "pending").await;
    approve(&ctx.db, orgs.beta, by(orgs.bob), orgs.in_beta.id)
        .await
        .unwrap();
    let alice = users::Model::find_by_id(&ctx.db, orgs.alice).await.unwrap();
    assert!(alice.secrets_unlocked(Some("sid-1"), orgs.acme));
}

/// A login in `org_id` that names `user_id`. Stored straight, with a dummy
/// secret: only the naming matters here.
async fn login_naming(ctx: &AppContext, org_id: i64, user_id: i64) -> i64 {
    use collab::models::{_entities::login_accesses, folders, logins};
    let folder = folders::Entity::find()
        .filter(folders::Column::OrganisationId.eq(org_id))
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    let login = logins::ActiveModel {
        title: ActiveValue::Set("Bank".to_string()),
        username: ActiveValue::Set("shop".to_string()),
        url: ActiveValue::Set("https://bank.example".to_string()),
        notes: ActiveValue::Set(String::new()),
        password_ct: ActiveValue::Set(vec![1]),
        password_nonce: ActiveValue::Set(vec![1]),
        dek_version: ActiveValue::Set(1),
        organisation_id: ActiveValue::Set(org_id),
        folder_id: ActiveValue::Set(folder.id),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .unwrap();
    login_accesses::ActiveModel {
        login_id: ActiveValue::Set(login.id),
        user_id: ActiveValue::Set(user_id),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .unwrap();
    login.id
}

#[tokio::test]
#[serial]
async fn approve_clears_named_login_access_in_that_org_only() {
    use collab::models::login_accesses;
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    let in_beta = login_naming(ctx, orgs.beta, orgs.alice).await;
    let in_acme = login_naming(ctx, orgs.acme, orgs.alice).await;
    let bob_in_beta = login_naming(ctx, orgs.beta, orgs.bob).await;
    set_beta_status(ctx, &orgs, "pending").await;

    approve(&ctx.db, orgs.beta, by(orgs.bob), orgs.in_beta.id)
        .await
        .unwrap();

    let named = |login, user| async move {
        login_accesses::Model::is_named(&ctx.db, login, user)
            .await
            .unwrap()
    };
    assert!(!named(in_beta, orgs.alice).await);
    assert!(named(in_acme, orgs.alice).await);
    assert!(named(bob_in_beta, orgs.bob).await);
}

async fn in_conversation(ctx: &AppContext, conversation_id: i64, user_id: i64) -> u64 {
    collab::models::conversation_members::Entity::find()
        .filter(collab::models::conversation_members::Column::ConversationId.eq(conversation_id))
        .filter(collab::models::conversation_members::Column::UserId.eq(user_id))
        .count(&ctx.db)
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn approve_adds_general_once() {
    use collab::models::{conversation_members, conversations};
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    let general = conversations::Model::find_general(&ctx.db, orgs.beta)
        .await
        .unwrap();
    conversation_members::Model::add(&ctx.db, &general, orgs.alice)
        .await
        .unwrap();
    set_beta_status(ctx, &orgs, "pending").await;

    approve(&ctx.db, orgs.beta, by(orgs.bob), orgs.in_beta.id)
        .await
        .unwrap();
    assert_eq!(in_conversation(ctx, general.id, orgs.alice).await, 1);

    // And a first join gets it too.
    let (carol, _) = sign_up(ctx, "Gamma", "carol").await;
    let gamma_row = requested(request(&ctx.db, &org(ctx, orgs.beta).await, carol, "carol").await);
    approve(&ctx.db, orgs.beta, by(orgs.bob), gamma_row.id)
        .await
        .unwrap();
    assert_eq!(in_conversation(ctx, general.id, carol).await, 1);
}

#[tokio::test]
#[serial]
async fn approve_restores_direct_messages_in_that_org() {
    use collab::models::conversations;
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    add_member(ctx, orgs.acme, orgs.bob, "bob").await.unwrap();
    let beta_dm = conversations::Model::start_dm(&ctx.db, orgs.beta, orgs.bob, orgs.alice)
        .await
        .unwrap();
    let acme_dm = conversations::Model::start_dm(&ctx.db, orgs.acme, orgs.alice, orgs.bob)
        .await
        .unwrap();
    let group = conversations::Model::create_group(
        &ctx.db,
        orgs.beta,
        orgs.bob,
        &conversations::GroupParams {
            name: "Bakers".to_string(),
            member_ids: vec![orgs.alice],
        },
    )
    .await
    .unwrap();
    remove_alice_from_beta(ctx, &orgs).await;
    assert_eq!(in_conversation(ctx, beta_dm.id, orgs.alice).await, 0);
    requested(request(&ctx.db, &org(ctx, orgs.beta).await, orgs.alice, "alice").await);

    approve(&ctx.db, orgs.beta, by(orgs.bob), orgs.in_beta.id)
        .await
        .unwrap();

    assert_eq!(in_conversation(ctx, beta_dm.id, orgs.alice).await, 1);
    assert_eq!(in_conversation(ctx, beta_dm.id, orgs.bob).await, 1);
    assert_eq!(in_conversation(ctx, acme_dm.id, orgs.alice).await, 1);
    assert_eq!(
        in_conversation(ctx, group.id, orgs.alice).await,
        0,
        "groups are not restored"
    );
}

#[tokio::test]
#[serial]
async fn reject_changes_only_a_pending_row() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    for status in ["active", "removed", "rejected"] {
        let before = set_beta_status(ctx, &orgs, status).await;
        let err = memberships::Model::reject(&ctx.db, orgs.beta, by(orgs.bob), orgs.in_beta.id)
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Only pending requests can be declined.",
            "{status}"
        );
        assert_eq!(reload(ctx, orgs.in_beta.id).await, before, "{status}");
    }
}

#[tokio::test]
#[serial]
async fn reject_sets_the_decision_date() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _, bob, beta) = alice_and_beta(ctx).await;
    let row = requested(request(&ctx.db, &beta, alice, "alice").await);
    assert_eq!(row.approved_at, None);

    let declined = memberships::Model::reject(&ctx.db, beta.id, by(bob), row.id)
        .await
        .unwrap();
    assert_eq!(declined.status, "rejected");
    assert_eq!(declined.approved_by_id, Some(bob));
    assert!(declined.approved_at.is_some());
    assert!(matches!(
        declined.history(),
        Some(memberships::History::Declined(_))
    ));
}

#[tokio::test]
#[serial]
async fn rejoin_after_decline_updates_the_same_row() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _, bob, beta) = alice_and_beta(ctx).await;
    let first = requested(request(&ctx.db, &beta, alice, "alice").await);
    let declined = memberships::Model::reject(&ctx.db, beta.id, by(bob), first.id)
        .await
        .unwrap();

    let again = requested(request(&ctx.db, &beta, alice, "alicia").await);
    assert_eq!(rows_for(ctx, beta.id, alice).await.len(), 1);
    assert_eq!(again.id, first.id);
    assert_eq!(again.status, "pending");
    assert_eq!(again.username, "alicia");
    assert_eq!(again.approved_at, declined.approved_at);
    assert_eq!(again.approved_by_id, Some(bob));
    assert!(matches!(
        again.history(),
        Some(memberships::History::Declined(_))
    ));
}
