//! Card order: `move_to` on tasks and projects, the bottom place for other
//! status and lane changes, and moves at the same time.
use std::time::Duration;

use collab::{
    app::App,
    models::{
        order::{self, Place, STEP},
        organisations::{self, SignupParams},
        projects::{self, MoveRefusal, OwnerRights, ProjectParams},
        tasks::{self, Stale, TaskParams},
    },
};
use loco_rs::{app::AppContext, testing::prelude::*};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseConnection, EntityTrait,
    IntoActiveModel, TransactionTrait,
};
use serial_test::serial;

use crate::locks::waiting_locks;

async fn org(ctx: &AppContext, name: &str, owner: &str) -> i64 {
    organisations::Model::sign_up(
        &ctx.db,
        &SignupParams {
            organisation_name: name.to_string(),
            name: owner.to_string(),
            email: format!("{owner}@example.com"),
            password: "correct-horse".to_string(),
        },
    )
    .await
    .unwrap()
    .1
    .id
}

fn project_params(name: &str, lane: &str) -> ProjectParams {
    ProjectParams {
        name: name.to_string(),
        lane: lane.to_string(),
        status: "Active".to_string(),
        accent: "#6cb6ff".to_string(),
        owner_ids: vec![],
        summary: String::new(),
    }
}

async fn project(ctx: &AppContext, org_id: i64, name: &str, lane: &str) -> projects::Model {
    projects::Model::create(&ctx.db, org_id, &project_params(name, lane), None)
        .await
        .unwrap()
}

fn task_params(title: &str, status: &str) -> TaskParams {
    TaskParams {
        title: title.to_string(),
        project_id: String::new(),
        assignee_ids: vec![],
        priority: "medium".to_string(),
        due_on: String::new(),
        status: status.to_string(),
    }
}

async fn task(ctx: &AppContext, org_id: i64, title: &str, status: &str) -> tasks::Model {
    tasks::Model::create(&ctx.db, org_id, &task_params(title, status), None)
        .await
        .unwrap()
}

/// Gives each card the old style of number (1, 2, 3...), as before this
/// change, so a move between two of them has no room.
async fn squash(ctx: &AppContext, cards: &[&tasks::Model]) {
    for (i, t) in cards.iter().enumerate() {
        let mut active = (*t).clone().into_active_model();
        active.sort_order = ActiveValue::Set(i64::try_from(i).unwrap() + 1);
        active.update(&ctx.db).await.unwrap();
    }
}

/// Titles in one column, in list order.
async fn column(ctx: &AppContext, org_id: i64, status: &str) -> Vec<String> {
    tasks::Model::list_for_org(&ctx.db, org_id)
        .await
        .unwrap()
        .into_iter()
        .filter(|t| t.status == status)
        .map(|t| t.title)
        .collect()
}

async fn lane(ctx: &AppContext, org_id: i64, lane: &str) -> Vec<String> {
    projects::Model::list_for_org(&ctx.db, org_id)
        .await
        .unwrap()
        .into_iter()
        .filter(|p| p.in_lane(lane))
        .map(|p| p.name)
        .collect()
}

async fn reload(ctx: &AppContext, t: &tasks::Model) -> tasks::Model {
    tasks::Entity::find_by_id(t.id)
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap()
}

async fn move_task(
    ctx: &AppContext,
    org_id: i64,
    t: &tasks::Model,
    status: &str,
    place: Place,
) -> Result<tasks::Moved, Stale> {
    tasks::Model::move_to(&ctx.db, org_id, t.id, status, place)
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn moving_inside_a_column_writes_the_new_order() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let org_id = org(ctx, "Juniper Tea Co", "ana").await;
    let a = task(ctx, org_id, "Order tins", "todo").await;
    let b = task(ctx, org_id, "Book the courier", "todo").await;
    let c = task(ctx, org_id, "Label the jars", "todo").await;
    let elsewhere = task(ctx, org_id, "Price list", "progress").await;
    assert_eq!(b.sort_order - a.sort_order, STEP, "new cards leave room");

    let moved = move_task(ctx, org_id, &c, "todo", Place::Before(a.id))
        .await
        .unwrap();
    assert!(!moved.changed_status);
    assert_eq!(
        column(ctx, org_id, "todo").await,
        ["Label the jars", "Order tins", "Book the courier"]
    );
    move_task(ctx, org_id, &c, "todo", Place::After(b.id))
        .await
        .unwrap();
    assert_eq!(
        column(ctx, org_id, "todo").await,
        ["Order tins", "Book the courier", "Label the jars"]
    );
    move_task(ctx, org_id, &a, "todo", Place::End)
        .await
        .unwrap();
    assert_eq!(
        column(ctx, org_id, "todo").await,
        ["Book the courier", "Label the jars", "Order tins"]
    );
    // A reorder isn't an edit.
    for t in [&a, &b, &c, &elsewhere] {
        assert_eq!(reload(ctx, t).await.updated_at, t.updated_at, "{}", t.title);
    }
}

#[tokio::test]
#[serial]
async fn only_a_move_with_no_room_renumbers() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let org_id = org(ctx, "Juniper Tea Co", "ana").await;
    let a = task(ctx, org_id, "A", "todo").await;
    let b = task(ctx, org_id, "B", "todo").await;
    let c = task(ctx, org_id, "C", "todo").await;
    let d = task(ctx, org_id, "D", "todo").await;
    squash(ctx, &[&a, &b, &c, &d]).await;
    let before: Vec<i64> = tasks::Model::list_for_org(&ctx.db, org_id)
        .await
        .unwrap()
        .iter()
        .map(|t| t.sort_order)
        .collect();
    assert_eq!(before, [1, 2, 3, 4]);

    // To the top: room below the first card, one row written.
    move_task(ctx, org_id, &d, "todo", Place::Before(a.id))
        .await
        .unwrap();
    assert_eq!(reload(ctx, &a).await.sort_order, 1);
    assert_eq!(column(ctx, org_id, "todo").await, ["D", "A", "B", "C"]);

    // Between A (1) and B (2): no room, so the column is renumbered.
    move_task(ctx, org_id, &c, "todo", Place::After(a.id))
        .await
        .unwrap();
    let numbers: Vec<(String, i64)> = tasks::Model::list_for_org(&ctx.db, org_id)
        .await
        .unwrap()
        .into_iter()
        .map(|t| (t.title, t.sort_order))
        .collect();
    assert_eq!(
        numbers,
        [
            ("D".to_string(), STEP),
            ("A".to_string(), 2 * STEP),
            ("C".to_string(), 3 * STEP),
            ("B".to_string(), 4 * STEP),
        ]
    );
    // Now there is room: the next move writes one row.
    move_task(ctx, org_id, &b, "todo", Place::Before(c.id))
        .await
        .unwrap();
    assert_eq!(reload(ctx, &b).await.sort_order, 2 * STEP + STEP / 2);
    for (t, n) in [(&d, STEP), (&a, 2 * STEP), (&c, 3 * STEP)] {
        assert_eq!(reload(ctx, t).await.sort_order, n);
    }
}

#[tokio::test]
#[serial]
async fn moving_to_another_column_changes_status_and_place() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let org_id = org(ctx, "Juniper Tea Co", "ana").await;
    let tins = task(ctx, org_id, "Order tins", "todo").await;
    let first = task(ctx, org_id, "Price list", "progress").await;
    let second = task(ctx, org_id, "Photos", "progress").await;

    let moved = move_task(ctx, org_id, &tins, "progress", Place::Before(second.id))
        .await
        .unwrap();
    assert!(moved.changed_status);
    assert_eq!(moved.task.status, "progress");
    assert_eq!(
        column(ctx, org_id, "progress").await,
        ["Price list", "Order tins", "Photos"]
    );
    assert!(reload(ctx, &tins).await.updated_at > tins.updated_at);
    assert_eq!(reload(ctx, &first).await.updated_at, first.updated_at);
    assert!(reload(ctx, &tins).await.done_at.is_none());
}

#[tokio::test]
#[serial]
async fn moves_that_are_refused() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let org_id = org(ctx, "Juniper Tea Co", "ana").await;
    let other_org = org(ctx, "Bramble Bakery", "bob").await;
    let a = task(ctx, org_id, "Order tins", "todo").await;
    let b = task(ctx, org_id, "Book the courier", "todo").await;
    let blocked = task(ctx, org_id, "Supplier", "blocked").await;
    let foreign = task(ctx, other_org, "Their task", "todo").await;

    for (status, place) in [
        ("done", Place::End),
        ("someday", Place::End),
        ("todo", Place::Before(a.id)),
    ] {
        assert!(
            tasks::Model::move_to(&ctx.db, org_id, a.id, status, place)
                .await
                .is_err(),
            "{status} {place:?}"
        );
    }
    // An anchor in another column, or another organisation: stale.
    assert_eq!(
        move_task(ctx, org_id, &a, "todo", Place::Before(blocked.id))
            .await
            .unwrap_err(),
        Stale
    );
    assert_eq!(
        move_task(ctx, org_id, &a, "todo", Place::Before(foreign.id))
            .await
            .unwrap_err(),
        Stale
    );
    // Another organisation's card: not found.
    assert!(matches!(
        tasks::Model::move_to(&ctx.db, org_id, foreign.id, "todo", Place::End).await,
        Err(loco_rs::model::ModelError::EntityNotFound)
    ));
    assert_eq!(
        column(ctx, org_id, "todo").await,
        ["Order tins", "Book the courier"]
    );
    assert_eq!(reload(ctx, &b).await.sort_order, b.sort_order);
}

/// Any other status change puts the card at the bottom of its new column;
/// no change keeps its place.
#[tokio::test]
#[serial]
async fn other_status_changes_go_to_the_bottom() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let org_id = org(ctx, "Juniper Tea Co", "ana").await;
    let tins = task(ctx, org_id, "Order tins", "todo").await;
    task(ctx, org_id, "Price list", "progress").await;
    let photos = task(ctx, org_id, "Photos", "todo").await;
    let courier = task(ctx, org_id, "Book the courier", "todo").await;

    tins.clone().set_status(&ctx.db, "progress").await.unwrap();
    assert_eq!(
        column(ctx, org_id, "progress").await,
        ["Price list", "Order tins"]
    );
    let kept = reload(ctx, &photos)
        .await
        .set_status(&ctx.db, "todo")
        .await
        .unwrap();
    assert_eq!(kept.sort_order, photos.sort_order);

    // The task form with a new status, then with the same one.
    let saved = reload(ctx, &photos)
        .await
        .update_from(&ctx.db, &task_params("Photos", "progress"))
        .await
        .unwrap();
    assert_eq!(
        column(ctx, org_id, "progress").await,
        ["Price list", "Order tins", "Photos"]
    );
    let again = saved
        .clone()
        .update_from(&ctx.db, &task_params("Photos again", "progress"))
        .await
        .unwrap();
    assert_eq!(again.sort_order, saved.sort_order);
    assert_eq!(column(ctx, org_id, "todo").await, ["Book the courier"]);
    assert_eq!(reload(ctx, &courier).await.sort_order, courier.sort_order);
}

#[tokio::test]
#[serial]
async fn projects_move_inside_and_across_lanes() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let org_id = org(ctx, "Juniper Tea Co", "ana").await;
    let other_org = org(ctx, "Bramble Bakery", "bob").await;
    let tasting = project(ctx, org_id, "Tea tasting evenings", "next").await;
    let gift = project(ctx, org_id, "Gift box redesign", "next").await;
    let spring = project(ctx, org_id, "Spring blend", "next").await;
    let website = project(ctx, org_id, "Website", "now").await;
    let theirs = project(ctx, other_org, "Bakery site", "next").await;

    let moved =
        projects::Model::move_to(&ctx.db, org_id, spring.id, "next", Place::Before(gift.id))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(moved.lane, "next");
    assert_eq!(
        lane(ctx, org_id, "next").await,
        ["Tea tasting evenings", "Spring blend", "Gift box redesign"]
    );
    let saved = projects::Entity::find_by_id(spring.id)
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.updated_at, spring.updated_at, "a move isn't an edit");

    // Across lanes, before a card.
    projects::Model::move_to(
        &ctx.db,
        org_id,
        tasting.id,
        "now",
        Place::Before(website.id),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        lane(ctx, org_id, "now").await,
        ["Tea tasting evenings", "Website"]
    );

    // Stale anchors, another org's project, a closed project.
    assert!(matches!(
        projects::Model::move_to(&ctx.db, org_id, gift.id, "next", Place::Before(website.id)).await,
        Ok(Err(MoveRefusal::Stale))
    ));
    assert!(matches!(
        projects::Model::move_to(&ctx.db, org_id, gift.id, "next", Place::Before(theirs.id)).await,
        Ok(Err(MoveRefusal::Stale))
    ));
    assert!(matches!(
        projects::Model::move_to(&ctx.db, org_id, theirs.id, "next", Place::End).await,
        Err(loco_rs::model::ModelError::EntityNotFound)
    ));
    let ana = collab::models::memberships::Model::owners(&ctx.db, org_id)
        .await
        .unwrap()[0]
        .1
        .id;
    projects::Model::close(&ctx.db, org_id, gift.id, ana, true)
        .await
        .unwrap();
    assert!(matches!(
        projects::Model::move_to(&ctx.db, org_id, gift.id, "next", Place::End).await,
        Ok(Err(MoveRefusal::Closed(_)))
    ));
}

#[tokio::test]
#[serial]
async fn other_lane_changes_go_to_the_bottom() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let org_id = org(ctx, "Juniper Tea Co", "ana").await;
    let later = project(ctx, org_id, "Ship to Europe", "later").await;
    project(ctx, org_id, "Website", "now").await;
    let shop = project(ctx, org_id, "Instagram shop", "next").await;
    project(ctx, org_id, "Wholesale catalogue", "now").await;

    later.move_to_lane(&ctx.db, "now").await.unwrap();
    assert_eq!(
        lane(ctx, org_id, "now").await,
        ["Website", "Wholesale catalogue", "Ship to Europe"]
    );
    shop.clone()
        .update_from(
            &ctx.db,
            &project_params("Instagram shop", "now"),
            OwnerRights::Any,
        )
        .await
        .unwrap();
    assert_eq!(
        lane(ctx, org_id, "now").await,
        [
            "Website",
            "Wholesale catalogue",
            "Ship to Europe",
            "Instagram shop"
        ]
    );
    let shop = projects::Entity::find_by_id(shop.id)
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    let saved = shop
        .clone()
        .update_from(
            &ctx.db,
            &project_params("Instagram shop 2", "now"),
            OwnerRights::Any,
        )
        .await
        .unwrap();
    assert_eq!(
        saved.project.sort_order, shop.sort_order,
        "same lane, same place"
    );
}

/// A second connection holding an organisation's order lock in an open
/// transaction, as a move in progress does.
async fn hold_order_lock(
    ctx: &AppContext,
    what: &str,
    org_id: i64,
) -> (DatabaseConnection, sea_orm::DatabaseTransaction) {
    let conn = sea_orm::Database::connect(&ctx.config.database.uri)
        .await
        .unwrap();
    let txn = conn.begin().await.unwrap();
    order::lock(&txn, what, org_id).await.unwrap();
    (conn, txn)
}

/// The spawned side has not finished, and Postgres says it is waiting.
/// Asked on the holding connection: the test pool has one connection, and
/// the spawned side holds it.
async fn assert_waiting<T>(task: &tokio::task::JoinHandle<T>, conn: &DatabaseConnection) {
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!task.is_finished(), "it waits for the lock");
    assert!(waiting_locks(conn).await > 0);
}

/// Two moves in one organisation run one after the other: the second reads
/// the numbers the first wrote.
#[tokio::test]
#[serial]
async fn a_second_move_waits_and_places_against_the_first() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let org_id = org(ctx, "Juniper Tea Co", "ana").await;
    let a = task(ctx, org_id, "A", "todo").await;
    let b = task(ctx, org_id, "B", "todo").await;
    let c = task(ctx, org_id, "C", "todo").await;
    squash(ctx, &[&a, &b, &c]).await;

    let (conn, first) = hold_order_lock(ctx, "tasks", org_id).await;
    let db = ctx.db.clone();
    let c_id = c.id;
    let a_id = a.id;
    let second = tokio::spawn(async move {
        tasks::Model::move_to(&db, org_id, c_id, "todo", Place::After(a_id)).await
    });
    assert_waiting(&second, &conn).await;
    // The first move, in the held transaction: B to the top.
    first
        .execute_unprepared(&format!(
            "UPDATE tasks SET sort_order = 0 WHERE id = {}",
            b.id
        ))
        .await
        .unwrap();
    first.commit().await.unwrap();
    second.await.unwrap().unwrap().unwrap();
    assert_eq!(column(ctx, org_id, "todo").await, ["B", "A", "C"]);
}

/// A move that waits more than 5 seconds gives up with a busy error.
#[tokio::test]
#[serial]
async fn a_move_that_waits_too_long_is_busy() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let org_id = org(ctx, "Juniper Tea Co", "ana").await;
    let a = task(ctx, org_id, "A", "todo").await;
    let (_conn, held) = hold_order_lock(ctx, "tasks", org_id).await;
    let err = tasks::Model::move_to(&ctx.db, org_id, a.id, "todo", Place::End)
        .await
        .unwrap_err();
    assert!(order::is_busy(&err), "{err:?}");
    held.rollback().await.unwrap();
    // Another organisation's moves never waited.
    let other = org(ctx, "Bramble Bakery", "bob").await;
    let (_conn, held) = hold_order_lock(ctx, "tasks", org_id).await;
    let theirs = task(ctx, other, "Theirs", "todo").await;
    tasks::Model::move_to(&ctx.db, other, theirs.id, "todo", Place::End)
        .await
        .unwrap()
        .unwrap();
    held.rollback().await.unwrap();
}

/// T1: a renumber writes only rows still in the column it worked out. A
/// task whose status changes while the renumber waits for it keeps the
/// place it got.
#[tokio::test]
#[serial]
async fn a_renumber_skips_a_row_that_left_the_column() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let org_id = org(ctx, "Juniper Tea Co", "ana").await;
    let a = task(ctx, org_id, "A", "todo").await;
    let b = task(ctx, org_id, "B", "todo").await;
    let c = task(ctx, org_id, "C", "todo").await;
    squash(ctx, &[&a, &b, &c]).await;
    let b = reload(ctx, &b).await;

    // B's status change is in flight: the move still reads B in To do, then
    // its renumber waits on B's row.
    let conn = sea_orm::Database::connect(&ctx.config.database.uri)
        .await
        .unwrap();
    let changing = conn.begin().await.unwrap();
    let blocked = b.set_status(&changing, "blocked").await.unwrap();
    let db = ctx.db.clone();
    let (b_id, c_id) = (blocked.id, c.id);
    let moving = tokio::spawn(async move {
        tasks::Model::move_to(&db, org_id, c_id, "todo", Place::Before(b_id)).await
    });
    assert_waiting(&moving, &conn).await;
    changing.commit().await.unwrap();
    moving.await.unwrap().unwrap().unwrap();

    assert_eq!(reload(ctx, &a).await.sort_order, STEP);
    assert_eq!(reload(ctx, &c).await.sort_order, 2 * STEP);
    assert_eq!(reload(ctx, &blocked).await.sort_order, blocked.sort_order);
}

async fn setting(txn: &sea_orm::DatabaseTransaction, name: &str) -> String {
    txn.query_one_raw(sea_orm::Statement::from_sql_and_values(
        sea_orm::DbBackend::Postgres,
        "SELECT current_setting($1) AS v",
        [name.into()],
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get::<String>("", "v")
    .unwrap()
}

/// A close or delete holds the order lock while it writes many rows, so
/// only the wait for the lock has a limit.
#[tokio::test]
#[serial]
async fn the_long_lock_drops_the_timeouts_after_the_wait() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let txn = ctx.db.begin().await.unwrap();
    let before = setting(&txn, "statement_timeout").await;
    order::lock(&txn, "tasks", 1).await.unwrap();
    assert_eq!(setting(&txn, "statement_timeout").await, "10s");
    txn.rollback().await.unwrap();

    let txn = ctx.db.begin().await.unwrap();
    order::lock_then_unlimit(&txn, "tasks", 1).await.unwrap();
    assert_eq!(setting(&txn, "statement_timeout").await, before);
    assert_ne!(setting(&txn, "lock_timeout").await, "5s");
    txn.rollback().await.unwrap();
}

/// T2: a close takes the tasks' order lock first, so it waits for a move.
#[tokio::test]
#[serial]
async fn a_close_waits_for_a_move() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let org_id = org(ctx, "Juniper Tea Co", "ana").await;
    let website = project(ctx, org_id, "Website", "now").await;
    let mut params = task_params("Order tins", "todo");
    params.project_id = website.id.to_string();
    let tins = tasks::Model::create(&ctx.db, org_id, &params, None)
        .await
        .unwrap();
    let ana = collab::models::memberships::Model::owners(&ctx.db, org_id)
        .await
        .unwrap()[0]
        .1
        .id;

    let (conn, moving) = hold_order_lock(ctx, "tasks", org_id).await;
    let db = ctx.db.clone();
    let id = website.id;
    let close =
        tokio::spawn(async move { projects::Model::close(&db, org_id, id, ana, true).await });
    assert_waiting(&close, &conn).await;
    moving.commit().await.unwrap();
    let closed = close.await.unwrap().unwrap().unwrap();
    assert_eq!(closed.tasks.len(), 1);
    assert_eq!(reload(ctx, &tins).await.status, "done");
}
