use std::time::Duration;

use chrono::{DateTime, FixedOffset, Utc};
use collab::{
    app::App,
    models::{
        _entities::projects,
        organisations::{self, SignupParams},
        projects::{self as project_model, ProjectParams},
        tasks::{self, TaskParams},
    },
};
use loco_rs::{app::AppContext, prelude::TenantActiveModelExt, testing::prelude::*};
use sea_orm::{ActiveModelTrait, ActiveValue, Database, EntityTrait, TransactionTrait};
use serial_test::serial;

macro_rules! configure_insta {
    ($($expr:expr),*) => {
        let mut settings = insta::Settings::clone_current();
        settings.set_prepend_module_to_snapshot(false);
        let _guard = settings.bind_to_scope();
    };
}

/// The `Projects` entity matches the table its migration created.
///
/// Selecting every column is the cheapest assertion that says something true:
/// it fails if the migration and the entity disagree — a renamed column, a
/// type that does not round-trip, a migration that never ran — which is the
/// most common way a generated model breaks. Extend it as the model grows.
#[tokio::test]
#[serial]
async fn can_query_projects() {
    configure_insta!();

    let boot = boot_test::<App>().await.unwrap();
    seed::<App>(&boot.app_context).await.unwrap();

    // The query is the assertion. Bind the result and compare it once you have
    // seed data to compare against, e.g.:
    //
    // let items = projects::Entity::find().all(&boot.app_context.db).await.unwrap();
    // assert_debug_snapshot!(items);
    projects::Entity::find()
        .all(&boot.app_context.db)
        .await
        .expect("`projects` should be queryable — entity and migration must agree");
}

/// An organisation and its owner: `(org_id, user_id)`.
async fn org(ctx: &AppContext, name: &str, owner: &str) -> (i64, i64) {
    let (user, org) = organisations::Model::sign_up(
        &ctx.db,
        &SignupParams {
            organisation_name: name.to_string(),
            name: owner.to_string(),
            email: format!("{owner}@example.com"),
            password: "correct-horse".to_string(),
        },
    )
    .await
    .unwrap();
    (org.id, user.id)
}

fn params(name: &str) -> ProjectParams {
    ProjectParams {
        name: name.to_string(),
        lane: "now".to_string(),
        status: "Active".to_string(),
        accent: "#6cb6ff".to_string(),
        owner_id: String::new(),
        summary: String::new(),
    }
}

async fn project(ctx: &AppContext, org_id: i64, name: &str) -> project_model::Model {
    project_model::Model::create(&ctx.db, org_id, &params(name), None)
        .await
        .unwrap()
}

async fn task(
    ctx: &AppContext,
    org_id: i64,
    project_id: i64,
    title: &str,
    status: &str,
    done_at: Option<DateTime<FixedOffset>>,
) -> tasks::Model {
    tasks::ActiveModel {
        title: ActiveValue::Set(title.to_string()),
        status: ActiveValue::Set(status.to_string()),
        priority: ActiveValue::Set("medium".to_string()),
        sort_order: ActiveValue::Set(1),
        project_id: ActiveValue::Set(Some(project_id)),
        done_at: ActiveValue::Set(done_at),
        ..Default::default()
    }
    .set_tenant(org_id)
    .unwrap()
    .insert(&ctx.db)
    .await
    .unwrap()
}

async fn reload(ctx: &AppContext, t: &tasks::Model) -> tasks::Model {
    tasks::Entity::find_by_id(t.id)
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap()
}

fn last_week() -> DateTime<FixedOffset> {
    (Utc::now() - chrono::Duration::days(7)).into()
}

fn task_params(title: &str, project_id: Option<i64>, status: &str) -> TaskParams {
    TaskParams {
        title: title.to_string(),
        project_id: project_id.map(|id| id.to_string()).unwrap_or_default(),
        assignee_ids: Vec::new(),
        priority: "medium".to_string(),
        due_on: String::new(),
        status: status.to_string(),
    }
}

/// Website with one task in each open status, and one done last week.
struct Website {
    org_id: i64,
    user_id: i64,
    project: project_model::Model,
    todo: tasks::Model,
    progress: tasks::Model,
    blocked: tasks::Model,
    done: tasks::Model,
}

async fn website(ctx: &AppContext) -> Website {
    let (org_id, user_id) = org(ctx, "Juniper Tea Co", "ana").await;
    let project = project(ctx, org_id, "Website").await;
    let pid = project.id;
    Website {
        todo: task(ctx, org_id, pid, "Returns page", "todo", None).await,
        progress: task(ctx, org_id, pid, "Checkout", "progress", None).await,
        blocked: task(ctx, org_id, pid, "Payments", "blocked", None).await,
        done: task(ctx, org_id, pid, "Logo", "done", Some(last_week())).await,
        org_id,
        user_id,
        project,
    }
}

#[tokio::test]
#[serial]
async fn close_marks_every_open_task_done() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let w = website(ctx).await;

    let closed = project_model::Model::close(&ctx.db, w.org_id, w.project.id, w.user_id)
        .await
        .unwrap();
    assert!(!closed.already);
    assert_eq!(closed.tasks.len(), 3);
    assert!(closed.project.closed_at.is_some());
    assert_eq!(closed.project.closed_by_id, Some(w.user_id));
    assert_eq!(closed.project.lane, "now", "the lane is kept");

    for (before, status) in [
        (&w.todo, "todo"),
        (&w.progress, "progress"),
        (&w.blocked, "blocked"),
    ] {
        let after = reload(ctx, before).await;
        assert_eq!(after.status, "done");
        assert!(after.done_at.is_some());
        assert_eq!(after.status_before_close.as_deref(), Some(status));
    }
    let done = reload(ctx, &w.done).await;
    assert_eq!(done.done_at, w.done.done_at, "keeps when it was finished");
    assert_eq!(done.status_before_close, None);
}

#[tokio::test]
#[serial]
async fn close_twice_is_a_no_op() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let w = website(ctx).await;
    let first = project_model::Model::close(&ctx.db, w.org_id, w.project.id, w.user_id)
        .await
        .unwrap();

    let second = project_model::Model::close(&ctx.db, w.org_id, w.project.id, w.user_id)
        .await
        .unwrap();
    assert!(second.already);
    assert!(second.tasks.is_empty());
    assert_eq!(second.project.closed_at, first.project.closed_at);
    assert_eq!(
        reload(ctx, &w.blocked).await.status_before_close.as_deref(),
        Some("blocked"),
        "the saved status is not overwritten with done"
    );
}

#[tokio::test]
#[serial]
async fn close_of_another_orgs_project_changes_nothing() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let w = website(ctx).await;
    let (globex, gina) = org(ctx, "Globex", "gina").await;

    let err = project_model::Model::close(&ctx.db, globex, w.project.id, gina)
        .await
        .unwrap_err();
    assert!(matches!(err, loco_rs::model::ModelError::EntityNotFound));
    let err = project_model::Model::reopen(&ctx.db, globex, w.project.id)
        .await
        .unwrap_err();
    assert!(matches!(err, loco_rs::model::ModelError::EntityNotFound));

    let project = project_model::Model::find_in_org(&ctx.db, w.org_id, w.project.id)
        .await
        .unwrap();
    assert!(!project.is_closed());
    for t in [&w.todo, &w.progress, &w.blocked, &w.done] {
        assert_eq!(&reload(ctx, t).await, t);
    }
}

#[tokio::test]
#[serial]
async fn reopen_puts_each_task_back() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let w = website(ctx).await;
    project_model::Model::close(&ctx.db, w.org_id, w.project.id, w.user_id)
        .await
        .unwrap();

    let reopened = project_model::Model::reopen(&ctx.db, w.org_id, w.project.id)
        .await
        .unwrap();
    assert!(!reopened.already);
    assert_eq!(reopened.restored, 3);
    assert!(reopened.project.closed_at.is_none());
    assert!(reopened.project.closed_by_id.is_none());
    assert!(reopened.project.in_lane("now"));
    for t in [&w.todo, &w.progress, &w.blocked] {
        let after = reload(ctx, t).await;
        assert_eq!(after.status, t.status);
        assert_eq!(after.done_at, None);
        assert_eq!(after.status_before_close, None);
    }
    let done = reload(ctx, &w.done).await;
    assert_eq!(done.status, "done");
    assert_eq!(done.done_at, w.done.done_at);

    let again = project_model::Model::reopen(&ctx.db, w.org_id, w.project.id)
        .await
        .unwrap();
    assert!(again.already);
}

#[tokio::test]
#[serial]
async fn a_task_changed_since_is_left_alone() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let w = website(ctx).await;
    project_model::Model::close(&ctx.db, w.org_id, w.project.id, w.user_id)
        .await
        .unwrap();
    // Set back to in progress, as a board drag does.
    reload(ctx, &w.todo)
        .await
        .set_status(&ctx.db, "progress")
        .await
        .unwrap();
    // In progress, then done again.
    let blocked = reload(ctx, &w.blocked)
        .await
        .set_status(&ctx.db, "progress")
        .await
        .unwrap();
    assert_eq!(blocked.status_before_close, None);
    blocked.set_status(&ctx.db, "done").await.unwrap();

    let reopened = project_model::Model::reopen(&ctx.db, w.org_id, w.project.id)
        .await
        .unwrap();
    assert_eq!(reopened.restored, 1, "only the untouched task");
    assert_eq!(reload(ctx, &w.todo).await.status, "progress");
    assert_eq!(reload(ctx, &w.blocked).await.status, "done");
    assert_eq!(reload(ctx, &w.progress).await.status, "progress");
}

#[tokio::test]
#[serial]
async fn a_title_edit_does_not_count_as_a_change() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let w = website(ctx).await;
    project_model::Model::close(&ctx.db, w.org_id, w.project.id, w.user_id)
        .await
        .unwrap();

    // Saved from the form with the status still done, and from a form that
    // leaves the status out.
    reload(ctx, &w.blocked)
        .await
        .update_from(
            &ctx.db,
            &task_params("Payments (Stripe)", Some(w.project.id), "done"),
        )
        .await
        .unwrap();
    reload(ctx, &w.todo)
        .await
        .update_from(&ctx.db, &task_params("Returns", Some(w.project.id), ""))
        .await
        .unwrap();

    project_model::Model::reopen(&ctx.db, w.org_id, w.project.id)
        .await
        .unwrap();
    let blocked = reload(ctx, &w.blocked).await;
    assert_eq!(blocked.title, "Payments (Stripe)");
    assert_eq!(blocked.status, "blocked");
    assert_eq!(reload(ctx, &w.todo).await.status, "todo");
}

#[tokio::test]
#[serial]
async fn a_task_moved_out_keeps_no_saved_status() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let w = website(ctx).await;
    let shop = project(ctx, w.org_id, "Shop").await;
    project_model::Model::close(&ctx.db, w.org_id, w.project.id, w.user_id)
        .await
        .unwrap();

    let moved = reload(ctx, &w.blocked)
        .await
        .update_from(&ctx.db, &task_params("Payments", Some(shop.id), "done"))
        .await
        .unwrap();
    assert_eq!(moved.status_before_close, None);

    project_model::Model::close(&ctx.db, w.org_id, shop.id, w.user_id)
        .await
        .unwrap();
    project_model::Model::reopen(&ctx.db, w.org_id, shop.id)
        .await
        .unwrap();
    assert_eq!(
        reload(ctx, &w.blocked).await.status,
        "done",
        "Shop's reopen leaves a task that was done when Shop closed"
    );
}

#[tokio::test]
#[serial]
async fn a_closed_project_takes_no_new_or_moved_tasks() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let w = website(ctx).await;
    let shop = project(ctx, w.org_id, "Shop").await;
    let elsewhere = task(ctx, w.org_id, shop.id, "Shelf labels", "todo", None).await;
    project_model::Model::close(&ctx.db, w.org_id, w.project.id, w.user_id)
        .await
        .unwrap();
    let refusal = "Website is done. Reopen the project first.";

    let err = tasks::Model::create(
        &ctx.db,
        w.org_id,
        &task_params("Banner", Some(w.project.id), ""),
        None,
    )
    .await
    .unwrap_err();
    assert!(format!("{err:?}").contains(refusal), "{err:?}");

    let err = elsewhere
        .clone()
        .update_from(
            &ctx.db,
            &task_params("Shelf labels", Some(w.project.id), ""),
        )
        .await
        .unwrap_err();
    assert!(format!("{err:?}").contains(refusal), "{err:?}");
    assert_eq!(reload(ctx, &elsewhere).await.project_id, Some(shop.id));

    // A task that stays in it can still change.
    let back = reload(ctx, &w.todo)
        .await
        .update_from(
            &ctx.db,
            &task_params("Returns page", Some(w.project.id), "progress"),
        )
        .await
        .unwrap();
    assert_eq!(back.status, "progress");
    assert_eq!(back.status_before_close, None);
}

/// A task insert holding `lock_open_for_tasks` makes a close wait, and the
/// close then marks the new task done too. Deterministic: the close is
/// checked to be still waiting before the insert commits.
#[tokio::test]
#[serial]
async fn a_task_added_during_a_close_is_closed_too() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let w = website(ctx).await;
    let uri = ctx.config.database.uri.clone();
    let inserting = Database::connect(&uri).await.unwrap();
    let closing = Database::connect(&uri).await.unwrap();

    let txn = inserting.begin().await.unwrap();
    project_model::lock_open_for_tasks(&txn, w.org_id, w.project.id)
        .await
        .unwrap();
    let (org_id, project_id, user_id) = (w.org_id, w.project.id, w.user_id);
    let close = tokio::spawn(async move {
        project_model::Model::close(&closing, org_id, project_id, user_id).await
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!close.is_finished(), "the close waits for the insert");

    let added = tasks::ActiveModel {
        title: ActiveValue::Set("Late addition".to_string()),
        status: ActiveValue::Set("todo".to_string()),
        priority: ActiveValue::Set("medium".to_string()),
        sort_order: ActiveValue::Set(9),
        project_id: ActiveValue::Set(Some(project_id)),
        ..Default::default()
    }
    .set_tenant(org_id)
    .unwrap()
    .insert(&txn)
    .await
    .unwrap();
    txn.commit().await.unwrap();

    let closed = close.await.unwrap().unwrap();
    assert!(closed.tasks.iter().any(|t| t.id == added.id));
    assert_eq!(closed.tasks.len(), 4);
    assert_eq!(reload(ctx, &added).await.status, "done");

    // The other order: the close has committed, so the insert is refused.
    let err = tasks::Model::create(
        &inserting,
        org_id,
        &task_params("Too late", Some(project_id), ""),
        None,
    )
    .await
    .unwrap_err();
    assert!(format!("{err:?}").contains("Reopen the project first."));
}
