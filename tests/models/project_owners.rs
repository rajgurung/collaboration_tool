use collab::{
    app::App,
    models::{
        _entities::projects,
        memberships,
        organisations::{self, SignupParams},
        project_owners,
    },
};
use loco_rs::{app::AppContext, prelude::TenantActiveModelExt, testing::prelude::*};
use migration::{
    m20261026_120000_project_owners::{COPY_OWNERS, FIRST_OWNER_BACK},
    MigrationTrait, MigratorTrait, SchemaManager,
};
use sea_orm::{ActiveModelTrait, ActiveValue, ConnectionTrait, EntityTrait, Statement};
use serial_test::serial;

macro_rules! configure_insta {
    ($($expr:expr),*) => {
        let mut settings = insta::Settings::clone_current();
        settings.set_prepend_module_to_snapshot(false);
        let _guard = settings.bind_to_scope();
    };
}

/// The `ProjectOwners` entity matches the table its migration created.
#[tokio::test]
#[serial]
async fn can_query_project_owners() {
    configure_insta!();

    let boot = boot_test::<App>().await.unwrap();
    seed::<App>(&boot.app_context).await.unwrap();

    project_owners::Entity::find()
        .all(&boot.app_context.db)
        .await
        .expect("`project_owners` should be queryable — entity and migration must agree");
}

/// An organisation and its owner: `(org_id, user_id)`.
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
    (org.id, user.id)
}

async fn add_member(ctx: &AppContext, org_id: i64, user_id: i64, username: &str) {
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
    .unwrap();
}

/// A project written straight to the table, with no owners.
async fn project(ctx: &AppContext, org_id: i64, name: &str) -> i64 {
    projects::ActiveModel {
        name: ActiveValue::Set(name.to_string()),
        lane: ActiveValue::Set("now".to_string()),
        status: ActiveValue::Set("Active".to_string()),
        accent: ActiveValue::Set("#6cb6ff".to_string()),
        summary: ActiveValue::Set(String::new()),
        sort_order: ActiveValue::Set(1),
        organisation_id: ActiveValue::Set(org_id),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .unwrap()
    .id
}

async fn insert_owner(
    ctx: &AppContext,
    org_id: i64,
    project_id: i64,
    user_id: i64,
) -> Result<project_owners::Model, sea_orm::DbErr> {
    project_owners::ActiveModel {
        project_id: ActiveValue::Set(project_id),
        user_id: ActiveValue::Set(user_id),
        ..Default::default()
    }
    .set_tenant(org_id)
    .unwrap()
    .insert(&ctx.db)
    .await
}

/// The migration that dropped `projects.owner_id`.
fn drop_owner_id() -> Box<dyn MigrationTrait> {
    migration::Migrator::migrations()
        .into_iter()
        .find(|m| m.name() == "m20261026_150000_drop_project_owner_id")
        .unwrap()
}

async fn sql(ctx: &AppContext, query: &str) {
    ctx.db.execute_unprepared(query).await.unwrap();
}

/// Reads `projects.owner_id`; only there while the drop is rolled back.
async fn owner_id(ctx: &AppContext, project_id: i64) -> Option<i64> {
    ctx.db
        .query_one_raw(Statement::from_string(
            ctx.db.get_database_backend(),
            format!("SELECT owner_id FROM projects WHERE id = {project_id}"),
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<Option<i64>>("", "owner_id")
        .unwrap()
}

async fn has_owner_id_column(ctx: &AppContext) -> bool {
    ctx.db
        .query_one_raw(Statement::from_string(
            ctx.db.get_database_backend(),
            "SELECT count(*) AS n FROM information_schema.columns
              WHERE table_name = 'projects' AND column_name = 'owner_id'",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "n")
        .unwrap()
        == 1
}

#[tokio::test]
#[serial]
async fn replace_keeps_the_first_owner_first() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (org, ana) = sign_up(ctx, "Juniper Tea Co", "ana").await;
    let (_, leo) = sign_up(ctx, "Leo Lettering", "leo").await;
    let (_, tom) = sign_up(ctx, "Tom Tins", "tom").await;
    add_member(ctx, org, leo, "leo").await;
    add_member(ctx, org, tom, "tom").await;
    let website = project(ctx, org, "Website").await;

    project_owners::Model::replace(&ctx.db, org, website, &[leo, ana])
        .await
        .unwrap();
    project_owners::Model::replace(&ctx.db, org, website, &[tom, ana, leo])
        .await
        .unwrap();
    assert_eq!(
        project_owners::Model::for_project(&ctx.db, org, website)
            .await
            .unwrap(),
        vec![leo, ana, tom],
        "people who stay keep their place; new ones go last"
    );
    project_owners::Model::replace(&ctx.db, org, website, &[ana])
        .await
        .unwrap();
    let all = project_owners::Model::by_project(&ctx.db, org)
        .await
        .unwrap();
    assert_eq!(all.get(&website), Some(&vec![ana]));

    project_owners::Model::remove_person(&ctx.db, org, ana)
        .await
        .unwrap();
    assert!(project_owners::Model::for_project(&ctx.db, org, website)
        .await
        .unwrap()
        .is_empty());
}

/// The composite keys keep a row's project and person in its organisation,
/// and the unique index keeps one row per person per project.
#[tokio::test]
#[serial]
async fn the_database_refuses_rows_across_organisations() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (juniper, ana) = sign_up(ctx, "Juniper Tea Co", "ana").await;
    let (other, bob) = sign_up(ctx, "Bramble Bakery", "bob").await;
    let website = project(ctx, juniper, "Website").await;
    let bakery_site = project(ctx, other, "Bakery site").await;

    assert!(
        insert_owner(ctx, juniper, bakery_site, ana).await.is_err(),
        "a project in another organisation"
    );
    assert!(
        insert_owner(ctx, juniper, website, bob).await.is_err(),
        "a person with no membership here"
    );
    insert_owner(ctx, juniper, website, ana).await.unwrap();
    assert!(
        insert_owner(ctx, juniper, website, ana).await.is_err(),
        "the same person twice"
    );
}

/// The project owners migration's own SQL, both ways. It needs the
/// `owner_id` column, so the drop is rolled back first.
#[tokio::test]
#[serial]
async fn the_migration_copies_owners_and_writes_the_first_back() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    drop_owner_id()
        .down(&SchemaManager::new(&ctx.db))
        .await
        .unwrap();
    let (org, ana) = sign_up(ctx, "Juniper Tea Co", "ana").await;
    let (_, bob) = sign_up(ctx, "Bramble Bakery", "bob").await;
    let (_, leo) = sign_up(ctx, "Leo Lettering", "leo").await;
    add_member(ctx, org, leo, "leo").await;
    let website = project(ctx, org, "Website").await;
    // Bob has no membership here; the app never allowed this, but the copy
    // must skip it rather than fail.
    let stray = project(ctx, org, "Stray").await;
    let later = project(ctx, org, "Ship to Europe").await;
    // Tom was removed: his project gets no owner.
    let (_, tom) = sign_up(ctx, "Tom Tins", "tom").await;
    add_member(ctx, org, tom, "tom").await;
    let toms = project(ctx, org, "Tins").await;
    sql(ctx, &format!(
        "UPDATE projects SET owner_id = {ana} WHERE id = {website};
         UPDATE projects SET owner_id = {bob} WHERE id = {stray};
         UPDATE projects SET owner_id = {tom} WHERE id = {toms};
         UPDATE memberships SET status = 'removed' WHERE user_id = {tom} AND organisation_id = {org};"
    ))
    .await;

    ctx.db.execute_unprepared(COPY_OWNERS).await.unwrap();
    // A second run (or a row already there) changes nothing.
    ctx.db.execute_unprepared(COPY_OWNERS).await.unwrap();
    let all = project_owners::Model::by_project(&ctx.db, org)
        .await
        .unwrap();
    assert_eq!(all.get(&website), Some(&vec![ana]));
    assert_eq!(all.get(&stray), None);
    assert_eq!(all.get(&later), None);
    assert_eq!(all.get(&toms), None, "a removed member");

    insert_owner(ctx, org, website, leo).await.unwrap();
    ctx.db.execute_unprepared(FIRST_OWNER_BACK).await.unwrap();
    assert_eq!(owner_id(ctx, website).await, Some(ana), "the first added");
    assert_eq!(owner_id(ctx, later).await, None);
    assert_eq!(owner_id(ctx, stray).await, None, "no owners means no owner");
}

/// Up drops `projects.owner_id`; down puts it back, with its foreign key,
/// holding each project's first-added owner.
#[tokio::test]
#[serial]
async fn dropping_owner_id_rolls_back_with_the_first_owner() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let schema = SchemaManager::new(&ctx.db);
    assert!(!has_owner_id_column(ctx).await, "migrated up: no column");

    let (org, ana) = sign_up(ctx, "Juniper Tea Co", "ana").await;
    let (_, leo) = sign_up(ctx, "Leo Lettering", "leo").await;
    add_member(ctx, org, leo, "leo").await;
    let website = project(ctx, org, "Website").await;
    let later = project(ctx, org, "Ship to Europe").await;
    project_owners::Model::replace(&ctx.db, org, website, &[leo, ana])
        .await
        .unwrap();

    drop_owner_id().down(&schema).await.unwrap();
    assert!(has_owner_id_column(ctx).await);
    assert_eq!(owner_id(ctx, website).await, Some(leo), "the first added");
    assert_eq!(owner_id(ctx, later).await, None);
    // The foreign key is back: deleting the user clears the column.
    sql(ctx, &format!("DELETE FROM users WHERE id = {leo}")).await;
    assert_eq!(owner_id(ctx, website).await, None);

    drop_owner_id().up(&schema).await.unwrap();
    assert!(!has_owner_id_column(ctx).await);
}
