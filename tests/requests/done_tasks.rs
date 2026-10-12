//! Keeping Done short: when tasks were finished, the board's capped Done
//! cells, and the Done view grouped by week and month.
use chrono::{DateTime, Duration, FixedOffset, Utc};
use collab::{
    app::App,
    models::{organisations, tasks, users},
};
use loco_rs::{app::AppContext, testing::prelude::*};
use migration::{MigratorTrait, SchemaManager};
use sea_orm::{ActiveModelTrait, ConnectionTrait, EntityTrait, Set};
use serial_test::serial;

use super::{
    prepare_data::sign_up,
    tasks::{project_in, task_form},
};

type Cookie = (axum::http::HeaderName, axum::http::HeaderValue);

async fn task_named(ctx: &AppContext, title: &str) -> tasks::Model {
    tasks::Entity::find()
        .all(&ctx.db)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.title == title)
        .unwrap()
}

/// Adds a done task and says when it was finished.
async fn done_task(
    request: &loco_rs::TestServer,
    ctx: &AppContext,
    owner: &Cookie,
    title: &str,
    project_id: i64,
    assignee: &str,
    done_at: DateTime<FixedOffset>,
) {
    let mut form = task_form(title, project_id, assignee);
    form["status"] = "done".into();
    request
        .post("/tasks")
        .add_header(owner.0.clone(), owner.1.clone())
        .form(&form)
        .await;
    let mut task: tasks::ActiveModel = task_named(ctx, title).await.into();
    task.done_at = Set(Some(done_at));
    task.update(&ctx.db).await.unwrap();
}

fn hours_ago(hours: i64) -> DateTime<FixedOffset> {
    (Utc::now() - Duration::hours(hours)).into()
}

#[tokio::test]
#[serial]
async fn done_at_follows_the_status_on_every_route() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let project = project_in(&ctx, "acme", "Launch").await;
        let done_at = |title: &'static str| {
            let ctx = &ctx;
            async move { task_named(ctx, title).await.done_at }
        };

        // Added straight into Done.
        let mut form = task_form("Born done", project.id, "");
        form["status"] = "done".into();
        request
            .post("/tasks")
            .add_header(owner.0.clone(), owner.1.clone())
            .form(&form)
            .await;
        assert!(done_at("Born done").await.is_some());

        request
            .post("/tasks")
            .add_header(owner.0.clone(), owner.1.clone())
            .form(&task_form("Ship it", project.id, ""))
            .await;
        let id = task_named(&ctx, "Ship it").await.id;
        assert_eq!(done_at("Ship it").await, None);

        // The status switcher and a drag on the board both post here.
        let status = |value: &'static str| {
            let request = &request;
            let cookie = owner.clone();
            async move {
                request
                    .post(&format!("/tasks/{id}/status"))
                    .add_header(cookie.0, cookie.1)
                    .form(&serde_json::json!({ "status": value }))
                    .await;
            }
        };
        status("done").await;
        let finished = done_at("Ship it").await.expect("set when done");
        status("todo").await;
        assert_eq!(
            done_at("Ship it").await,
            None,
            "cleared when it leaves done"
        );
        status("done").await;
        let finished_again = done_at("Ship it").await.unwrap();
        assert!(finished_again >= finished);

        // The edit form: saving a done task again keeps when it was finished.
        let edit = |value: &'static str| {
            let request = &request;
            let cookie = owner.clone();
            let mut form = task_form("Ship it", project.id, "");
            form["status"] = value.into();
            async move {
                request
                    .post(&format!("/tasks/{id}"))
                    .add_header(cookie.0, cookie.1)
                    .form(&form)
                    .await;
            }
        };
        edit("done").await;
        assert_eq!(done_at("Ship it").await, Some(finished_again));
        edit("progress").await;
        assert_eq!(done_at("Ship it").await, None);
        edit("done").await;
        assert!(done_at("Ship it").await.is_some());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_board_shows_the_latest_five_done_per_lane() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();
        let launch = project_in(&ctx, "acme", "Launch").await;
        let mobile = project_in(&ctx, "acme", "Mobile").await;
        let me = alice.id.to_string();
        // Finished one hour apart: "Launch done 1" is the newest.
        for i in 1..=7 {
            done_task(
                &request,
                &ctx,
                &owner,
                &format!("Launch done {i}"),
                launch.id,
                &me,
                hours_ago(i),
            )
            .await;
        }
        for i in 1..=3 {
            done_task(
                &request,
                &ctx,
                &owner,
                &format!("Mobile done {i}"),
                mobile.id,
                "",
                hours_ago(i),
            )
            .await;
        }
        request
            .post("/tasks")
            .add_header(owner.0.clone(), owner.1.clone())
            .form(&task_form("Launch still open", launch.id, ""))
            .await;

        let get = |url: &'static str| {
            let request = &request;
            let cookie = owner.clone();
            async move { request.get(url).add_header(cookie.0, cookie.1).await.text() }
        };
        let launch_link = format!(r#"href="/tasks?filter=done&project={}""#, launch.id);
        let mobile_link = format!(r#"href="/tasks?filter=done&project={}""#, mobile.id);
        for (url, layout) in [
            ("/tasks/board?group=project", "board"),
            ("/tasks/board?group=project&view=list", "list"),
        ] {
            let body = get(url).await;
            for i in 1..=5 {
                assert!(body.contains(&format!("Launch done {i}<")), "{layout}: {i}");
            }
            assert!(
                !body.contains("Launch done 6") && !body.contains("Launch done 7"),
                "{layout}: older done tasks are left off"
            );
            let first = body.find("Launch done 1<").unwrap();
            let fifth = body.find("Launch done 5<").unwrap();
            assert!(first < fifth, "{layout}: newest first");
            assert_eq!(body.matches(&launch_link).count(), 1, "{layout}");
            assert!(body.contains("All done tasks"));
            assert!(
                !body.contains(&mobile_link),
                "{layout}: 3 done needs no link"
            );
            // Progress still counts every done task.
            assert!(body.contains("8 tasks · 7 done"), "{layout}");
            assert!(body.contains(">87%<"), "{layout}");
        }

        // A person's lane links to that person's done tasks.
        let by_person = get("/tasks/board?group=person").await;
        assert!(by_person.contains(&format!(r#"href="/tasks?filter=done&person={}""#, alice.id)));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_done_view_groups_by_week_and_month_in_the_orgs_zone() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let launch = project_in(&ctx, "acme", "Launch").await;
        let mobile = project_in(&ctx, "acme", "Mobile").await;
        let at = |s: &str| DateTime::parse_from_rfc3339(s).unwrap();
        done_task(&request, &ctx, &owner, "Fresh", launch.id, "", hours_ago(0)).await;
        done_task(
            &request,
            &ctx,
            &owner,
            "Week old",
            launch.id,
            "",
            hours_ago(24 * 7),
        )
        .await;
        // 23:30 UTC on 31 August is already 1 September in London (BST).
        done_task(
            &request,
            &ctx,
            &owner,
            "Boundary",
            launch.id,
            "",
            at("2026-08-31T23:30:00Z"),
        )
        .await;
        done_task(
            &request,
            &ctx,
            &owner,
            "Summer",
            mobile.id,
            "",
            at("2026-07-15T12:00:00Z"),
        )
        .await;

        let get = |url: String| {
            let request = &request;
            let cookie = owner.clone();
            async move {
                request
                    .get(&url)
                    .add_header(cookie.0, cookie.1)
                    .await
                    .text()
            }
        };
        let body = get("/tasks/list?filter=done".into()).await;
        let labels = [
            "This week · 1",
            "Last week · 1",
            "September 2026 · 1",
            "July 2026 · 1",
        ];
        let positions: Vec<usize> = labels
            .iter()
            .map(|l| body.find(l).unwrap_or_else(|| panic!("{l} in {body}")))
            .collect();
        assert!(positions.is_sorted(), "newest group first");
        assert!(!body.contains("August 2026"));
        assert!(!body.contains(">Done ·"), "no single Done group");

        // The full page shows the Done view on desktop instead of the board.
        let page = get("/tasks?filter=done".into()).await;
        assert!(page.contains("list-view-done") && !page.contains(r#"id="task-board""#));

        // Search works within it.
        let found = get("/tasks/list?filter=done&q=bound".into()).await;
        assert!(found.contains("Boundary") && !found.contains("Fresh"));
        assert!(found.contains("September 2026 · 1") && !found.contains("This week"));

        // Narrowed to one project, from a board lane.
        let theirs = get(format!("/tasks/list?filter=done&project={}", mobile.id)).await;
        assert!(theirs.contains("Summer") && !theirs.contains("Fresh"));
        assert!(theirs.contains("done tasks for Mobile"));

        // The same instant falls in August in New York.
        let org = organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap();
        let mut org: organisations::ActiveModel = org.into();
        org.timezone = Set("America/New_York".to_string());
        org.update(&ctx.db).await.unwrap();
        let body = get("/tasks/list?filter=done".into()).await;
        assert!(body.contains("August 2026 · 1") && !body.contains("September 2026"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_done_view_ignores_another_orgs_project_or_person() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let gina = sign_up(&request, "Globex", "gina", "gina@example.com").await;
        let launch = project_in(&ctx, "acme", "Launch").await;
        let secret = project_in(&ctx, "globex", "Secret plan").await;
        done_task(&request, &ctx, &owner, "Ours", launch.id, "", hours_ago(1)).await;
        done_task(&request, &ctx, &gina, "Theirs", secret.id, "", hours_ago(1)).await;
        let gina_user = users::Model::find_by_email(&ctx.db, "gina@example.com")
            .await
            .unwrap();

        for query in [
            format!("project={}", secret.id),
            format!("person={}", gina_user.id),
        ] {
            let body = request
                .get(&format!("/tasks/list?filter=done&{query}"))
                .add_header(owner.0.clone(), owner.1.clone())
                .await
                .text();
            assert!(body.contains("Ours"), "not narrowed: {query}");
            assert!(
                !body.contains("Theirs") && !body.contains("Secret plan") && !body.contains("gina"),
                "{query}"
            );
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_migration_backfills_done_at_from_updated_at() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let launch = project_in(&ctx, "acme", "Launch").await;
        done_task(
            &request,
            &ctx,
            &owner,
            "Finished",
            launch.id,
            "",
            hours_ago(1),
        )
        .await;
        request
            .post("/tasks")
            .add_header(owner.0.clone(), owner.1.clone())
            .form(&task_form("Open", launch.id, ""))
            .await;

        let migrations = migration::Migrator::migrations();
        let done_at = migrations
            .iter()
            .find(|m| m.name() == "m20261012_204623_add_done_at_to_tasks")
            .unwrap();
        let schema = SchemaManager::new(&ctx.db);
        done_at.down(&schema).await.unwrap();
        ctx.db
            .execute_unprepared("UPDATE tasks SET updated_at = '2026-09-01T10:00:00Z'")
            .await
            .unwrap();
        done_at.up(&schema).await.unwrap();

        let finished = task_named(&ctx, "Finished").await;
        assert_eq!(finished.done_at, Some(finished.updated_at));
        assert_eq!(
            finished.done_at.unwrap(),
            DateTime::parse_from_rfc3339("2026-09-01T10:00:00Z").unwrap()
        );
        assert_eq!(task_named(&ctx, "Open").await.done_at, None);
    })
    .await;
}
