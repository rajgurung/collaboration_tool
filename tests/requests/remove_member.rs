//! Owners and admins remove people. Removal ends the person's access to that
//! organisation, not their sessions: it takes them out of its chat, revokes
//! their Claude tokens there, stops its emails and closes its live
//! connections.
use collab::{
    app::App,
    data::{
        live::CloseScope,
        notify_hub::{NotifyEvent, NotifyHub},
    },
    models::{
        access_tokens, conversation_members, conversations, memberships, messages, note_reactions,
        notifications, organisations, reactions, task_notes, tasks, users,
    },
};
use loco_rs::{app::AppContext, testing::prelude::*, TestServer};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, IntoActiveModel, PaginatorTrait,
    QueryFilter,
};
use serial_test::serial;

use super::{
    admin::super_admin_cookie,
    prepare_data::{join, sign_up_member, AsMember},
};

use super::prepare_data::TestMember;

/// Someone who joined Acme and was approved with `role`.
async fn member(
    request: &TestServer,
    ctx: &AppContext,
    name: &str,
    role: &str,
) -> (TestMember, memberships::Model) {
    let email = format!("{name}@example.com");
    let cookie = join(request, ctx, "acme", name, &email).await;
    let user = users::Model::find_by_email(&ctx.db, &email).await.unwrap();
    let pending = memberships::Model::default_for_user(&ctx.db, user.id)
        .await
        .unwrap()
        .unwrap();
    pending.approve(&ctx.db, user.id).await.unwrap();
    let approved = memberships::Model::default_for_user(&ctx.db, user.id)
        .await
        .unwrap()
        .unwrap();
    let mut active = approved.into_active_model();
    active.role = ActiveValue::Set(role.to_string());
    (cookie, active.update(&ctx.db).await.unwrap())
}

async fn remove(
    request: &TestServer,
    cookie: &TestMember,
    target: &memberships::Model,
) -> (u16, String) {
    let res = request
        .post(&format!("/members/{}/remove", target.id))
        .as_member(cookie)
        .add_header("HX-Request", "true")
        .await;
    let toast = res
        .headers()
        .get("HX-Trigger")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    (res.status_code().as_u16(), toast)
}

async fn status_of(ctx: &AppContext, id: i64) -> String {
    memberships::Entity::find_by_id(id)
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap()
        .status
}

#[tokio::test]
#[serial]
async fn removing_someone_locks_them_out_of_everything() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let (carol, carol_m) = member(&request, &ctx, "carol", "member").await;
        let org = organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap();
        let general = conversations::Model::find_general(&ctx.db, org.id)
            .await
            .unwrap();
        access_tokens::Model::create_personal(
            &ctx.db,
            org.id,
            carol_m.user_id,
            &access_tokens::PersonalParams {
                name: "Laptop".to_string(),
            },
            "http://localhost:5150/mcp",
        )
        .await
        .unwrap();
        // A mention carol has not seen yet.
        request
            .post(&format!("/chat/{}/messages", general.id))
            .as_member(&alice)
            .form(&serde_json::json!({ "body": "@carol can you check?" }))
            .await;
        let mut notify = ctx.shared_store.get::<NotifyHub>().unwrap().subscribe();

        let res = request
            .post(&format!("/members/{}/remove", carol_m.id))
            .as_member(&alice)
            .add_header("HX-Request", "true")
            .await;
        let back = format!("/members/{}/rotate?removed=1", carol_m.id);
        assert_eq!(res.headers()["hx-redirect"], back.as_str());
        assert_eq!(status_of(&ctx, carol_m.id).await, "removed");
        let page = request
            .get(&back)
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(
            page.contains("carol has been removed and signed out."),
            "{page}"
        );
        // The name comes from the database: a member id that was not removed shows nothing.
        let alice_m = memberships::Model::default_for_user(&ctx.db, org.created_by_id.unwrap())
            .await
            .unwrap()
            .unwrap();
        let page = request
            .get(&format!("/members?removed={}", alice_m.id))
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(!page.contains("was removed and signed out."));

        // Her open pages in Acme were told to close.
        assert_eq!(
            notify.try_recv().unwrap(),
            NotifyEvent::Close(CloseScope::Org {
                org_id: org.id,
                user_id: carol_m.user_id
            })
        );

        // She stays signed in, and her next page says why, and nothing else.
        let res = request
            .get("/dashboard")
            .add_header(carol.0.clone(), carol.1.clone())
            .await;
        assert_eq!(res.status_code(), 403);
        assert!(res.text().contains("You were removed from Acme"));
        let before = messages::Entity::find().count(&ctx.db).await.unwrap();
        let res = request
            .post(&format!("/chat/{}/messages", general.id))
            .as_member(&carol)
            .form(&serde_json::json!({ "body": "still here?" }))
            .await;
        assert_eq!(res.status_code(), 403);
        assert_eq!(
            messages::Entity::find().count(&ctx.db).await.unwrap(),
            before
        );
        // Nor react, or see who reacted.
        let mention = messages::Entity::find()
            .filter(messages::Column::Body.eq("@carol can you check?"))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        let res = request
            .post(&format!(
                "/chat/{}/messages/{}/reactions",
                general.id, mention.id
            ))
            .as_member(&carol)
            .form(&serde_json::json!({ "emoji": "👍", "on": "1" }))
            .await;
        assert_eq!(res.status_code(), 403);
        let res = request
            .get(&format!(
                "/chat/{}/messages/{}/actions",
                general.id, mention.id
            ))
            .add_header(carol.0.clone(), carol.1.clone())
            .await;
        assert_eq!(res.status_code(), 403);
        assert_eq!(reactions::Entity::find().count(&ctx.db).await.unwrap(), 0);
        // Nor react to a task note.
        let task = tasks::Model::create(
            &ctx.db,
            org.id,
            &tasks::TaskParams {
                title: "Stock count".to_string(),
                project_id: String::new(),
                assignee_ids: vec![],
                priority: "low".to_string(),
                due_on: String::new(),
                status: String::new(),
            },
        )
        .await
        .unwrap();
        let note = task_notes::Model::create(
            &ctx.db,
            &task,
            org.created_by_id.unwrap(),
            &task_notes::NoteParams {
                body: "Shelves done".to_string(),
            },
        )
        .await
        .unwrap();
        let res = request
            .post(&format!("/tasks/{}/notes/{}/reactions", task.id, note.id))
            .as_member(&carol)
            .form(&serde_json::json!({ "emoji": "👍", "on": "1" }))
            .await;
        assert_eq!(res.status_code(), 403);
        assert_eq!(
            note_reactions::Entity::find().count(&ctx.db).await.unwrap(),
            0
        );
        // Nor open, reload or reply in a thread.
        let thread = format!("/chat/{}/threads/{}", general.id, mention.id);
        for path in [
            thread.clone(),
            format!("{thread}/feed"),
            format!("/chat/{}/ws?thread={}", general.id, mention.id),
        ] {
            let res = request
                .get(&path)
                .add_header(carol.0.clone(), carol.1.clone())
                .await;
            assert_eq!(res.status_code(), 403, "GET {path}");
        }
        let res = request
            .post(&format!("{thread}/replies"))
            .as_member(&carol)
            .form(&serde_json::json!({ "body": "still here?" }))
            .await;
        assert_eq!(res.status_code(), 403);
        assert_eq!(
            messages::Entity::find().count(&ctx.db).await.unwrap(),
            before
        );
        let seats = conversation_members::Entity::find()
            .filter(conversation_members::Column::UserId.eq(carol_m.user_id))
            .count(&ctx.db)
            .await
            .unwrap();
        assert_eq!(seats, 0, "out of every conversation");

        // Her Claude token no longer works.
        let tokens = access_tokens::Entity::find()
            .filter(access_tokens::Column::UserId.eq(carol_m.user_id))
            .all(&ctx.db)
            .await
            .unwrap();
        assert!(tokens.iter().all(|t| t.revoked_at.is_some()));

        // Her unseen mention is marked emailed, so it is never sent...
        let hers = notifications::Entity::find()
            .filter(notifications::Column::UserId.eq(carol_m.user_id))
            .all(&ctx.db)
            .await
            .unwrap();
        assert_eq!(hers.len(), 1);
        assert!(hers[0].emailed_at.is_some());
        // ...and the sender skips her even if one were still due.
        notifications::Entity::update_many()
            .col_expr(
                notifications::Column::EmailedAt,
                sea_orm::sea_query::Expr::value(Option::<chrono::DateTime<chrono::Utc>>::None),
            )
            .col_expr(
                notifications::Column::CreatedAt,
                sea_orm::sea_query::Expr::value(chrono::Utc::now() - chrono::Duration::minutes(30)),
            )
            .exec(&ctx.db)
            .await
            .unwrap();
        assert_eq!(collab::mailers::mentions::send_due(&ctx).await.unwrap(), 0);

        // Gone from the member list; owners see her under Removed.
        let page = request
            .get("/members")
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(
            !page.contains("carol@example.com"),
            "not listed as a member"
        );
        assert!(page.contains("Removed · 1"));
        assert!(page.contains("Removed by alice"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn removal_rules() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let alice_m = memberships::Model::default_for_user(
            &ctx.db,
            users::Model::find_by_email(&ctx.db, "alice@example.com")
                .await
                .unwrap()
                .id,
        )
        .await
        .unwrap()
        .unwrap();
        let (bob, bob_m) = member(&request, &ctx, "bob", "admin").await;
        let (dan, dan_m) = member(&request, &ctx, "dan", "member").await;
        let (_, erin_m) = member(&request, &ctx, "erin", "admin").await;

        // Nobody removes themselves.
        let (status, toast) = remove(&request, &alice, &alice_m).await;
        assert_eq!(status, 422);
        assert!(toast.contains("You can't remove yourself."), "{toast}");

        // An admin can't remove an owner, and sees no button for one.
        let (status, toast) = remove(&request, &bob, &alice_m).await;
        assert_eq!(status, 422);
        assert!(
            toast.contains("Only an owner can remove an owner."),
            "{toast}"
        );
        let page = request
            .get("/members")
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(!page.contains(&format!("/members/{}/remove", alice_m.id)));
        assert!(page.contains(&format!("/members/{}/remove", erin_m.id)));

        // A member can't remove anyone.
        let res = request
            .post(&format!("/members/{}/remove", erin_m.id))
            .as_member(&dan)
            .await;
        assert_eq!(res.status_code(), 403);

        // An admin can remove another admin.
        assert_eq!(remove(&request, &bob, &erin_m).await.0, 200);
        assert_eq!(status_of(&ctx, erin_m.id).await, "removed");

        // A second owner can be removed by an owner; then the last one stays.
        let mut promoted = dan_m.clone().into_active_model();
        promoted.role = ActiveValue::Set("owner".to_string());
        let dan_m = promoted.update(&ctx.db).await.unwrap();
        assert!(alice_m.can_remove(dan_m.user_id, "owner", 2));
        assert!(
            !alice_m.can_remove(dan_m.user_id, "owner", 1),
            "the last owner stays"
        );
        assert_eq!(remove(&request, &alice, &dan_m).await.0, 200);
        assert_eq!(status_of(&ctx, dan_m.id).await, "removed");
        let page = request
            .get("/members")
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(!page.contains(&format!("/members/{}/remove", alice_m.id)));

        // Pending requests are declined, not removed.
        join(&request, &ctx, "acme", "fay", "fay@example.com").await;
        let fay = users::Model::find_by_email(&ctx.db, "fay@example.com")
            .await
            .unwrap();
        let fay_m = memberships::Model::default_for_user(&ctx.db, fay.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(remove(&request, &alice, &fay_m).await.0, 422);
        assert_eq!(status_of(&ctx, bob_m.id).await, "active");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_super_admin_acting_inside_cannot_remove_anyone() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let (_, carol_m) = member(&request, &ctx, "carol", "member").await;
        let acme = organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap();
        let admin = super_admin_cookie(&request, &ctx).await;
        let acting = format!("{}; acting_org={}", admin.1.to_str().unwrap(), acme.id);

        let page = request
            .get("/members")
            .add_header("cookie", acting.clone())
            .await
            .text();
        assert!(!page.contains(&format!("/members/{}/remove", carol_m.id)));
        let res = request
            .post(&format!("/members/{}/remove", carol_m.id))
            .add_header("cookie", acting)
            .add_header("x-org-id", acme.id.to_string())
            .await;
        assert_eq!(res.status_code(), 403);
        assert_eq!(status_of(&ctx, carol_m.id).await, "active");
    })
    .await;
}
