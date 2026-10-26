//! Card order through the web: the two move routes, what they tell people,
//! and where the order shows. Juniper Tea Co: Ana (owner), Maya (admin), Tom
//! and Leo (members).
use collab::{
    app::App,
    models::{notifications, projects, tasks},
};
use loco_rs::{app::AppContext, testing::prelude::*, TestServer};
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
use serde_json::json;
use serial_test::serial;

use super::{
    juniper::juniper,
    mcp::{token_for, tool},
    prepare_data::{join, AsMember, TestMember},
    roadmap_done::task_in,
    tasks::{own, project_in},
};

const ORG: &str = "juniper-tea-co";

/// Posts a move as the page does. Returns the status and the `HX-Trigger`.
async fn post_move(
    request: &TestServer,
    who: &TestMember,
    path: &str,
    form: serde_json::Value,
) -> (u16, String) {
    let res = request
        .post(&who.at(path))
        .as_member(who)
        .add_header("HX-Request", "true")
        .form(&form)
        .await;
    assert!(
        res.headers().get("HX-Redirect").is_none(),
        "never a stale tab"
    );
    let trigger = res
        .headers()
        .get("HX-Trigger")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    (res.status_code().as_u16(), trigger)
}

async fn move_task(
    request: &TestServer,
    who: &TestMember,
    task: &tasks::Model,
    form: serde_json::Value,
) -> (u16, String) {
    post_move(request, who, &format!("/tasks/{}/move", task.id), form).await
}

async fn move_project(
    request: &TestServer,
    who: &TestMember,
    project: &projects::Model,
    form: serde_json::Value,
) -> (u16, String) {
    post_move(
        request,
        who,
        &format!("/roadmap/projects/{}/move", project.id),
        form,
    )
    .await
}

async fn page(request: &TestServer, who: &TestMember, path: &str) -> String {
    let res = request.get(&who.at(path)).as_member(who).await;
    assert_eq!(res.status_code(), 200, "{path}");
    res.text()
}

/// `names` in the order they first appear in `html`.
fn order_in<'a>(html: &str, names: &[&'a str]) -> Vec<&'a str> {
    let mut found: Vec<(usize, &str)> = names
        .iter()
        .map(|n| {
            (
                html.find(n)
                    .unwrap_or_else(|| panic!("{n} not on the page")),
                *n,
            )
        })
        .collect();
    found.sort_unstable();
    found.into_iter().map(|(_, n)| n).collect()
}

/// Titles in one status, in list order, from the database.
async fn column(ctx: &AppContext, org_id: i64, status: &str) -> Vec<String> {
    tasks::Model::list_for_org(&ctx.db, org_id)
        .await
        .unwrap()
        .into_iter()
        .filter(|t| t.status == status)
        .map(|t| t.title)
        .collect()
}

async fn notices(ctx: &AppContext) -> u64 {
    notifications::Entity::find().count(&ctx.db).await.unwrap()
}

#[tokio::test]
#[serial]
async fn a_move_inside_a_column_shows_for_everyone() {
    request::<App, _, _>(|request, ctx| async move {
        let t = juniper(&request, &ctx).await;
        let website = project_in(&ctx, ORG, "Website").await;
        own(&ctx, &website, &[t.ana_id, t.leo_id]).await;
        let pid = Some(website.id);
        let courier = task_in(&ctx, t.org_id, pid, "Book the courier", "todo", &[]).await;
        let jars = task_in(&ctx, t.org_id, pid, "Label the jars", "todo", &[]).await;
        let tins = task_in(&ctx, t.org_id, pid, "Order tins", "todo", &[]).await;
        let quiet = notices(&ctx).await;

        let (status, trigger) = move_task(
            &request,
            &t.ana,
            &tins,
            json!({ "status": "todo", "before": courier.id }),
        )
        .await;
        assert_eq!(status, 200);
        assert!(trigger.is_empty(), "no toast: {trigger}");
        assert_eq!(notices(&ctx).await, quiet, "a reorder tells no one");

        let names = ["Order tins", "Book the courier", "Label the jars"];
        let board = page(&request, &t.ana, "/tasks/board").await;
        assert_eq!(order_in(&board, &names), names);
        let list = page(&request, &t.tom, "/tasks/list?filter=all").await;
        assert_eq!(order_in(&list, &names), names);

        // After the last card: it ends up last.
        move_task(
            &request,
            &t.tom,
            &tins,
            json!({ "status": "todo", "after": jars.id }),
        )
        .await;
        assert_eq!(
            column(&ctx, t.org_id, "todo").await,
            ["Book the courier", "Label the jars", "Order tins"]
        );
    })
    .await;
}

/// The bug this fixes: a card dropped into another column jumped back to its
/// old place after the refresh.
#[tokio::test]
#[serial]
async fn a_drop_into_another_column_lands_where_dropped() {
    request::<App, _, _>(|request, ctx| async move {
        let t = juniper(&request, &ctx).await;
        let website = project_in(&ctx, ORG, "Website").await;
        own(&ctx, &website, &[t.ana_id, t.leo_id]).await;
        let pid = Some(website.id);
        let courier = task_in(&ctx, t.org_id, pid, "Book the courier", "todo", &[]).await;
        task_in(&ctx, t.org_id, pid, "Price list", "progress", &[]).await;
        let photos = task_in(&ctx, t.org_id, pid, "Photos", "progress", &[]).await;

        let (status, _) = move_task(
            &request,
            &t.maya,
            &courier,
            json!({ "status": "progress", "before": photos.id }),
        )
        .await;
        assert_eq!(status, 200);
        let names = ["Price list", "Book the courier", "Photos"];
        assert_eq!(column(&ctx, t.org_id, "progress").await, names);
        let board = page(&request, &t.maya, "/tasks/board").await;
        assert_eq!(order_in(&board, &names), names);
        // A status change: the owners hear about blocked, once each.
        let tins = task_in(&ctx, t.org_id, pid, "Order tins", "todo", &[]).await;
        let before = notices(&ctx).await;
        move_task(&request, &t.tom, &tins, json!({ "status": "blocked" })).await;
        assert_eq!(notices(&ctx).await, before + 2, "Ana and Leo");
    })
    .await;
}

/// With Mine on, Leo sees A and C; B, not his, sits between them. Moving C
/// before A leaves B where it was relative to A.
#[tokio::test]
#[serial]
async fn hidden_cards_keep_their_place() {
    request::<App, _, _>(|request, ctx| async move {
        let t = juniper(&request, &ctx).await;
        let website = project_in(&ctx, ORG, "Website").await;
        let shop = project_in(&ctx, ORG, "Instagram shop").await;
        let a = task_in(&ctx, t.org_id, Some(website.id), "A", "todo", &[t.leo_id]).await;
        task_in(&ctx, t.org_id, Some(shop.id), "B", "todo", &[t.tom_id]).await;
        let c = task_in(&ctx, t.org_id, Some(website.id), "C", "todo", &[t.leo_id]).await;

        move_task(
            &request,
            &t.leo,
            &c,
            json!({ "status": "todo", "before": a.id }),
        )
        .await;
        assert_eq!(column(&ctx, t.org_id, "todo").await, ["C", "A", "B"]);
        // In the project swimlane, B (another project) kept its place after A.
        move_task(
            &request,
            &t.leo,
            &a,
            json!({ "status": "todo", "before": c.id }),
        )
        .await;
        assert_eq!(column(&ctx, t.org_id, "todo").await, ["A", "C", "B"]);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn bad_moves_are_refused() {
    request::<App, _, _>(|request, ctx| async move {
        let t = juniper(&request, &ctx).await;
        let a = task_in(&ctx, t.org_id, None, "A", "todo", &[]).await;
        let b = task_in(&ctx, t.org_id, None, "B", "todo", &[]).await;
        let stuck = task_in(&ctx, t.org_id, None, "Stuck", "todo", &[]).await;

        for form in [
            json!({ "status": "done" }),
            json!({ "status": "someday" }),
            json!({ "status": "todo", "before": b.id, "after": b.id }),
            json!({ "status": "todo", "before": a.id }),
        ] {
            let (status, _) = move_task(&request, &t.ana, &a, form.clone()).await;
            assert_eq!(status, 400, "{form}");
        }
        // The anchor moved to Blocked first: stale, nothing changed.
        move_task(&request, &t.ana, &stuck, json!({ "status": "blocked" })).await;
        let (status, trigger) = move_task(
            &request,
            &t.ana,
            &a,
            json!({ "status": "todo", "before": stuck.id }),
        )
        .await;
        assert_eq!(status, 409);
        assert!(trigger.is_empty());
        assert_eq!(column(&ctx, t.org_id, "todo").await, ["A", "B"]);

        // Someone waiting for approval gets the usual refusal on both routes.
        let pending = join(&request, &ctx, ORG, "sam", "sam@example.com").await;
        let website = project_in(&ctx, ORG, "Website").await;
        for (path, form) in [
            (format!("/tasks/{}/move", a.id), json!({ "status": "todo" })),
            (
                format!("/roadmap/projects/{}/move", website.id),
                json!({ "lane": "next" }),
            ),
        ] {
            let res = request
                .post(&pending.at(&path))
                .as_member(&pending)
                .form(&form)
                .await;
            assert_ne!(res.status_code(), 200, "{path}");
        }
        assert_eq!(column(&ctx, t.org_id, "todo").await, ["A", "B"]);
        let website = projects::Entity::find_by_id(website.id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(website.lane, "now");
    })
    .await;
}

/// The roadmap's order shows wherever projects are listed.
#[tokio::test]
#[serial]
async fn the_roadmap_order_shows_everywhere() {
    request::<App, _, _>(|request, ctx| async move {
        let t = juniper(&request, &ctx).await;
        for name in ["Website", "Wholesale catalogue", "Instagram shop"] {
            project_in(&ctx, ORG, name).await;
        }
        let shop = projects::Entity::find()
            .filter(projects::Column::Name.eq("Instagram shop"))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        let website = projects::Entity::find()
            .filter(projects::Column::Name.eq("Website"))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();

        let (status, trigger) = move_project(
            &request,
            &t.maya,
            &shop,
            json!({ "lane": "now", "before": website.id }),
        )
        .await;
        assert_eq!(status, 200);
        assert!(trigger.is_empty(), "no toast: {trigger}");
        let names = ["Instagram shop", "Website", "Wholesale catalogue"];
        let roadmap = page(&request, &t.tom, "/roadmap").await;
        assert_eq!(order_in(&roadmap, &names), names);
        let home = page(&request, &t.tom, "/dashboard").await;
        assert_eq!(order_in(&home, &names), names);
        let form = page(&request, &t.tom, "/tasks/new").await;
        assert_eq!(order_in(&form, &names), names);
        let token = token_for(&ctx, "tom@example.com").await;
        let (_, list) = tool(&request, &token, "list_projects", json!({})).await;
        let listed: Vec<&str> = list["projects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(listed, names);

        // Across lanes, before a card there.
        let spring = project_in(&ctx, ORG, "Spring blend").await;
        move_project(&request, &t.maya, &spring, json!({ "lane": "next" })).await;
        let (status, _) = move_project(
            &request,
            &t.maya,
            &website,
            json!({ "lane": "next", "before": spring.id }),
        )
        .await;
        assert_eq!(status, 200);
        let next: Vec<String> = projects::Model::list_for_org(&ctx.db, t.org_id)
            .await
            .unwrap()
            .into_iter()
            .filter(|p| p.in_lane("next"))
            .map(|p| p.name)
            .collect();
        assert_eq!(next, ["Website", "Spring blend"]);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_closed_project_and_the_old_routes() {
    request::<App, _, _>(|request, ctx| async move {
        let t = juniper(&request, &ctx).await;
        let website = project_in(&ctx, ORG, "Website").await;
        let shop = project_in(&ctx, ORG, "Instagram shop").await;
        let europe = project_in(&ctx, ORG, "Ship to Europe").await;
        move_project(&request, &t.maya, &europe, json!({ "lane": "later" })).await;
        projects::Model::close(
            &ctx.db,
            t.org_id,
            website.id,
            t.ana_id,
            projects::CloseScope::Everything,
        )
        .await
        .unwrap();
        let (status, trigger) =
            move_project(&request, &t.maya, &website, json!({ "lane": "now" })).await;
        assert_eq!(status, 409);
        assert!(
            trigger.contains("Website is done. Reopen the project first."),
            "{trigger}"
        );

        // The old lane route, from tabs drawn before the deploy: the bottom.
        let res = request
            .post(&t.maya.at(&format!("/roadmap/projects/{}/lane", shop.id)))
            .as_member(&t.maya)
            .add_header("HX-Request", "true")
            .form(&json!({ "lane": "later" }))
            .await;
        assert_eq!(res.status_code(), 200);
        let order: Vec<String> = projects::Model::list_for_org(&ctx.db, t.org_id)
            .await
            .unwrap()
            .into_iter()
            .filter(|p| p.in_lane("later"))
            .map(|p| p.name)
            .collect();
        assert_eq!(order, ["Ship to Europe", "Instagram shop"]);

        // The old status route: the bottom of the new column.
        let a = task_in(&ctx, t.org_id, None, "A", "progress", &[]).await;
        let b = task_in(&ctx, t.org_id, None, "B", "todo", &[]).await;
        let c = task_in(&ctx, t.org_id, None, "C", "progress", &[]).await;
        move_task(
            &request,
            &t.tom,
            &a,
            json!({ "status": "progress", "after": c.id }),
        )
        .await;
        request
            .post(&t.tom.at(&format!("/tasks/{}/status", b.id)))
            .as_member(&t.tom)
            .add_header("HX-Request", "true")
            .form(&json!({ "status": "progress" }))
            .await;
        assert_eq!(column(&ctx, t.org_id, "progress").await, ["C", "A", "B"]);
    })
    .await;
}

/// A move that waits more than 5 seconds for another one gives up: 409 and
/// a toast, never a 500.
#[tokio::test]
#[serial]
async fn a_busy_move_says_try_again() {
    request::<App, _, _>(|request, ctx| async move {
        use sea_orm::TransactionTrait;
        let t = juniper(&request, &ctx).await;
        let a = task_in(&ctx, t.org_id, None, "A", "todo", &[]).await;
        let conn = sea_orm::Database::connect(&ctx.config.database.uri)
            .await
            .unwrap();
        let held = conn.begin().await.unwrap();
        collab::models::order::lock(&held, "tasks", t.org_id)
            .await
            .unwrap();
        let (status, trigger) =
            move_task(&request, &t.ana, &a, json!({ "status": "blocked" })).await;
        held.rollback().await.unwrap();
        assert_eq!(status, 409);
        assert!(
            trigger.contains("Someone else is moving cards. Try again."),
            "{trigger}"
        );
        assert_eq!(column(&ctx, t.org_id, "todo").await, ["A"]);
    })
    .await;
}

/// A close or delete waiting behind a move gives up the same way: 409 and a
/// toast, with nothing changed.
#[tokio::test]
#[serial]
async fn a_busy_close_or_delete_says_try_again() {
    request::<App, _, _>(|request, ctx| async move {
        use sea_orm::TransactionTrait;
        let t = juniper(&request, &ctx).await;
        let website = project_in(&ctx, ORG, "Website").await;
        task_in(&ctx, t.org_id, Some(website.id), "A", "todo", &[]).await;
        let conn = sea_orm::Database::connect(&ctx.config.database.uri)
            .await
            .unwrap();
        let held = conn.begin().await.unwrap();
        collab::models::order::lock(&held, "tasks", t.org_id)
            .await
            .unwrap();
        for action in ["close", "delete"] {
            let res = request
                .post(
                    &t.ana
                        .at(&format!("/roadmap/projects/{}/{action}", website.id)),
                )
                .as_member(&t.ana)
                .add_header("HX-Request", "true")
                .await;
            assert_eq!(res.status_code(), 409, "{action}");
            let trigger = res.headers()["hx-trigger"].to_str().unwrap().to_string();
            assert!(
                trigger.contains("Someone else is moving cards. Try again."),
                "{action}: {trigger}"
            );
        }
        held.rollback().await.unwrap();
        let website = projects::Entity::find_by_id(website.id)
            .one(&ctx.db)
            .await
            .unwrap()
            .expect("not deleted");
        assert!(!website.is_closed());
        assert_eq!(column(&ctx, t.org_id, "todo").await, ["A"]);
    })
    .await;
}

/// What the pages draw for ordering: arrows on open cards only, Reorder
/// where it can be used, and the "newest first" labels on Done.
#[tokio::test]
#[serial]
async fn the_pages_draw_reorder_where_it_works() {
    request::<App, _, _>(|request, ctx| async move {
        let t = juniper(&request, &ctx).await;
        let website = project_in(&ctx, ORG, "Website").await;
        let pid = Some(website.id);
        task_in(&ctx, t.org_id, pid, "Order tins", "todo", &[t.ana_id]).await;
        task_in(&ctx, t.org_id, pid, "Book the courier", "todo", &[t.ana_id]).await;
        task_in(&ctx, t.org_id, pid, "Old label", "done", &[t.ana_id]).await;

        let tasks_page = page(&request, &t.ana, "/tasks?filter=all").await;
        // A card is a div with a link over it, never a button inside a link.
        assert!(tasks_page.contains(r#"class="task-card" draggable="true""#));
        assert!(tasks_page.contains(r#"class="task-card-link" draggable="false""#));
        assert!(tasks_page.contains(r#"class="list-row-link" draggable="false""#));
        assert!(tasks_page.contains(r#"data-name="Order tins""#));
        assert!(tasks_page.contains(r#"aria-label="Move Order tins up""#));
        assert!(
            !tasks_page.contains("Move Old label"),
            "no arrows on done cards"
        );
        assert!(tasks_page.contains("· Newest first"));
        assert_eq!(
            tasks_page.matches(r#"data-reorder-start="tasks""#).count(),
            2,
            "board and phone list"
        );
        // The first card's up arrow and the last's down arrow are hidden.
        assert!(tasks_page.contains(
            r#"class="move-btn is-none" data-move="up" aria-label="Move Order tins up" tabindex="-1" aria-hidden="true""#
        ));
        assert!(tasks_page.contains(r#"class="move-btn" data-move="up" aria-label="Move Book the courier up">"#));
        assert!(tasks_page.contains(
            r#"class="move-btn is-none" data-move="down" aria-label="Move Book the courier down""#
        ));
        assert!(
            tasks_page.contains(r#"<div id="reorder-status" class="sr-only" aria-live="polite">"#)
        );

        let done = page(&request, &t.ana, "/tasks?filter=done").await;
        assert!(
            !done.contains("data-reorder-start"),
            "Done keeps its own order"
        );
        let table = page(&request, &t.ana, "/tasks?view=list").await;
        assert_eq!(
            table.matches("data-reorder-start").count(),
            1,
            "only the phone list"
        );

        let roadmap = page(&request, &t.ana, "/roadmap?lane=done").await;
        assert!(roadmap.contains("Newest first"));
        assert!(roadmap.contains(r#"data-reorder-start="roadmap""#));
        assert!(roadmap.contains(r#"aria-label="Move Website up""#));
        assert!(!roadmap.contains("data-reordering"));
    })
    .await;
}

/// A phone lane switch in Reorder mode carries `reorder=1`, and the next
/// page opens in the mode.
#[tokio::test]
#[serial]
async fn reorder_mode_survives_a_lane_switch() {
    request::<App, _, _>(|request, ctx| async move {
        let t = juniper(&request, &ctx).await;
        project_in(&ctx, ORG, "Website").await;
        let plain = page(&request, &t.maya, "/roadmap?lane=next").await;
        assert!(
            plain.contains("data-lane-switch"),
            "the links the client adds it to"
        );
        assert!(!plain.contains(r#"data-reordering="roadmap""#));
        let switched = page(&request, &t.maya, "/roadmap?lane=now&reorder=1").await;
        assert!(switched.contains(r#"data-reordering="roadmap""#));
        let member = page(&request, &t.leo, "/roadmap?lane=now&reorder=1").await;
        assert!(
            !member.contains(r#"data-reordering="roadmap""#),
            "members don't reorder"
        );
    })
    .await;
}
