use collab::{
    app::App,
    models::{organisations, projects, tasks, users},
};
use loco_rs::{app::AppContext, testing::prelude::*};
use serial_test::serial;

use super::prepare_data::sign_up;

async fn add_task(
    ctx: &AppContext,
    org_id: i64,
    project_id: i64,
    owner: Option<i64>,
    title: &str,
    status: &str,
) {
    let task = tasks::Model::create(
        &ctx.db,
        org_id,
        &tasks::TaskParams {
            title: title.to_string(),
            project_id: project_id.to_string(),
            assignee_ids: owner.into_iter().collect(),
            priority: "medium".to_string(),
            due_on: String::new(),
            status: String::new(),
        },
    )
    .await
    .unwrap();
    task.set_status(&ctx.db, status).await.unwrap();
}

#[tokio::test]
#[serial]
async fn dashboard_numbers_follow_the_tasks() {
    request::<App, _, _>(|request, ctx| async move {
        let cookie = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let org = organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap();
        let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();
        let project = projects::Model::create(
            &ctx.db,
            org.id,
            &projects::ProjectParams {
                name: "Launch".to_string(),
                lane: "now".to_string(),
                status: "Active".to_string(),
                accent: "#72e5b4".to_string(),
                owner_id: alice.id.to_string(),
                summary: String::new(),
            },
        )
        .await
        .unwrap();
        // alice: done + todo -> score 58; plus one unowned blocked task. 1 of 3 done -> 33%.
        add_task(
            &ctx,
            org.id,
            project.id,
            Some(alice.id),
            "Write copy",
            "done",
        )
        .await;
        add_task(
            &ctx,
            org.id,
            project.id,
            Some(alice.id),
            "Book venue",
            "todo",
        )
        .await;
        add_task(&ctx, org.id, project.id, None, "Pick a date", "blocked").await;

        let body = request
            .get("/dashboard")
            .add_header(cookie.0, cookie.1)
            .await
            .text();
        let compact: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            compact.contains(r#"stat-value">1</span><span class="meta">Your open tasks"#),
            "alice has one open task"
        );
        assert!(
            compact.contains(r#"stat-value">1</span><span class="meta">Blocked"#),
            "one blocked task in the org"
        );
        assert!(
            compact.contains("Launch")
                && compact.contains("33%")
                && compact.contains("1 of 3 done"),
            "now-lane progress comes from finished tasks"
        );
        assert!(
            compact.contains("1 blocked"),
            "the project shows its blocked task"
        );
        assert!(
            compact.contains("Book venue"),
            "alice's open task is listed"
        );
        assert!(!compact.contains("Write copy"), "done tasks are not listed");
        assert!(
            !compact.contains("Pick a date"),
            "other people's tasks are not in My tasks"
        );
    })
    .await;
}
