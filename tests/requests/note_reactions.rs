//! Emoji reactions on task notes: setting them, the fixed six, who can react,
//! and how the task sheet shows them.
use collab::{
    app::App,
    models::{
        note_reactions, notifications, organisations,
        task_notes::{self, NoteParams},
        tasks::{self, TaskParams},
    },
};
use loco_rs::{app::AppContext, testing::prelude::*, TestServer};
use sea_orm::{EntityTrait, PaginatorTrait};
use serial_test::serial;

use super::{
    admin::super_admin_cookie,
    chat::{acme_id, acme_with_bob, user_id, Cookie},
    prepare_data::{join, sign_up},
};

fn react_path(note: &task_notes::Model) -> String {
    format!("/tasks/{}/notes/{}/reactions", note.task_id, note.id)
}

async fn task_in(ctx: &AppContext, org_id: i64, title: &str) -> tasks::Model {
    tasks::Model::create(
        &ctx.db,
        org_id,
        &TaskParams {
            title: title.to_string(),
            project_id: String::new(),
            assignee_ids: vec![],
            priority: "medium".to_string(),
            due_on: String::new(),
            status: String::new(),
        },
    )
    .await
    .unwrap()
}

async fn note_on(
    ctx: &AppContext,
    task: &tasks::Model,
    author_id: i64,
    body: &str,
) -> task_notes::Model {
    task_notes::Model::create(
        &ctx.db,
        task,
        author_id,
        &NoteParams {
            body: body.to_string(),
        },
    )
    .await
    .unwrap()
}

/// alice and bob in Acme, with a task and bob's note on it.
async fn acme_note(request: &TestServer, ctx: &AppContext) -> (Cookie, Cookie, task_notes::Model) {
    let (alice, bob) = acme_with_bob(request, ctx).await;
    let task = task_in(ctx, acme_id(ctx).await, "Stock count").await;
    let bob_id = user_id(ctx, "bob@example.com").await;
    let note = note_on(ctx, &task, bob_id, "Shelves are counted").await;
    (alice, bob, note)
}

/// Sets `who`'s reaction through the route, as the page does.
async fn react(
    request: &TestServer,
    who: &Cookie,
    note: &task_notes::Model,
    emoji: &str,
    on: &str,
) -> (u16, String) {
    let res = request
        .post(&react_path(note))
        .add_header(who.0.clone(), who.1.clone())
        .add_header("HX-Request", "true")
        .form(&serde_json::json!({ "emoji": emoji, "on": on }))
        .await;
    (res.status_code().as_u16(), res.text())
}

async fn count(ctx: &AppContext) -> u64 {
    note_reactions::Entity::find().count(&ctx.db).await.unwrap()
}

#[tokio::test]
#[serial]
async fn reacting_sets_your_reaction_once() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _bob, note) = acme_note(&request, &ctx).await;

        let (status, html) = react(&request, &alice, &note, "👍", "1").await;
        assert_eq!(status, 200);
        assert!(
            html.contains(&format!(r#"<article class="note" id="note-{}">"#, note.id)),
            "{html}"
        );
        assert!(html.contains(r#"class="react mine""#), "{html}");
        assert!(
            html.contains(r#"<span class="react-count" aria-hidden="true">1</span>"#),
            "{html}"
        );
        assert_eq!(count(&ctx).await, 1);
        react(&request, &alice, &note, "👍", "1").await;
        assert_eq!(count(&ctx).await, 1, "setting it again changes nothing");

        for _ in 0..2 {
            let (status, html) = react(&request, &alice, &note, "👍", "0").await;
            assert_eq!(status, 200);
            assert_eq!(count(&ctx).await, 0);
            assert!(
                html.contains(&format!(
                    r#"<div class="reacts" id="note-reacts-{}" role="group" aria-label="Reactions"></div>"#,
                    note.id
                )),
                "{html}"
            );
            assert!(!html.contains("Who reacted"), "{html}");
        }
        // Reactions on notes tell nobody.
        assert_eq!(
            notifications::Entity::find().count(&ctx.db).await.unwrap(),
            0
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn chips_toggle_and_are_shared() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob, note) = acme_note(&request, &ctx).await;
        react(&request, &bob, &note, "✅", "1").await;
        react(&request, &bob, &note, "👍", "1").await;
        let (_, html) = react(&request, &alice, &note, "👍", "1").await;

        // Tapping your own chip removes it; anyone else's adds yours. The
        // quick bar's buttons say the same.
        assert_eq!(
            html.matches(r#"hx-vals='{"emoji": "👍", "on": "0"}'"#)
                .count(),
            2,
            "{html}"
        );
        assert_eq!(
            html.matches(r#"hx-vals='{"emoji": "✅", "on": "1"}'"#)
                .count(),
            2,
            "{html}"
        );
        assert_eq!(
            html.matches(r#"hx-sync="closest .note:replace""#).count(),
            8,
            "two chips and six picks: {html}"
        );
        assert!(html.contains(r#"aria-label="👍 2, you reacted""#), "{html}");
        assert!(html.contains(r#"aria-label="✅ 1""#), "{html}");
        // Fixed order, whoever reacted first.
        assert!(html.find("👍 2").unwrap() < html.find("✅ 1").unwrap());
        assert!(html.contains("Who reacted"), "{html}");
        assert!(
            html.contains(r#"<span class="names truncate-1">You, bob</span>"#),
            "{html}"
        );

        let (_, bobs) = react(&request, &bob, &note, "✅", "1").await;
        assert!(
            bobs.contains(r#"<span class="names truncate-1">You, alice</span>"#),
            "{bobs}"
        );
        assert!(bobs.contains(r#"aria-label="✅ 1, you reacted""#), "{bobs}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn only_the_six_reactions_are_allowed() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _bob, note) = acme_note(&request, &ctx).await;
        for (emoji, on) in [("😀", "1"), ("❤", "1"), ("", "1"), ("👍", "2"), ("👍", "")] {
            let res = request
                .post(&react_path(&note))
                .add_header(alice.0.clone(), alice.1.clone())
                .add_header("HX-Request", "true")
                .form(&serde_json::json!({ "emoji": emoji, "on": on }))
                .await;
            assert_eq!(res.status_code(), 422, "{emoji:?} {on:?}");
            assert_eq!(res.headers()["HX-Reswap"], "none");
            assert!(res.headers().contains_key("HX-Trigger"));
        }
        assert_eq!(count(&ctx).await, 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn only_members_of_the_notes_organisation_can_react() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _bob, note) = acme_note(&request, &ctx).await;
        let gina = sign_up(&request, "Globex", "gina", "gina@example.com").await;
        let dave = join(&request, "acme", "dave", "dave@example.com").await;

        assert_eq!(react(&request, &gina, &note, "👍", "1").await.0, 404);
        assert_eq!(
            react(&request, &dave, &note, "👍", "1").await.0,
            403,
            "pending"
        );

        // A note id from another Acme task, sent with this task's id.
        let other = task_in(&ctx, acme_id(&ctx).await, "Rota").await;
        let res = request
            .post(&format!("/tasks/{}/notes/{}/reactions", other.id, note.id))
            .add_header(alice.0.clone(), alice.1.clone())
            .form(&serde_json::json!({ "emoji": "👍", "on": "1" }))
            .await;
        assert_eq!(res.status_code(), 404);

        // A super admin acting inside Acme has no membership of their own there.
        let acme = organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap();
        let admin = super_admin_cookie(&request, &ctx).await;
        let acting = format!("{}; acting_org={}", admin.1.to_str().unwrap(), acme.id);
        let res = request
            .post(&react_path(&note))
            .add_header("cookie", acting)
            .form(&serde_json::json!({ "emoji": "👍", "on": "1" }))
            .await;
        assert_eq!(res.status_code(), 403);

        assert_eq!(count(&ctx).await, 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_sheet_shows_note_reactions() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob, note) = acme_note(&request, &ctx).await;
        let alice_id = user_id(&ctx, "alice@example.com").await;
        let task = tasks::Model::find_in_org(&ctx.db, note.organisation_id, note.task_id)
            .await
            .unwrap();
        let quiet = note_on(&ctx, &task, alice_id, "No reactions here").await;
        react(&request, &bob, &note, "🎉", "1").await;

        let sheet = request
            .get(&format!("/tasks/{}", task.id))
            .add_header(alice.0.clone(), alice.1.clone())
            .add_header("HX-Request", "true")
            .await
            .text();
        assert!(sheet.contains(r#"aria-label="🎉 1""#), "{sheet}");
        assert!(
            sheet.contains("<span>bob</span>"),
            "the popover names: {sheet}"
        );
        assert_eq!(sheet.matches("Who reacted").count(), 1, "{sheet}");
        assert!(
            sheet.contains(&format!(r##"data-toggle="#note-who-{}""##, note.id)),
            "{sheet}"
        );
        assert!(
            !sheet.contains(&format!(r#"id="note-who-{}""#, quiet.id)),
            "{sheet}"
        );

        let empty = task_in(&ctx, note.organisation_id, "Nothing yet").await;
        let sheet = request
            .get(&format!("/tasks/{}", empty.id))
            .add_header(alice.0.clone(), alice.1.clone())
            .add_header("HX-Request", "true")
            .await
            .text();
        assert!(sheet.contains("No notes yet."), "{sheet}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn deleting_a_task_removes_its_note_reactions() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob, note) = acme_note(&request, &ctx).await;
        react(&request, &alice, &note, "👍", "1").await;
        react(&request, &bob, &note, "👀", "1").await;
        assert_eq!(count(&ctx).await, 2);
        request
            .post(&format!("/tasks/{}/delete", note.task_id))
            .add_header(alice.0.clone(), alice.1.clone())
            .await;
        assert_eq!(count(&ctx).await, 0);
    })
    .await;
}
