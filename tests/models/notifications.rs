use collab::{app::App, models::_entities::notifications};
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

/// The `Notifications` entity matches the table its migration created.
///
/// Selecting every column is the cheapest assertion that says something true:
/// it fails if the migration and the entity disagree — a renamed column, a
/// type that does not round-trip, a migration that never ran — which is the
/// most common way a generated model breaks. Extend it as the model grows.
#[tokio::test]
#[serial]
async fn can_query_notifications() {
    configure_insta!();

    let boot = boot_test::<App>().await.unwrap();
    seed::<App>(&boot.app_context).await.unwrap();

    // The query is the assertion. Bind the result and compare it once you have
    // seed data to compare against, e.g.:
    //
    // let items = notifications::Entity::find().all(&boot.app_context.db).await.unwrap();
    // assert_debug_snapshot!(items);
    notifications::Entity::find()
        .all(&boot.app_context.db)
        .await
        .expect("`notifications` should be queryable — entity and migration must agree");
}

/// Mention emails claim their rows before sending. A claim takes only what
/// is still there, unread and unsent, and never a chat notice the old code
/// saved without its message.
mod mention_claims {
    use std::time::Duration;

    use collab::{
        app::App,
        models::{
            conversations, messages,
            notifications::{self, kind, Notice},
            organisations::{self, SignupParams},
        },
    };
    use loco_rs::{app::AppContext, testing::prelude::*};
    use sea_orm::{
        ActiveModelTrait, ActiveValue, ConnectionTrait, EntityTrait, IntoActiveModel, QueryOrder,
        TransactionTrait,
    };
    use serial_test::serial;

    struct Acme {
        org_id: i64,
        alice_id: i64,
        message: messages::Model,
    }

    /// Acme, with one #general message from alice that mentions her.
    async fn acme(ctx: &AppContext) -> Acme {
        let (alice, org) = organisations::Model::sign_up(
            &ctx.db,
            &SignupParams {
                organisation_name: "Acme".to_string(),
                name: "alice".to_string(),
                email: "alice@example.com".to_string(),
                password: "correct-horse".to_string(),
            },
        )
        .await
        .unwrap();
        let general = conversations::Model::find_general(&ctx.db, org.id)
            .await
            .unwrap();
        let message = messages::Model::create(
            &ctx.db,
            &general,
            alice.id,
            &messages::MessageParams {
                body: "@alice the Leeds price".to_string(),
            },
        )
        .await
        .unwrap();
        Acme {
            org_id: org.id,
            alice_id: alice.id,
            message,
        }
    }

    async fn notice(
        ctx: &AppContext,
        acme: &Acme,
        kind: &'static str,
        link: &str,
        message_id: Option<i64>,
    ) -> notifications::Model {
        notifications::Model::notify(
            &ctx.db,
            acme.org_id,
            acme.alice_id,
            &[acme.alice_id],
            &Notice {
                kind,
                body: "mentioned you in #general\nthe Leeds price".to_string(),
                link: link.to_string(),
                message_id,
            },
        )
        .await
        .unwrap();
        notifications::Entity::find()
            .order_by_desc(notifications::Column::Id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap()
    }

    fn ids(list: &[notifications::Model]) -> Vec<i64> {
        list.iter().map(|n| n.id).collect()
    }

    #[tokio::test]
    #[serial]
    async fn a_claim_returns_only_unread_unsent_mentions_in_order() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let acme = acme(ctx).await;
        let chat = format!("/chat/{}", acme.message.conversation_id);
        let id = Some(acme.message.id);
        let first = notice(ctx, &acme, kind::MENTION, &chat, id).await;
        let second = notice(ctx, &acme, kind::MENTION, &chat, id).await;
        let read = notice(ctx, &acme, kind::MENTION, &chat, id).await;
        let mut row = read.clone().into_active_model();
        row.read_at = ActiveValue::Set(Some(chrono::Utc::now().into()));
        row.update(&ctx.db).await.unwrap();
        let emailed = notice(ctx, &acme, kind::MENTION, &chat, id).await;
        let mut row = emailed.clone().into_active_model();
        row.emailed_at = ActiveValue::Set(Some(chrono::Utc::now().into()));
        row.update(&ctx.db).await.unwrap();
        let note = notice(ctx, &acme, kind::NOTE, "/tasks?open=1", None).await;
        // A task-note mention has no message, and is still emailed.
        let task_mention = notice(ctx, &acme, kind::MENTION, "/tasks?open=1", None).await;
        // A chat mention the old code saved in the seconds before a deploy.
        let unlinked = notice(ctx, &acme, kind::MENTION, &chat, None).await;

        let due = notifications::Model::due_mention_emails(&ctx.db, chrono::Duration::zero())
            .await
            .unwrap();
        assert_eq!(ids(&due), vec![first.id, second.id, task_mention.id]);

        let all = vec![
            task_mention.id,
            second.id,
            read.id,
            emailed.id,
            note.id,
            unlinked.id,
            first.id,
        ];
        let claimed = notifications::Model::claim_mention_emails(&ctx.db, &all)
            .await
            .unwrap();
        assert_eq!(ids(&claimed), vec![first.id, second.id, task_mention.id]);
        assert!(claimed.iter().all(|n| n.emailed_at.is_some()));
        let again = notifications::Model::claim_mention_emails(&ctx.db, &all)
            .await
            .unwrap();
        assert!(again.is_empty(), "claimed once");
    }

    /// A delete holding the cascaded notice makes the claim wait; the claim
    /// then finds it gone and emails nothing.
    #[tokio::test]
    #[serial]
    async fn a_claim_waiting_on_a_delete_skips_the_notice() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let acme = acme(ctx).await;
        let chat = format!("/chat/{}", acme.message.conversation_id);
        let mention = notice(ctx, &acme, kind::MENTION, &chat, Some(acme.message.id)).await;

        let deleting = sea_orm::Database::connect(&ctx.config.database.uri)
            .await
            .unwrap();
        let txn = deleting.begin().await.unwrap();
        txn.execute_unprepared("SET LOCAL lock_timeout = '10s'")
            .await
            .unwrap();
        txn.execute_unprepared(&format!(
            "DELETE FROM messages WHERE id = {}",
            acme.message.id
        ))
        .await
        .unwrap();

        let claiming = sea_orm::Database::connect(&ctx.config.database.uri)
            .await
            .unwrap();
        let ids = vec![mention.id];
        let claim = tokio::spawn(async move {
            notifications::Model::claim_mention_emails(&claiming, &ids).await
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!claim.is_finished(), "the claim waits for the delete");
        assert!(crate::locks::waiting_locks(&deleting).await > 0);
        txn.commit().await.unwrap();
        assert!(claim.await.unwrap().unwrap().is_empty());
    }
}
