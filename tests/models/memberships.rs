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
