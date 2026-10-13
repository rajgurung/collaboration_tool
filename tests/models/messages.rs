use collab::{app::App, models::_entities::messages};
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

/// The `Messages` entity matches the table its migration created.
///
/// Selecting every column is the cheapest assertion that says something true:
/// it fails if the migration and the entity disagree — a renamed column, a
/// type that does not round-trip, a migration that never ran — which is the
/// most common way a generated model breaks. Extend it as the model grows.
#[tokio::test]
#[serial]
async fn can_query_messages() {
    configure_insta!();

    let boot = boot_test::<App>().await.unwrap();
    seed::<App>(&boot.app_context).await.unwrap();

    // The query is the assertion. Bind the result and compare it once you have
    // seed data to compare against, e.g.:
    //
    // let items = messages::Entity::find().all(&boot.app_context.db).await.unwrap();
    // assert_debug_snapshot!(items);
    messages::Entity::find()
        .all(&boot.app_context.db)
        .await
        .expect("`messages` should be queryable — entity and migration must agree");
}

mod threads {
    use collab::{
        app::App,
        models::{
            conversation_members::{self, ReadSpan},
            conversations, messages,
            organisations::{self, SignupParams},
            users::{self, RegisterParams},
        },
    };
    use loco_rs::{app::AppContext, testing::prelude::*};
    use serial_test::serial;

    struct Org {
        general: conversations::Model,
        alice: users::Model,
        bob: users::Model,
    }

    /// An organisation whose General has alice (the owner) and bob.
    async fn org(ctx: &AppContext, name: &str) -> Org {
        let slug = name.to_lowercase();
        let (alice, org) = organisations::Model::sign_up(
            &ctx.db,
            &SignupParams {
                organisation_name: name.to_string(),
                name: format!("alice{slug}"),
                email: format!("alice@{slug}.example.com"),
                password: "correct-horse".to_string(),
            },
        )
        .await
        .unwrap();
        let bob = users::Model::create_with_password(
            &ctx.db,
            &RegisterParams {
                email: format!("bob@{slug}.example.com"),
                password: "correct-horse".to_string(),
                name: format!("bob{slug}"),
            },
        )
        .await
        .unwrap();
        let general = conversations::Model::find_general(&ctx.db, org.id)
            .await
            .unwrap();
        conversation_members::Model::add(&ctx.db, &general, bob.id)
            .await
            .unwrap();
        Org {
            general,
            alice,
            bob,
        }
    }

    async fn say(ctx: &AppContext, org: &Org, who: i64, body: &str) -> messages::Model {
        messages::Model::create(
            &ctx.db,
            &org.general,
            who,
            &messages::MessageParams {
                body: body.to_string(),
            },
        )
        .await
        .unwrap()
    }

    async fn answer(
        ctx: &AppContext,
        org: &Org,
        parent: &messages::Model,
        who: i64,
        body: &str,
    ) -> messages::Model {
        messages::Model::create_reply(
            &ctx.db,
            &org.general,
            parent,
            who,
            &messages::MessageParams {
                body: body.to_string(),
            },
        )
        .await
        .unwrap()
    }

    fn bodies(list: &[messages::Model]) -> Vec<&str> {
        list.iter().map(|m| m.body.as_str()).collect()
    }

    #[tokio::test]
    #[serial]
    async fn recent_leaves_out_replies() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let acme = org(ctx, "Acme").await;
        let parent = say(ctx, &acme, acme.alice.id, "parent").await;
        answer(ctx, &acme, &parent, acme.bob.id, "reply").await;
        say(ctx, &acme, acme.bob.id, "after").await;
        let feed = messages::Model::recent(&ctx.db, &acme.general)
            .await
            .unwrap();
        assert_eq!(bodies(&feed), vec!["parent", "after"]);
        let replies = messages::Model::replies(&ctx.db, &parent).await.unwrap();
        assert_eq!(bodies(&replies), vec!["reply"]);
    }

    #[tokio::test]
    #[serial]
    async fn read_in_span_leaves_out_replies() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let acme = org(ctx, "Acme").await;
        let parent = say(ctx, &acme, acme.alice.id, "parent").await;
        say(ctx, &acme, acme.bob.id, "top-level from bob").await;
        answer(ctx, &acme, &parent, acme.bob.id, "reply from bob").await;
        let span = ReadSpan {
            from: None,
            to: (chrono::Utc::now() + chrono::Duration::minutes(1)).into(),
        };
        let read = messages::Model::read_in_span(
            &ctx.db,
            acme.general.organisation_id,
            acme.general.id,
            acme.alice.id,
            &span,
        )
        .await
        .unwrap();
        assert_eq!(bodies(&read), vec!["top-level from bob"]);
    }

    #[tokio::test]
    #[serial]
    async fn latest_per_conversation_ignores_a_newer_reply() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let acme = org(ctx, "Acme").await;
        let parent = say(ctx, &acme, acme.alice.id, "parent").await;
        answer(ctx, &acme, &parent, acme.bob.id, "newer reply").await;
        let latest = messages::Model::latest_per_conversation(
            &ctx.db,
            acme.general.organisation_id,
            &[acme.general.id],
        )
        .await
        .unwrap();
        assert_eq!(latest[&acme.general.id].body, "parent");
    }

    #[tokio::test]
    #[serial]
    async fn unread_counts_leave_out_replies() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let acme = org(ctx, "Acme").await;
        let parent = say(ctx, &acme, acme.alice.id, "parent").await;
        answer(ctx, &acme, &parent, acme.alice.id, "reply").await;
        let counts = conversation_members::Model::unread_counts(
            &ctx.db,
            acme.general.organisation_id,
            acme.bob.id,
        )
        .await
        .unwrap();
        assert_eq!(counts.get(&acme.general.id), Some(&1), "only the parent");
    }

    #[tokio::test]
    #[serial]
    async fn thread_stats_group_by_person_and_stay_in_the_tenant() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let acme = org(ctx, "Acme").await;
        let globex = org(ctx, "Globex").await;
        let parent = say(ctx, &acme, acme.alice.id, "parent").await;
        let quiet = say(ctx, &acme, acme.alice.id, "quiet").await;
        answer(ctx, &acme, &parent, acme.bob.id, "one").await;
        answer(ctx, &acme, &parent, acme.alice.id, "two").await;
        answer(ctx, &acme, &parent, acme.bob.id, "three").await;
        let other = say(ctx, &globex, globex.alice.id, "globex parent").await;
        answer(ctx, &globex, &other, globex.bob.id, "globex reply").await;

        let stats = messages::Model::thread_stats(
            &ctx.db,
            acme.general.organisation_id,
            &[parent.id, quiet.id, other.id],
        )
        .await
        .unwrap();
        assert!(!stats.contains_key(&quiet.id), "no replies");
        assert!(!stats.contains_key(&other.id), "another organisation");
        let people: Vec<(i64, i64)> = stats[&parent.id]
            .iter()
            .map(|s| (s.user_id, s.replies))
            .collect();
        assert_eq!(
            people,
            vec![(acme.bob.id, 2), (acme.alice.id, 1)],
            "in first-reply order"
        );
        assert!(stats[&parent.id][0].last_at >= stats[&parent.id][1].last_at);
    }
}
