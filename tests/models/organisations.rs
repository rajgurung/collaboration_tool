use collab::{
    app::App,
    models::{
        _entities::organisations,
        conversation_members, conversations, folders, memberships,
        organisations::{NewOrgParams, SignupParams, AT_CAP, MAX_ORGS_CREATED},
        users,
    },
    views::forms::field_errors,
};
use loco_rs::{
    app::AppContext, model::ModelError, prelude::TenantActiveModelExt, testing::prelude::*,
};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter,
};
use serial_test::serial;
use validator::Validate;

macro_rules! configure_insta {
    ($($expr:expr),*) => {
        let mut settings = insta::Settings::clone_current();
        settings.set_prepend_module_to_snapshot(false);
        let _guard = settings.bind_to_scope();
    };
}

/// The `Organisations` entity matches the table its migration created.
///
/// Selecting every column is the cheapest assertion that says something true:
/// it fails if the migration and the entity disagree — a renamed column, a
/// type that does not round-trip, a migration that never ran — which is the
/// most common way a generated model breaks. Extend it as the model grows.
#[tokio::test]
#[serial]
async fn can_query_organisations() {
    configure_insta!();

    let boot = boot_test::<App>().await.unwrap();
    seed::<App>(&boot.app_context).await.unwrap();

    // The query is the assertion. Bind the result and compare it once you have
    // seed data to compare against, e.g.:
    //
    // let items = organisations::Entity::find().all(&boot.app_context.db).await.unwrap();
    // assert_debug_snapshot!(items);
    organisations::Entity::find()
        .all(&boot.app_context.db)
        .await
        .expect("`organisations` should be queryable — entity and migration must agree");
}

fn new_org(name: &str, username: &str) -> NewOrgParams {
    NewOrgParams {
        organisation_name: name.to_string(),
        name: username.to_string(),
    }
}

fn signup(name: &str) -> SignupParams {
    SignupParams {
        organisation_name: name.to_string(),
        name: "alice".to_string(),
        email: "alice@example.com".to_string(),
        password: "correct-horse".to_string(),
    }
}

/// The message on `organisation_name`, if validation refused it.
fn name_error(result: Result<(), validator::ValidationErrors>) -> Option<String> {
    result.err().and_then(|e| {
        e.field_errors()
            .get("organisation_name")
            .and_then(|list| list.first())
            .and_then(|err| err.message.as_ref().map(ToString::to_string))
    })
}

#[test]
fn a_spaces_only_name_is_refused() {
    for name in ["   ", " a ", ""] {
        assert_eq!(
            name_error(new_org(name, "alice").validate()).as_deref(),
            Some("Use 2 to 80 characters."),
            "create: {name:?}"
        );
        assert_eq!(
            name_error(signup(name).validate()).as_deref(),
            Some("Use 2 to 80 characters."),
            "signup: {name:?}"
        );
    }
    // Spaces around a real name don't count against it.
    let long = format!("  {}  ", "a".repeat(80));
    assert_eq!(name_error(new_org(&long, "alice").validate()), None);
    assert_eq!(name_error(signup(" ab ").validate()), None);
    assert!(name_error(signup(&"a".repeat(81)).validate()).is_some());
}

type Org = collab::models::organisations::Model;

async fn sign_up(ctx: &AppContext, org: &str, owner: &str) -> (users::Model, Org) {
    Org::sign_up(
        &ctx.db,
        &SignupParams {
            organisation_name: org.to_string(),
            name: owner.to_string(),
            email: format!("{owner}@example.com"),
            password: "correct-horse".to_string(),
        },
    )
    .await
    .unwrap()
}

async fn create(ctx: &AppContext, user: &users::Model, name: &str) -> Result<Org, ModelError> {
    Org::create_for_user(&ctx.db, user, &new_org(name, "ali"), None).await
}

/// Moves every org this person created back two days, out of the daily window.
async fn backdate(ctx: &AppContext, user_id: i64) {
    organisations::Entity::update_many()
        .col_expr(
            organisations::Column::CreatedAt,
            sea_orm::sea_query::Expr::value(chrono::Utc::now() - chrono::Duration::days(2)),
        )
        .filter(organisations::Column::CreatedById.eq(user_id))
        .exec(&ctx.db)
        .await
        .unwrap();
}

/// How many organisations exist.
async fn org_count(ctx: &AppContext) -> u64 {
    organisations::Entity::find().count(&ctx.db).await.unwrap()
}

fn error_on(err: &ModelError, field: &str) -> Option<String> {
    field_errors(err).and_then(|e| e.get(field).cloned())
}

async fn membership(ctx: &AppContext, org_id: i64, user_id: i64) -> memberships::Model {
    memberships::Model::active_in(&ctx.db, org_id, user_id)
        .await
        .unwrap()
        .expect("an active membership")
}

#[tokio::test]
#[serial]
async fn create_for_user_makes_an_active_owner_with_the_chosen_username() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _) = sign_up(ctx, "Acme Studio", "alice").await;
    let org = create(ctx, &alice, "Lumen Café").await.unwrap();
    assert_eq!(org.name, "Lumen Café");
    let m = membership(ctx, org.id, alice.id).await;
    assert_eq!(m.username, "ali", "the chosen name, not the account's");
    assert_eq!(m.role, memberships::role::OWNER);
    assert!(m.is_active());
}

#[tokio::test]
#[serial]
async fn create_for_user_sets_up_general_and_the_starter_folders() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _) = sign_up(ctx, "Acme Studio", "alice").await;
    let org = create(ctx, &alice, "Lumen Café").await.unwrap();
    let general = conversations::Model::find_general(&ctx.db, org.id)
        .await
        .unwrap();
    let members = conversation_members::Entity::find()
        .filter(conversation_members::Column::ConversationId.eq(general.id))
        .all(&ctx.db)
        .await
        .unwrap();
    assert_eq!(
        members.iter().map(|m| m.user_id).collect::<Vec<_>>(),
        [alice.id]
    );
    let mut names: Vec<String> = folders::Model::list_for_org(&ctx.db, org.id)
        .await
        .unwrap()
        .into_iter()
        .map(|f| f.folder.name)
        .collect();
    names.sort();
    assert_eq!(names, ["Company", "Logins", "Shared"]);
}

#[tokio::test]
#[serial]
async fn create_for_user_sets_created_by() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _) = sign_up(ctx, "Acme Studio", "alice").await;
    let org = create(ctx, &alice, "Lumen Café").await.unwrap();
    assert_eq!(org.created_by_id, Some(alice.id));
    assert_eq!(Org::created_count(&ctx.db, alice.id).await.unwrap(), 2);
}

#[tokio::test]
#[serial]
async fn create_for_user_checks_fields_like_signup() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _) = sign_up(ctx, "Acme Studio", "alice").await;
    let before = org_count(ctx).await;
    for (name, username, field, message) in [
        ("L", "ali", "organisation_name", "Use 2 to 80 characters."),
        (
            &"a".repeat(81) as &str,
            "ali",
            "organisation_name",
            "Use 2 to 80 characters.",
        ),
        (
            "Lumen",
            "2ali",
            "name",
            "Start with a letter and use 3 to 30 letters or numbers.",
        ),
        ("Lumen", "everyone", "name", "That name is reserved."),
    ] {
        let err = Org::create_for_user(&ctx.db, &alice, &new_org(name, username), None)
            .await
            .unwrap_err();
        assert_eq!(
            error_on(&err, field).as_deref(),
            Some(message),
            "{name} {username}"
        );
    }
    assert_eq!(org_count(ctx).await, before, "nothing saved");
}

#[tokio::test]
#[serial]
async fn create_for_user_stops_at_the_cap() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _) = sign_up(ctx, "Acme Studio", "alice").await;
    for n in 2..=MAX_ORGS_CREATED {
        backdate(ctx, alice.id).await;
        create(ctx, &alice, &format!("Org {n}")).await.unwrap();
    }
    backdate(ctx, alice.id).await;
    let before = org_count(ctx).await;
    let err = create(ctx, &alice, "Org 11").await.unwrap_err();
    assert!(error_on(&err, AT_CAP).is_some(), "{err:?}");
    assert_eq!(org_count(ctx).await, before);
}

#[tokio::test]
#[serial]
async fn create_for_user_stops_at_the_daily_limit() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _) = sign_up(ctx, "Acme Studio", "alice").await;
    // The signup org counts as one of today's three.
    create(ctx, &alice, "Two").await.unwrap();
    create(ctx, &alice, "Three").await.unwrap();
    let err = create(ctx, &alice, "Four").await.unwrap_err();
    assert_eq!(
        error_on(&err, "organisation_name").as_deref(),
        Some("You've set up 3 organisations today. Try again tomorrow.")
    );
    // One of the three moves out of the last 24 hours.
    organisations::Entity::update_many()
        .col_expr(
            organisations::Column::CreatedAt,
            sea_orm::sea_query::Expr::value(chrono::Utc::now() - chrono::Duration::hours(25)),
        )
        .filter(organisations::Column::Name.eq("Acme Studio"))
        .exec(&ctx.db)
        .await
        .unwrap();
    create(ctx, &alice, "Four").await.unwrap();
}

#[tokio::test]
#[serial]
async fn create_for_user_refuses_a_name_you_already_have() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _) = sign_up(ctx, "Acme Studio", "alice").await;
    backdate(ctx, alice.id).await;
    for name in ["  acme   studio ", "ACME STUDIO"] {
        let err = create(ctx, &alice, name).await.unwrap_err();
        assert_eq!(
            error_on(&err, "organisation_name").as_deref(),
            Some("You already have an organisation called Acme Studio. Pick a name you can tell apart."),
            "{name}"
        );
    }
    create(ctx, &alice, "école").await.unwrap();
    let err = create(ctx, &alice, "ÉCOLE").await.unwrap_err();
    assert!(error_on(&err, "organisation_name")
        .unwrap()
        .contains("called école."));

    // Someone else's org, and one Alice is only waiting for, don't count.
    sign_up(ctx, "Lumen Café", "bob").await;
    let (_, pending) = sign_up(ctx, "Northwind Bakery", "nina").await;
    memberships::ActiveModel {
        user_id: ActiveValue::Set(alice.id),
        username: ActiveValue::Set("alice".to_string()),
        role: ActiveValue::Set(memberships::role::MEMBER.to_string()),
        status: ActiveValue::Set(memberships::status::PENDING.to_string()),
        ..Default::default()
    }
    .set_tenant(pending.id)
    .unwrap()
    .insert(&ctx.db)
    .await
    .unwrap();
    create(ctx, &alice, "Lumen Café").await.unwrap();
    backdate(ctx, alice.id).await;
    create(ctx, &alice, "Northwind Bakery").await.unwrap();
}

/// A second connection pool to the test database. The app's pool has one
/// connection, so queries through it never overlap; races need two.
async fn second_connection(ctx: &AppContext) -> sea_orm::DatabaseConnection {
    sea_orm::Database::connect(&ctx.config.database.uri)
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn two_creates_at_once_do_not_pass_the_cap() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _) = sign_up(ctx, "Acme Studio", "alice").await;
    for n in 2..MAX_ORGS_CREATED {
        backdate(ctx, alice.id).await;
        create(ctx, &alice, &format!("Org {n}")).await.unwrap();
    }
    backdate(ctx, alice.id).await;
    let other = second_connection(ctx).await;
    let (left, right) = (new_org("Left", "ali"), new_org("Right", "ali"));
    let (a, b) = tokio::join!(
        Org::create_for_user(&ctx.db, &alice, &left, None),
        Org::create_for_user(&other, &alice, &right, None)
    );
    let made = [a.is_ok(), b.is_ok()].iter().filter(|ok| **ok).count();
    assert_eq!(made, 1, "exactly one gets the tenth: {a:?} {b:?}");
    let refused = a.err().or(b.err()).unwrap();
    assert!(error_on(&refused, AT_CAP).is_some());
    assert_eq!(
        Org::created_count(&ctx.db, alice.id).await.unwrap(),
        MAX_ORGS_CREATED
    );
}

#[tokio::test]
#[serial]
async fn create_for_user_copies_the_given_time_zone() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _) = sign_up(ctx, "Acme Studio", "alice").await;
    let org = Org::create_for_user(
        &ctx.db,
        &alice,
        &new_org("Lumen Café", "ali"),
        Some("Asia/Kathmandu"),
    )
    .await
    .unwrap();
    assert_eq!(org.timezone, "Asia/Kathmandu");
}

#[tokio::test]
#[serial]
async fn create_for_user_without_a_time_zone_keeps_the_default() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _) = sign_up(ctx, "Acme Studio", "alice").await;
    let org = create(ctx, &alice, "Lumen Café").await.unwrap();
    assert_eq!(
        Org::find_by_id(&ctx.db, org.id).await.unwrap().timezone,
        "Europe/London"
    );
}

#[tokio::test]
#[serial]
async fn the_created_membership_is_already_welcomed() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, acme) = sign_up(ctx, "Acme Studio", "alice").await;
    let org = create(ctx, &alice, "Lumen Café").await.unwrap();
    let m = membership(ctx, org.id, alice.id).await;
    assert!(m.welcomed_at.is_some());
    assert!(m.guide_dismissed_at.is_none(), "the guide still shows");
    assert!(
        membership(ctx, acme.id, alice.id)
            .await
            .welcomed_at
            .is_none(),
        "signup keeps the pop-up"
    );
}

#[tokio::test]
#[serial]
async fn sign_up_still_uses_the_account_name_as_the_username_and_the_default_zone() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, acme) = sign_up(ctx, "Acme Studio", "alice").await;
    assert_eq!(membership(ctx, acme.id, alice.id).await.username, "alice");
    assert_eq!(acme.timezone, "Europe/London");
    assert_eq!(acme.created_by_id, Some(alice.id));
}

#[tokio::test]
#[serial]
async fn a_locked_account_cannot_create() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, _) = sign_up(ctx, "Acme Studio", "alice").await;
    users::Model::lock_account(ctx, alice.id).await.unwrap();
    let before = org_count(ctx).await;
    // The model passed in is from before the lock, as a request's would be.
    let err = create(ctx, &alice, "Lumen Café").await.unwrap_err();
    assert_eq!(
        error_on(&err, "organisation_name").as_deref(),
        Some("Your account is locked. Reset your password to unlock it.")
    );
    assert_eq!(org_count(ctx).await, before);
}
