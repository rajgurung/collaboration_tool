use collab::{
    app::App,
    models::{
        memberships,
        organisations::{self, SignupParams},
    },
};
use loco_rs::{app::AppContext, prelude::TenantActiveModelExt, testing::prelude::*};
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait};
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
