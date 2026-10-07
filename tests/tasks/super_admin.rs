use collab::{
    app::App,
    models::{memberships, organisations, users},
};
use loco_rs::{boot::run_task, task, testing::prelude::*};
use sea_orm::{EntityTrait, PaginatorTrait};
use serial_test::serial;

#[tokio::test]
#[serial]
async fn super_admin_task_is_idempotent() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    for _ in 0..2 {
        run_task::<App>(
            ctx,
            Some(&"super_admin".to_string()),
            &task::Vars::default(),
        )
        .await
        .unwrap();
    }

    assert_eq!(users::Entity::find().count(&ctx.db).await.unwrap(), 1);
    assert_eq!(
        organisations::Entity::find().count(&ctx.db).await.unwrap(),
        1
    );

    let admin = users::Model::find_by_email(&ctx.db, "gurungraj26@gmail.com")
        .await
        .unwrap();
    assert!(admin.is_super_admin);
    assert_eq!(admin.name, "raj");

    let membership = memberships::Model::find_for_user(&ctx.db, admin.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(membership.role, "owner");
    let org = organisations::Model::find_by_id(&ctx.db, membership.organisation_id)
        .await
        .unwrap();
    assert_eq!(org.name, "Himalayan Ritual");
    assert_eq!(org.slug, "himalayan-ritual");
}

#[tokio::test]
#[serial]
async fn super_admin_task_promotes_an_existing_account_without_touching_its_password() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    users::Model::create_with_password(
        &ctx.db,
        &users::RegisterParams {
            email: "gurungraj26@gmail.com".to_string(),
            password: "my-own-password".to_string(),
            name: "rajg".to_string(),
        },
    )
    .await
    .unwrap();

    run_task::<App>(
        ctx,
        Some(&"super_admin".to_string()),
        &task::Vars::default(),
    )
    .await
    .unwrap();

    let admin = users::Model::find_by_email(&ctx.db, "gurungraj26@gmail.com")
        .await
        .unwrap();
    assert!(admin.is_super_admin);
    assert!(admin.verify_password("my-own-password"));
}
