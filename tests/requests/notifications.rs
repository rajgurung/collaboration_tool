use collab::{
    app::App,
    models::{
        conversation_members, conversations, memberships, notifications, organisations, tasks,
        users,
    },
};
use loco_rs::{app::AppContext, testing::prelude::*};
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serial_test::serial;

use super::{
    prepare_data::{join, sign_up_member, AsMember, TestMember},
    tasks::{approved_member, form_body, project_in, task_form},
};

/// (kind, body) of someone's notifications, oldest first.
async fn inbox(ctx: &AppContext, user_id: i64) -> Vec<(String, String)> {
    notifications::Entity::find()
        .filter(notifications::Column::UserId.eq(user_id))
        .order_by_asc(notifications::Column::Id)
        .all(&ctx.db)
        .await
        .unwrap()
        .into_iter()
        .map(|n| (n.kind, n.body))
        .collect()
}

async fn user_id(ctx: &AppContext, email: &str) -> i64 {
    users::Model::find_by_email(&ctx.db, email)
        .await
        .unwrap()
        .id
}

#[tokio::test]
#[serial]
async fn chat_mentions_notify_people_who_can_see_the_conversation() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let (bob, bob_id) = approved_member(&request, &ctx, &alice, "bob", "bob@example.com").await;
        let (_, carol_id) =
            approved_member(&request, &ctx, &alice, "carol", "carol@example.com").await;
        let alice_id = user_id(&ctx, "alice@example.com").await;
        let org = organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap();
        let general = conversations::Model::find_general(&ctx.db, org.id)
            .await
            .unwrap();

        let res = request
            .post(&alice.at(&format!("/chat/{}/messages", general.id)))
            .as_member(&alice)
            .add_header("HX-Request", "true")
            .form(&serde_json::json!({ "body": "Can @bob and @Carol look? cc @alice @nobody" }))
            .await;
        assert!(
            res.text().contains(r#"<span class="mention">@bob</span>"#),
            "mentions are highlighted"
        );
        assert_eq!(
            inbox(&ctx, bob_id).await,
            vec![(
                "mention".to_string(),
                "mentioned you in #general\nCan @bob and @Carol look? cc @alice @nobody"
                    .to_string()
            )]
        );
        assert_eq!(inbox(&ctx, carol_id).await.len(), 1);
        assert_eq!(
            inbox(&ctx, alice_id).await.len(),
            1,
            "tagging yourself is a reminder"
        );
        let own = request
            .get(&alice.at("/notifications"))
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(own.contains("<strong>You</strong> mentioned yourself in #general"));

        // A group bob is not in: mentioning him there tells him nothing.
        request
            .post(&alice.at("/chat/groups"))
            .as_member(&alice)
            .form(&serde_json::json!({ "name": "leads" }))
            .await;
        let group = conversations::Entity::find()
            .filter(conversations::Column::Kind.eq("group"))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        request
            .post(&alice.at(&format!("/chat/{}/messages", group.id)))
            .as_member(&alice)
            .form(&serde_json::json!({ "body": "secret plan for @bob" }))
            .await;
        assert_eq!(inbox(&ctx, bob_id).await.len(), 1);

        let page = request
            .get(&bob.at("/notifications"))
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(page.contains("mentioned you in #general"));
        let badge = request
            .get(&bob.at("/notifications/unread"))
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(badge.contains(">1<"), "{badge}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn everyone_tells_each_active_member_once() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let (_, bob_id) = approved_member(&request, &ctx, &alice, "bob", "bob@example.com").await;
        let (_, carol_id) =
            approved_member(&request, &ctx, &alice, "carol", "carol@example.com").await;
        let (_, dave_id) =
            approved_member(&request, &ctx, &alice, "dave", "dave@example.com").await;
        join(&request, &ctx, "acme", "erin", "erin@example.com").await;
        let erin_id = user_id(&ctx, "erin@example.com").await;
        let alice_id = user_id(&ctx, "alice@example.com").await;
        let org = organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap();
        let general = conversations::Model::find_general(&ctx.db, org.id)
            .await
            .unwrap();

        // Dave is no longer on the team but still has his General row.
        let dave = memberships::Entity::find()
            .filter(memberships::Column::UserId.eq(dave_id))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        memberships::ActiveModel {
            id: ActiveValue::Unchanged(dave.id),
            status: ActiveValue::Set(memberships::status::REJECTED.to_string()),
            ..Default::default()
        }
        .update(&ctx.db)
        .await
        .unwrap();
        assert!(conversation_members::Entity::find()
            .filter(conversation_members::Column::ConversationId.eq(general.id))
            .filter(conversation_members::Column::UserId.eq(dave_id))
            .one(&ctx.db)
            .await
            .unwrap()
            .is_some());

        let page = request
            .get(&alice.at(&format!("/chat/{}", general.id)))
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(
            page.contains(r#"data-mentions="everyone,"#),
            "the picker offers everyone first"
        );
        assert!(
            page.contains(r#"data-everyone-hint="Notify everyone in #general ("#),
            "{page}"
        );

        let res = request
            .post(&alice.at(&format!("/chat/{}/messages", general.id)))
            .as_member(&alice)
            .add_header("HX-Request", "true")
            .form(&serde_json::json!({ "body": "@everyone launch at 3, @bob runs it" }))
            .await;
        assert!(res
            .text()
            .contains(r#"<span class="mention">@everyone</span>"#));

        assert_eq!(
            inbox(&ctx, carol_id).await,
            vec![(
                "mention".to_string(),
                "mentioned everyone in #general\n@everyone launch at 3, @bob runs it".to_string()
            )]
        );
        assert_eq!(
            inbox(&ctx, bob_id).await,
            vec![(
                "mention".to_string(),
                "mentioned you in #general\n@everyone launch at 3, @bob runs it".to_string()
            )],
            "named people get one notice, not two"
        );
        assert!(inbox(&ctx, alice_id).await.is_empty(), "not the sender");
        assert!(inbox(&ctx, dave_id).await.is_empty(), "not off the team");
        assert!(inbox(&ctx, erin_id).await.is_empty(), "not pending");

        // Tagging yourself alongside everyone is still one reminder.
        request
            .post(&alice.at(&format!("/chat/{}/messages", general.id)))
            .as_member(&alice)
            .form(&serde_json::json!({ "body": "@everyone @alice check the doc" }))
            .await;
        assert_eq!(
            inbox(&ctx, alice_id).await,
            vec![(
                "mention".to_string(),
                "mentioned you in #general\n@everyone @alice check the doc".to_string()
            )]
        );
        assert_eq!(inbox(&ctx, carol_id).await.len(), 2);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn everyone_stays_inside_groups_and_dms() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let (_, bob_id) = approved_member(&request, &ctx, &alice, "bob", "bob@example.com").await;
        let (_, carol_id) =
            approved_member(&request, &ctx, &alice, "carol", "carol@example.com").await;

        request
            .post(&alice.at("/chat/groups"))
            .as_member(&alice)
            .content_type("application/x-www-form-urlencoded")
            .bytes(format!("name=leads&member_ids={bob_id}").into())
            .await;
        let group = conversations::Entity::find()
            .filter(conversations::Column::Kind.eq("group"))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        request
            .post(&alice.at(&format!("/chat/{}/messages", group.id)))
            .as_member(&alice)
            .form(&serde_json::json!({ "body": "@everyone standup moved" }))
            .await;
        assert_eq!(
            inbox(&ctx, bob_id).await,
            vec![(
                "mention".to_string(),
                "mentioned everyone in leads\n@everyone standup moved".to_string()
            )]
        );
        assert!(
            inbox(&ctx, carol_id).await.is_empty(),
            "carol is not in the group"
        );

        // In a DM it reaches the one other person; the picker doesn't offer it.
        request
            .post(&alice.at("/chat/dms"))
            .as_member(&alice)
            .form(&serde_json::json!({ "user_id": carol_id }))
            .await;
        let dm = conversations::Entity::find()
            .filter(conversations::Column::Kind.eq("dm"))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        let page = request
            .get(&alice.at(&format!("/chat/{}", dm.id)))
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(page.contains("data-mentions=\"carol,alice\""), "{page}");
        assert!(!page.contains("data-everyone-hint"), "no @everyone in a DM");
        request
            .post(&alice.at(&format!("/chat/{}/messages", dm.id)))
            .as_member(&alice)
            .form(&serde_json::json!({ "body": "@everyone hi" }))
            .await;
        assert_eq!(
            inbox(&ctx, carol_id).await,
            vec![(
                "mention".to_string(),
                "mentioned everyone in a message\n@everyone hi".to_string()
            )]
        );
        assert_eq!(inbox(&ctx, bob_id).await.len(), 1);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn assignments_and_notes_notify_the_right_people() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let (_, bob_id) = approved_member(&request, &ctx, &alice, "bob", "bob@example.com").await;
        let (carol, carol_id) =
            approved_member(&request, &ctx, &alice, "carol", "carol@example.com").await;
        let alice_id = user_id(&ctx, "alice@example.com").await;
        let project = project_in(&ctx, "acme", "Launch").await;

        request
            .post(&alice.at("/tasks"))
            .as_member(&alice)
            .content_type("application/x-www-form-urlencoded")
            .bytes(form_body(&[
                ("title", "Write the FAQ".into()),
                ("project_id", project.id.to_string()),
                ("priority", "high".into()),
                ("assignee_ids", alice_id.to_string()),
                ("assignee_ids", bob_id.to_string()),
            ]))
            .await;
        let task = tasks::Entity::find().one(&ctx.db).await.unwrap().unwrap();
        assert_eq!(
            inbox(&ctx, bob_id).await,
            vec![(
                "assigned".to_string(),
                "assigned you to “Write the FAQ”".to_string()
            )]
        );
        assert!(
            inbox(&ctx, alice_id).await.is_empty(),
            "assigning yourself is not a notification"
        );

        // Adding carol tells carol only; bob was already on it.
        request
            .post(&alice.at(&format!("/tasks/{}", task.id)))
            .as_member(&alice)
            .content_type("application/x-www-form-urlencoded")
            .bytes(form_body(&[
                ("title", "Write the FAQ".into()),
                ("project_id", project.id.to_string()),
                ("priority", "high".into()),
                ("status", "todo".into()),
                ("assignee_ids", alice_id.to_string()),
                ("assignee_ids", bob_id.to_string()),
                ("assignee_ids", carol_id.to_string()),
            ]))
            .await;
        assert_eq!(inbox(&ctx, bob_id).await.len(), 1);
        assert_eq!(inbox(&ctx, carol_id).await.len(), 1);

        // Carol's note mentions alice: alice gets the mention, bob the note, carol nothing new.
        request
            .post(&carol.at(&format!("/tasks/{}/notes", task.id)))
            .as_member(&carol)
            .form(&serde_json::json!({ "body": "Draft is up, @alice please review" }))
            .await;
        assert_eq!(
            inbox(&ctx, alice_id).await,
            vec![(
                "mention".to_string(),
                "mentioned you on “Write the FAQ”\nDraft is up, @alice please review".to_string()
            )]
        );
        assert_eq!(inbox(&ctx, bob_id).await[1].0, "note");
        assert_eq!(inbox(&ctx, carol_id).await.len(), 1);

        let sheet = request
            .get(&carol.at(&format!("/tasks/{}", task.id)))
            .add_header(carol.0.clone(), carol.1.clone())
            .await
            .text();
        assert!(sheet.contains(r#"<span class="mention">@alice</span>"#));
        assert!(
            sheet.contains(r#"data-mentions="everyone,alice,bob,carol""#),
            "picker offers everyone, then the team, with you last"
        );

        // Tagging yourself in a note reads as a reminder to you.
        request
            .post(&carol.at(&format!("/tasks/{}/notes", task.id)))
            .as_member(&carol)
            .form(&serde_json::json!({ "body": "Note to self @carol" }))
            .await;
        let own = request
            .get(&carol.at("/notifications"))
            .add_header(carol.0.clone(), carol.1.clone())
            .await
            .text();
        assert!(own.contains("<strong>You</strong> mentioned yourself on “Write the FAQ”"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn project_owners_hear_about_new_and_blocked_tasks() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let (_, bob_id) = approved_member(&request, &ctx, &alice, "bob", "bob@example.com").await;
        request
            .post(&alice.at("/roadmap/projects"))
            .as_member(&alice)
            .form(&serde_json::json!({
                "name": "Launch", "lane": "now", "status": "In progress", "progress": "40",
                "accent": "#72e5b4", "owner_id": bob_id.to_string(), "summary": "",
            }))
            .await;
        assert_eq!(
            inbox(&ctx, bob_id).await,
            vec![("owner".to_string(), "made you owner of Launch".to_string())]
        );
        let project = collab::models::projects::Entity::find()
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();

        request
            .post(&alice.at("/tasks"))
            .as_member(&alice)
            .form(&task_form("Book the venue", project.id, ""))
            .await;
        let task = tasks::Entity::find().one(&ctx.db).await.unwrap().unwrap();
        for _ in 0..2 {
            request
                .post(&alice.at(&format!("/tasks/{}/status", task.id)))
                .as_member(&alice)
                .form(&serde_json::json!({ "status": "blocked" }))
                .await;
        }
        let bodies: Vec<String> = inbox(&ctx, bob_id).await.into_iter().map(|n| n.1).collect();
        assert_eq!(
            bodies,
            vec![
                "made you owner of Launch",
                "added “Book the venue” to Launch",
                "marked “Book the venue” as blocked in Launch",
            ],
            "blocked twice is one notification"
        );

        // An owner who is also assigned gets one notification, not two.
        request
            .post(&alice.at("/tasks"))
            .as_member(&alice)
            .form(&task_form("Print badges", project.id, &bob_id.to_string()))
            .await;
        let last = inbox(&ctx, bob_id).await;
        assert_eq!(last.len(), 4);
        assert_eq!(last[3].0, "assigned");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn opening_marks_read_and_stays_private() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let (bob, bob_id) = approved_member(&request, &ctx, &alice, "bob", "bob@example.com").await;
        let gina = sign_up_member(&request, "Globex", "gina", "gina@example.com").await;
        let project = project_in(&ctx, "acme", "Launch").await;
        for title in ["One", "Two"] {
            request
                .post(&alice.at("/tasks"))
                .as_member(&alice)
                .form(&task_form(title, project.id, &bob_id.to_string()))
                .await;
        }
        let first = notifications::Entity::find()
            .order_by_asc(notifications::Column::Id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        let task = tasks::Entity::find()
            .order_by_asc(tasks::Column::Id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();

        // Not yours, or another organisation's: not found, and still unread.
        for cookie in [&alice, &gina] {
            let res = request
                .post(&cookie.at(&format!("/notifications/{}/open", first.id)))
                .as_member(cookie)
                .await;
            assert_eq!(res.status_code(), 404);
            let page = request
                .get(&cookie.at("/notifications"))
                .add_header(cookie.0.clone(), cookie.1.clone())
                .await
                .text();
            assert!(!page.contains("assigned you"));
        }

        let res = request
            .post(&bob.at(&format!("/notifications/{}/open", first.id)))
            .as_member(&bob)
            .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(
            res.headers().get("location").unwrap(),
            &bob.at(&format!("/tasks?open={}", task.id))
        );
        let opened = request
            .get(&bob.at(&format!("/tasks?open={}", task.id)))
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(opened.contains(&format!(
            r##"hx-get="/o/acme/tasks/{}" hx-target="#task-sheet-body" hx-trigger="load""##,
            task.id
        )));

        let badge = |cookie: TestMember| {
            let request = &request;
            async move {
                request
                    .get(&cookie.at("/notifications/unread"))
                    .add_header(cookie.0.clone(), cookie.1.clone())
                    .await
                    .text()
            }
        };
        assert!(badge(bob.clone()).await.contains(">1<"));
        request
            .post(&bob.at("/notifications/read"))
            .as_member(&bob)
            .await;
        assert_eq!(badge(bob).await, "");
    })
    .await;
}

/// Makes every notification look `minutes` old.
async fn age_notifications(ctx: &AppContext, minutes: i64) {
    notifications::Entity::update_many()
        .col_expr(
            notifications::Column::CreatedAt,
            sea_orm::sea_query::Expr::value(
                chrono::Utc::now() - chrono::Duration::minutes(minutes),
            ),
        )
        .exec(&ctx.db)
        .await
        .unwrap();
}

#[tokio::test]
#[serial]
async fn unseen_mentions_are_emailed_once() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let (bob, bob_id) = approved_member(&request, &ctx, &alice, "bob", "bob@example.com").await;
        approved_member(&request, &ctx, &alice, "carol", "carol@example.com").await;
        let org = organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap();
        let general = conversations::Model::find_general(&ctx.db, org.id)
            .await
            .unwrap();
        let project = project_in(&ctx, "acme", "Launch").await;
        for body in ["Can @bob and @carol look?", "@everyone one more thing"] {
            request
                .post(&alice.at(&format!("/chat/{}/messages", general.id)))
                .as_member(&alice)
                .form(&serde_json::json!({ "body": body }))
                .await;
        }
        // An assignment is not a mention, so it is never emailed.
        request
            .post(&alice.at("/tasks"))
            .as_member(&alice)
            .form(&task_form(
                "Book the venue",
                project.id,
                &bob_id.to_string(),
            ))
            .await;

        // Too soon: nothing goes out.
        assert_eq!(collab::mailers::mentions::send_due(&ctx).await.unwrap(), 0);

        // Bob reads his mentions; carol hasn't opened either of hers.
        request
            .post(&bob.at("/notifications/read"))
            .as_member(&bob)
            .await;
        age_notifications(&ctx, 20).await;
        // Bob and Carol's approval emails went out already.
        let before = ctx.mailer.as_ref().unwrap().deliveries().count;

        assert_eq!(collab::mailers::mentions::send_due(&ctx).await.unwrap(), 1);
        let deliveries = ctx.mailer.as_ref().unwrap().deliveries();
        assert_eq!(deliveries.count, before + 1, "one email, to carol only");
        let email = deliveries.messages[before].replace("=\r\n", "");
        assert!(email.contains("carol@example.com"));
        assert!(email.contains("2 mentions waiting for you in Acme"));
        assert!(email.contains("alice mentioned everyone in #general"));
        assert!(email.contains(&format!("/chat/{}", general.id)));

        // Never twice.
        assert_eq!(collab::mailers::mentions::send_due(&ctx).await.unwrap(), 0);
    })
    .await;
}

/// Himalayan Ritual in Acme: raj owns it; shusma, kreepa and samir are
/// approved. Returns their cookies and ids, in that order, and General.
async fn team_of_four(
    request: &loco_rs::TestServer,
    ctx: &AppContext,
) -> (Vec<(super::chat::TestMember, i64)>, conversations::Model) {
    let raj = sign_up_member(request, "Acme", "raj", "raj@example.com").await;
    let raj_id = user_id(ctx, "raj@example.com").await;
    let mut people = vec![(raj.clone(), raj_id)];
    for name in ["shusma", "kreepa", "samir"] {
        people
            .push(approved_member(request, ctx, &raj, name, &format!("{name}@example.com")).await);
    }
    let org = organisations::Model::find_by_slug(&ctx.db, "acme")
        .await
        .unwrap();
    let general = conversations::Model::find_general(&ctx.db, org.id)
        .await
        .unwrap();
    (people, general)
}

#[tokio::test]
#[serial]
async fn replies_tell_the_parents_author_and_earlier_repliers_once() {
    request::<App, _, _>(|request, ctx| async move {
        let (people, general) = team_of_four(&request, &ctx).await;
        let [(_, raj), (shusma_c, shusma), (kreepa_c, kreepa), (samir_c, samir)] =
            people.try_into().unwrap();
        let parent = super::chat::post(
            &request,
            &ctx,
            &shusma_c,
            general.id,
            "Supplier sent the new price list",
        )
        .await;
        let reply =
            |who, body| super::chat_threads::reply(&request, who, general.id, parent.id, body);

        reply(&kreepa_c, "Bowls are up 8%").await;
        assert_eq!(
            inbox(&ctx, shusma).await,
            vec![(
                "reply".to_string(),
                "replied to your message in #general\nBowls are up 8%".to_string()
            )]
        );
        for nobody in [raj, kreepa, samir] {
            assert!(inbox(&ctx, nobody).await.is_empty());
        }

        reply(&samir_c, "Fine by me").await;
        assert_eq!(inbox(&ctx, shusma).await.len(), 2);
        assert_eq!(
            inbox(&ctx, shusma).await[1].1,
            "replied to your message in #general\nFine by me"
        );
        assert_eq!(
            inbox(&ctx, kreepa).await,
            vec![(
                "reply".to_string(),
                "replied in a thread in #general\nFine by me".to_string()
            )]
        );
        assert!(
            inbox(&ctx, samir).await.is_empty(),
            "not about his own reply"
        );
        assert!(inbox(&ctx, raj).await.is_empty(), "not in the thread");

        // Shusma answers in her own thread: both repliers hear, she does not.
        reply(&shusma_c, "Thanks both").await;
        assert_eq!(inbox(&ctx, shusma).await.len(), 2);
        assert_eq!(
            inbox(&ctx, kreepa).await[1].1,
            "replied in a thread in #general\nThanks both"
        );
        assert_eq!(
            inbox(&ctx, samir).await,
            vec![(
                "reply".to_string(),
                "replied in a thread in #general\nThanks both".to_string()
            )]
        );

        // The bell shows the green reply icon and opens the thread.
        let page = request
            .get(&shusma_c.at("/notifications"))
            .add_header(shusma_c.0.clone(), shusma_c.1.clone())
            .await
            .text();
        assert!(page.contains("notice-kind notice-reply"), "{page}");
        assert!(page.contains("<strong>kreepa</strong> replied to your message in #general"));
        let notice = notifications::Entity::find()
            .filter(notifications::Column::UserId.eq(shusma))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        let open = request
            .post(&shusma_c.at(&format!("/notifications/{}/open", notice.id)))
            .as_member(&shusma_c)
            .await;
        assert_eq!(
            open.headers()["location"],
            shusma_c
                .at(&format!("/chat/{}/threads/{}", general.id, parent.id))
                .as_str()
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_mention_in_a_reply_wins_over_the_reply_notice() {
    request::<App, _, _>(|request, ctx| async move {
        let (people, general) = team_of_four(&request, &ctx).await;
        let [(_, raj), (shusma_c, shusma), (kreepa_c, kreepa), (samir_c, samir)] =
            people.try_into().unwrap();
        let parent = super::chat::post(&request, &ctx, &shusma_c, general.id, "Price list").await;
        let reply =
            |who, body| super::chat_threads::reply(&request, who, general.id, parent.id, body);
        reply(&kreepa_c, "Looks fine").await;
        let before = inbox(&ctx, shusma).await.len();

        reply(&samir_c, "@shusma @raj can you check the bowls?").await;
        let shusmas = inbox(&ctx, shusma).await;
        assert_eq!(shusmas.len(), before + 1, "told once");
        assert_eq!(
            shusmas.last().unwrap(),
            &(
                "mention".to_string(),
                "mentioned you in a thread in #general\n@shusma @raj can you check the bowls?"
                    .to_string()
            )
        );
        assert_eq!(inbox(&ctx, raj).await.len(), 1);
        assert_eq!(inbox(&ctx, raj).await[0].0, "mention");
        assert_eq!(
            inbox(&ctx, kreepa).await,
            vec![(
                "reply".to_string(),
                "replied in a thread in #general\n@shusma @raj can you check the bowls?"
                    .to_string()
            )]
        );
        assert!(inbox(&ctx, samir).await.is_empty());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn everyone_in_a_reply_tells_each_member_once() {
    request::<App, _, _>(|request, ctx| async move {
        let (people, general) = team_of_four(&request, &ctx).await;
        let [(_, raj), (shusma_c, shusma), (kreepa_c, kreepa), (samir_c, samir)] =
            people.try_into().unwrap();
        let parent = super::chat::post(&request, &ctx, &shusma_c, general.id, "Opening day").await;
        super::chat_threads::reply(&request, &kreepa_c, general.id, parent.id, "Balloons?").await;
        let shusma_before = inbox(&ctx, shusma).await.len();

        super::chat_threads::reply(
            &request,
            &samir_c,
            general.id,
            parent.id,
            "@everyone 9am start",
        )
        .await;
        let everyone = "mentioned everyone in a thread in #general\n@everyone 9am start";
        assert_eq!(
            inbox(&ctx, raj).await,
            vec![("mention".to_string(), everyone.to_string())]
        );
        assert_eq!(inbox(&ctx, shusma).await.len(), shusma_before + 1);
        assert_eq!(inbox(&ctx, shusma).await.last().unwrap().1, everyone);
        assert_eq!(
            inbox(&ctx, kreepa).await,
            vec![("mention".to_string(), everyone.to_string())]
        );
        assert!(
            inbox(&ctx, samir).await.is_empty(),
            "never the reply's author"
        );
    })
    .await;
}

/// Tagging yourself in a reply is still a reminder, as in the channel, but
/// the reply's author never gets a reply notice.
#[tokio::test]
#[serial]
async fn mentioning_yourself_in_a_reply_is_a_reminder_only() {
    request::<App, _, _>(|request, ctx| async move {
        let (people, general) = team_of_four(&request, &ctx).await;
        let [_, (shusma_c, shusma), (kreepa_c, kreepa), _] = people.try_into().unwrap();
        let parent = super::chat::post(&request, &ctx, &shusma_c, general.id, "Rota").await;
        super::chat_threads::reply(
            &request,
            &kreepa_c,
            general.id,
            parent.id,
            "I'll take Monday",
        )
        .await;
        let before = inbox(&ctx, shusma).await.len();

        // Shusma wrote the parent and has replied before, yet hears only her tag.
        super::chat_threads::reply(
            &request,
            &shusma_c,
            general.id,
            parent.id,
            "@shusma book the van",
        )
        .await;
        let new: Vec<(String, String)> = inbox(&ctx, shusma).await.split_off(before);
        assert_eq!(
            new,
            vec![(
                "mention".to_string(),
                "mentioned you in a thread in #general\n@shusma book the van".to_string()
            )]
        );
        let page = request
            .get(&shusma_c.at("/notifications"))
            .add_header(shusma_c.0.clone(), shusma_c.1.clone())
            .await
            .text();
        assert!(
            page.contains("<strong>You</strong> mentioned yourself in a thread in #general"),
            "{page}"
        );
        assert_eq!(inbox(&ctx, kreepa).await.len(), 1, "kreepa hears once");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn reply_notices_stay_inside_groups_and_dms() {
    request::<App, _, _>(|request, ctx| async move {
        let (people, _general) = team_of_four(&request, &ctx).await;
        let [(raj_c, raj), (shusma_c, shusma), (_, kreepa), (samir_c, samir)] =
            people.try_into().unwrap();
        // A group of raj, shusma and samir. Samir starts a thread, then is removed.
        request
            .post(&raj_c.at("/chat/groups"))
            .as_member(&raj_c)
            .content_type("application/x-www-form-urlencoded")
            .bytes(format!("name=shop&member_ids={shusma}&member_ids={samir}").into())
            .await;
        let group = conversations::Entity::find()
            .filter(conversations::Column::Kind.eq("group"))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        let parent =
            super::chat::post(&request, &ctx, &samir_c, group.id, "Float for the till").await;
        let samir_m = memberships::Entity::find()
            .filter(memberships::Column::UserId.eq(samir))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        request
            .post(&raj_c.at(&format!("/members/{}/remove", samir_m.id)))
            .as_member(&raj_c)
            .await;

        super::chat_threads::reply(&request, &shusma_c, group.id, parent.id, "@kreepa £150?").await;
        assert!(inbox(&ctx, kreepa).await.is_empty(), "not in the group");
        assert!(inbox(&ctx, samir).await.is_empty(), "removed");
        assert!(inbox(&ctx, raj).await.is_empty(), "not in the thread");

        // A DM has no place in the wording.
        request
            .post(&raj_c.at("/chat/dms"))
            .as_member(&raj_c)
            .form(&serde_json::json!({ "user_id": shusma }))
            .await;
        let dm = conversations::Entity::find()
            .filter(conversations::Column::Kind.eq("dm"))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        let contract = super::chat::post(&request, &ctx, &shusma_c, dm.id, "Sign by Friday?").await;
        super::chat_threads::reply(&request, &raj_c, dm.id, contract.id, "Signed and scanned")
            .await;
        assert_eq!(
            inbox(&ctx, shusma).await,
            vec![(
                "reply".to_string(),
                "replied to your message\nSigned and scanned".to_string()
            )]
        );
        super::chat_threads::reply(&request, &shusma_c, dm.id, contract.id, "@raj thanks").await;
        assert_eq!(
            inbox(&ctx, raj).await,
            vec![(
                "mention".to_string(),
                "mentioned you in a thread\n@raj thanks".to_string()
            )]
        );
    })
    .await;
}

/// A task in Acme, assigned to `assignees`.
async fn acme_task(ctx: &AppContext, title: &str, assignees: Vec<i64>) -> tasks::Model {
    let org = organisations::Model::find_by_slug(&ctx.db, "acme")
        .await
        .unwrap();
    tasks::Model::create(
        &ctx.db,
        org.id,
        &tasks::TaskParams {
            title: title.to_string(),
            project_id: String::new(),
            assignee_ids: assignees,
            priority: "medium".to_string(),
            due_on: String::new(),
            status: String::new(),
        },
    )
    .await
    .unwrap()
}

async fn add_note(
    request: &loco_rs::TestServer,
    who: &super::chat::TestMember,
    task: &tasks::Model,
    body: &str,
) {
    let res = request
        .post(&who.at(&format!("/tasks/{}/notes", task.id)))
        .as_member(who)
        .add_header("HX-Request", "true")
        .form(&serde_json::json!({ "body": body }))
        .await;
    assert_eq!(res.status_code(), 200, "{}", res.text());
}

fn everyone_on(title: &str, body: &str) -> (String, String) {
    (
        "mention".to_string(),
        format!("mentioned everyone on “{title}”\n{body}"),
    )
}

#[tokio::test]
#[serial]
async fn everyone_in_a_note_tells_the_whole_team_once() {
    request::<App, _, _>(|request, ctx| async move {
        let (people, _) = team_of_four(&request, &ctx).await;
        let [(_, raj), (_, shusma), (_, kreepa), (samir_c, samir)] = people.try_into().unwrap();
        join(&request, &ctx, "acme", "pete", "pete@example.com").await;
        let pete = user_id(&ctx, "pete@example.com").await;
        let task = acme_task(&ctx, "Stock count", vec![]).await;

        add_note(&request, &samir_c, &task, "@everyone stock count Friday").await;
        for id in [raj, shusma, kreepa] {
            assert_eq!(
                inbox(&ctx, id).await,
                vec![everyone_on("Stock count", "@everyone stock count Friday")]
            );
        }
        assert!(inbox(&ctx, samir).await.is_empty(), "not the writer");
        assert!(inbox(&ctx, pete).await.is_empty(), "not a pending member");
        let links: Vec<String> = notifications::Entity::find()
            .all(&ctx.db)
            .await
            .unwrap()
            .into_iter()
            .map(|n| n.link)
            .collect();
        assert_eq!(links.len(), 3);
        assert!(links
            .iter()
            .all(|l| *l == format!("/tasks?open={}", task.id)));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_name_wins_over_everyone_in_a_note() {
    request::<App, _, _>(|request, ctx| async move {
        let (people, _) = team_of_four(&request, &ctx).await;
        let [(_, raj), (_, shusma), (_, kreepa), (samir_c, _)] = people.try_into().unwrap();
        let task = acme_task(&ctx, "Stock count", vec![]).await;

        add_note(&request, &samir_c, &task, "@shusma @everyone counts due").await;
        assert_eq!(
            inbox(&ctx, shusma).await,
            vec![(
                "mention".to_string(),
                "mentioned you on “Stock count”\n@shusma @everyone counts due".to_string()
            )]
        );
        for id in [raj, kreepa] {
            assert_eq!(
                inbox(&ctx, id).await,
                vec![everyone_on("Stock count", "@shusma @everyone counts due")]
            );
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn everyone_in_a_note_replaces_the_comment_notice() {
    request::<App, _, _>(|request, ctx| async move {
        let (people, _) = team_of_four(&request, &ctx).await;
        let [_, _, (_, kreepa), (samir_c, _)] = people.try_into().unwrap();
        let task = acme_task(&ctx, "Stock count", vec![kreepa]).await;

        add_note(&request, &samir_c, &task, "@everyone done").await;
        assert_eq!(
            inbox(&ctx, kreepa).await,
            vec![everyone_on("Stock count", "@everyone done")],
            "the assignee is told once"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn mentioning_yourself_and_everyone_in_a_note() {
    request::<App, _, _>(|request, ctx| async move {
        let (people, _) = team_of_four(&request, &ctx).await;
        let [(_, raj), (_, shusma), (_, kreepa), (samir_c, samir)] = people.try_into().unwrap();
        let task = acme_task(&ctx, "Stock count", vec![]).await;

        add_note(&request, &samir_c, &task, "@samir @everyone check").await;
        let own = inbox(&ctx, samir).await;
        assert_eq!(own.len(), 1, "only the reminder: {own:?}");
        assert!(own[0].1.starts_with("mentioned you on"), "{own:?}");
        for id in [raj, shusma, kreepa] {
            assert_eq!(inbox(&ctx, id).await.len(), 1);
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn an_acting_super_admin_cannot_notify_everyone() {
    request::<App, _, _>(|request, ctx| async move {
        let (people, _) = team_of_four(&request, &ctx).await;
        let [(_, raj), (_, shusma), (_, kreepa), _] = people.try_into().unwrap();
        let task = acme_task(&ctx, "Stock count", vec![kreepa]).await;
        let acme = organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap();
        let admin = super::admin::super_admin_cookie(&request, &ctx).await;
        let acting = format!("{}; acting_org={}", admin.1.to_str().unwrap(), acme.id);

        let res = request
            .post(&format!("/o/acme/tasks/{}/notes", task.id))
            .add_header("cookie", acting)
            .add_header("x-org-id", acme.id.to_string())
            .add_header("HX-Request", "true")
            .form(&serde_json::json!({ "body": "@everyone hi" }))
            .await;
        assert_eq!(res.status_code(), 200);
        // The note box does not offer it either.
        let sheet = res.text();
        assert!(sheet.contains("data-mentions="), "{sheet}");
        assert!(!sheet.contains(r#"data-mentions="everyone"#), "{sheet}");
        assert!(!sheet.contains("data-everyone-hint"), "{sheet}");
        for id in [raj, shusma] {
            assert!(inbox(&ctx, id).await.is_empty());
        }
        let kreepas = inbox(&ctx, kreepa).await;
        assert_eq!(kreepas.len(), 1, "{kreepas:?}");
        assert_eq!(kreepas[0].0, "note", "the assignee still hears of the note");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_note_box_offers_everyone_first() {
    request::<App, _, _>(|request, ctx| async move {
        let (people, _) = team_of_four(&request, &ctx).await;
        let [_, _, _, (samir_c, _)] = people.try_into().unwrap();
        let task = acme_task(&ctx, "Stock count", vec![]).await;
        add_note(&request, &samir_c, &task, "@everyone stock count Friday").await;

        let sheet = request
            .get(&samir_c.at(&format!("/tasks/{}", task.id)))
            .add_header(samir_c.0.clone(), samir_c.1.clone())
            .await
            .text();
        assert!(sheet.contains(r#"data-mentions="everyone,"#), "{sheet}");
        assert!(
            sheet.contains(r#"data-everyone-hint="Notify the whole team (3 people)""#),
            "{sheet}"
        );
        assert!(
            sheet.contains(r#"<span class="mention">@everyone</span>"#),
            "{sheet}"
        );
    })
    .await;
}
