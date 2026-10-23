use collab::{app::App, models::_entities::task_notes};
use loco_rs::testing::prelude::*;
use sea_orm::EntityTrait;
use serial_test::serial;

macro_rules! configure_insta {
    ($($expr:expr),*) => {
        let mut settings = insta::Settings::clone_current();
        settings.set_prepend_module_to_snapshot(false);
        let _guard = settings.bind_to_scope();
    };
}

/// The `TaskNotes` entity matches the table its migration created.
///
/// Selecting every column is the cheapest assertion that says something true:
/// it fails if the migration and the entity disagree — a renamed column, a
/// type that does not round-trip, a migration that never ran — which is the
/// most common way a generated model breaks. Extend it as the model grows.
#[tokio::test]
#[serial]
async fn can_query_task_notes() {
    configure_insta!();

    let boot = boot_test::<App>().await.unwrap();
    seed::<App>(&boot.app_context).await.unwrap();

    // The query is the assertion. Bind the result and compare it once you have
    // seed data to compare against, e.g.:
    //
    // let items = task_notes::Entity::find().all(&boot.app_context.db).await.unwrap();
    // assert_debug_snapshot!(items);
    task_notes::Entity::find()
        .all(&boot.app_context.db)
        .await
        .expect("`task_notes` should be queryable — entity and migration must agree");
}

/// A note is found only through its own task.
#[tokio::test]
#[serial]
async fn find_on_task_refuses_another_tasks_note() {
    use collab::models::{task_notes as notes, tasks};

    use super::note_reactions::{note, org_with_task};

    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, task) = org_with_task(ctx, "Acme", "alice").await;
    let (_, other_task) = org_with_task(ctx, "Acme Two", "ann").await;
    let (gina, globex_task) = org_with_task(ctx, "Globex", "gina").await;
    let mine = note(ctx, &task, alice.id, "Mine").await;
    let globex = note(ctx, &globex_task, gina.id, "Globex").await;
    let sibling = tasks::Model::create(
        &ctx.db,
        task.organisation_id,
        &tasks::TaskParams {
            title: "Another Acme task".to_string(),
            project_id: String::new(),
            assignee_ids: vec![],
            priority: "low".to_string(),
            due_on: String::new(),
            status: String::new(),
        },
        None,
    )
    .await
    .unwrap();

    assert_eq!(
        notes::Model::find_on_task(&ctx.db, &task, mine.id)
            .await
            .unwrap()
            .id,
        mine.id
    );
    assert!(notes::Model::find_on_task(&ctx.db, &sibling, mine.id)
        .await
        .is_err());
    assert!(notes::Model::find_on_task(&ctx.db, &other_task, mine.id)
        .await
        .is_err());
    assert!(notes::Model::find_on_task(&ctx.db, &task, globex.id)
        .await
        .is_err());
}
