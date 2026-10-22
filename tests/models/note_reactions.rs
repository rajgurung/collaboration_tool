use collab::{
    app::App,
    models::{
        note_reactions,
        organisations::{self, SignupParams},
        task_notes, tasks, users,
    },
};
use loco_rs::{app::AppContext, testing::prelude::*};
use sea_orm::{EntityTrait, PaginatorTrait};
use serial_test::serial;

/// An organisation with its owner and a task. Tasks without a project are chores.
pub async fn org_with_task(
    ctx: &AppContext,
    name: &str,
    owner: &str,
) -> (users::Model, tasks::Model) {
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
    let task = tasks::Model::create(
        &ctx.db,
        org.id,
        &tasks::TaskParams {
            title: format!("{name} stock count"),
            project_id: String::new(),
            assignee_ids: vec![],
            priority: "medium".to_string(),
            due_on: String::new(),
            status: String::new(),
        },
        None,
    )
    .await
    .unwrap();
    (user, task)
}

pub async fn note(
    ctx: &AppContext,
    task: &tasks::Model,
    author_id: i64,
    body: &str,
) -> task_notes::Model {
    task_notes::Model::create(
        &ctx.db,
        task,
        author_id,
        &task_notes::NoteParams {
            body: body.to_string(),
        },
    )
    .await
    .unwrap()
}

async fn count(ctx: &AppContext) -> u64 {
    note_reactions::Entity::find().count(&ctx.db).await.unwrap()
}

#[tokio::test]
#[serial]
async fn setting_on_or_off_twice_is_the_same_as_once() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, task) = org_with_task(ctx, "Acme", "alice").await;
    let note = note(ctx, &task, alice.id, "Counted the shelves").await;

    for _ in 0..2 {
        note_reactions::Model::set(&ctx.db, &note, alice.id, "👍", true)
            .await
            .unwrap();
        assert_eq!(count(ctx).await, 1);
    }
    for _ in 0..2 {
        note_reactions::Model::set(&ctx.db, &note, alice.id, "👍", false)
            .await
            .unwrap();
        assert_eq!(count(ctx).await, 0);
    }
}

#[tokio::test]
#[serial]
async fn only_the_six_reactions_are_allowed() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, task) = org_with_task(ctx, "Acme", "alice").await;
    let note = note(ctx, &task, alice.id, "Counted the shelves").await;

    for emoji in ["😀", "❤", ""] {
        let err = note_reactions::Model::set(&ctx.db, &note, alice.id, emoji, true)
            .await
            .unwrap_err();
        assert!(
            format!("{err:?}").contains("Pick one of the six reactions."),
            "{emoji:?}: {err:?}"
        );
    }
    assert_eq!(count(ctx).await, 0);
}

#[tokio::test]
#[serial]
async fn for_notes_groups_by_note_and_stays_in_the_tenant() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, acme_task) = org_with_task(ctx, "Acme", "alice").await;
    let (gina, globex_task) = org_with_task(ctx, "Globex", "gina").await;
    let first = note(ctx, &acme_task, alice.id, "First").await;
    let second = note(ctx, &acme_task, alice.id, "Second").await;
    let globex = note(ctx, &globex_task, gina.id, "Globex").await;
    for emoji in ["✅", "👍"] {
        note_reactions::Model::set(&ctx.db, &first, alice.id, emoji, true)
            .await
            .unwrap();
    }
    note_reactions::Model::set(&ctx.db, &second, alice.id, "🎉", true)
        .await
        .unwrap();
    note_reactions::Model::set(&ctx.db, &globex, gina.id, "🎉", true)
        .await
        .unwrap();

    let ids = [first.id, second.id, globex.id];
    let acme = note_reactions::Model::for_notes(&ctx.db, acme_task.organisation_id, &ids)
        .await
        .unwrap();
    assert_eq!(acme.len(), 2, "Globex's note is not Acme's");
    let emoji: Vec<&str> = acme[&first.id].iter().map(|r| r.emoji.as_str()).collect();
    assert_eq!(emoji, vec!["✅", "👍"], "oldest first");
    assert_eq!(acme[&second.id].len(), 1);
    assert!(!acme.contains_key(&globex.id));
    assert!(
        note_reactions::Model::for_notes(&ctx.db, acme_task.organisation_id, &[])
            .await
            .unwrap()
            .is_empty()
    );
}
