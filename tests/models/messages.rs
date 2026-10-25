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

/// The `messages-own-photo` constraint: a message's own photo has all four
/// columns or none, is always a photo type, and never sits beside a
/// Documents file.
mod own_photo_constraint {
    use collab::{
        app::App,
        models::{
            conversations, documents, folders, messages,
            organisations::{self, SignupParams},
        },
    };
    use loco_rs::{prelude::TenantActiveModelExt, testing::prelude::*};
    use sea_orm::{ActiveModelTrait, ActiveValue};
    use serial_test::serial;

    struct Row {
        key: Option<&'static str>,
        content_type: Option<&'static str>,
        size: Option<i64>,
        name: Option<&'static str>,
        has_photo: bool,
        document_id: Option<i64>,
    }

    const GOOD: Row = Row {
        key: Some("orgs/1/chat/1/abc"),
        content_type: Some("image/png"),
        size: Some(10),
        name: Some("Receipt.png"),
        has_photo: true,
        document_id: None,
    };

    #[tokio::test]
    #[serial]
    async fn own_photos_are_all_or_nothing_and_only_photos() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
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
        let folder = folders::Model::chat_photos(&ctx.db, org.id).await.unwrap();
        let file = documents::Model::upload(
            ctx,
            &folder,
            alice.id,
            "a.png",
            bytes::Bytes::from_static(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"),
        )
        .await
        .unwrap();
        let insert = |row: Row| {
            let db = ctx.db.clone();
            async move {
                messages::ActiveModel {
                    body: ActiveValue::Set(String::new()),
                    conversation_id: ActiveValue::Set(general.id),
                    user_id: ActiveValue::Set(alice.id),
                    has_photo: ActiveValue::Set(row.has_photo),
                    document_id: ActiveValue::Set(row.document_id),
                    photo_key: ActiveValue::Set(row.key.map(str::to_string)),
                    photo_type: ActiveValue::Set(row.content_type.map(str::to_string)),
                    photo_size: ActiveValue::Set(row.size),
                    photo_name: ActiveValue::Set(row.name.map(str::to_string)),
                    ..Default::default()
                }
                .set_tenant(org.id)
                .unwrap()
                .insert(&db)
                .await
            }
        };

        assert!(insert(GOOD).await.is_ok(), "a full own photo");
        let documents_photo = Row {
            key: None,
            content_type: None,
            size: None,
            name: None,
            has_photo: true,
            document_id: Some(file.id),
        };
        assert!(insert(documents_photo).await.is_ok(), "a Documents photo");

        let refused = [
            (
                "a Documents file too",
                Row {
                    document_id: Some(file.id),
                    ..GOOD
                },
            ),
            (
                "SVG",
                Row {
                    content_type: Some("image/svg+xml"),
                    ..GOOD
                },
            ),
            (
                "HTML",
                Row {
                    content_type: Some("text/html"),
                    ..GOOD
                },
            ),
            (
                "no type",
                Row {
                    content_type: None,
                    ..GOOD
                },
            ),
            ("no size", Row { size: None, ..GOOD }),
            ("no name", Row { name: None, ..GOOD }),
            ("no key", Row { key: None, ..GOOD }),
            (
                "only a key",
                Row {
                    content_type: None,
                    size: None,
                    name: None,
                    ..GOOD
                },
            ),
            (
                "empty",
                Row {
                    size: Some(0),
                    ..GOOD
                },
            ),
            (
                "not a photo message",
                Row {
                    has_photo: false,
                    ..GOOD
                },
            ),
        ];
        for (what, row) in refused {
            assert!(insert(row).await.is_err(), "{what} is refused");
        }
    }
}

/// Group and DM photos are stored on the message, outside Documents. Every
/// stored value comes from the server, never from the browser.
mod own_photos {
    use std::path::Path;

    use collab::{
        app::App,
        models::{
            conversations::{self, GroupParams},
            documents, folders, messages,
            organisations::{self, SignupParams},
            users,
        },
    };
    use loco_rs::{
        app::AppContext,
        prelude::{ModelError, TenantActiveModelExt},
        testing::prelude::*,
    };
    use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait, ModelTrait, PaginatorTrait};
    use serial_test::serial;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    /// Acme with alice (the owner), a group and a DM. Who else is in them
    /// doesn't matter to the model; the request tests cover that.
    async fn acme(
        ctx: &AppContext,
    ) -> (
        i64,
        users::Model,
        conversations::Model,
        conversations::Model,
    ) {
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
        let group = conversations::Model::create_group(
            &ctx.db,
            org.id,
            alice.id,
            &GroupParams {
                name: "finance".to_string(),
                member_ids: vec![],
            },
        )
        .await
        .unwrap();
        let dm = conversations::ActiveModel {
            kind: ActiveValue::Set(conversations::kind::DM.to_string()),
            dm_key: ActiveValue::Set(Some(format!("dm:{}:0", alice.id))),
            created_by_id: ActiveValue::Set(Some(alice.id)),
            ..Default::default()
        }
        .set_tenant(org.id)
        .unwrap()
        .insert(&ctx.db)
        .await
        .unwrap();
        (org.id, alice, group, dm)
    }

    /// Every file stored under an organisation's chat area.
    async fn stored_chat_files(ctx: &AppContext, org_id: i64) -> Vec<String> {
        ctx.storage
            .list(Path::new(&format!("orgs/{org_id}/chat")), true)
            .await
            .unwrap()
            .into_iter()
            .filter(|entry| !entry.is_dir)
            .map(|entry| entry.path)
            .collect()
    }

    #[tokio::test]
    #[serial]
    async fn group_and_dm_photos_are_stored_on_the_message() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let (org_id, alice, group, dm) = acme(ctx).await;
        assert!(stored_chat_files(ctx, org_id).await.is_empty());

        for conversation in [&group, &dm] {
            // PNG bytes with an HTML name: the bytes decide the type and the extension.
            let message = messages::Model::create_photo(
                ctx,
                conversation,
                alice.id,
                " Van fuel £62 ",
                "evil.html",
                bytes::Bytes::from_static(PNG),
            )
            .await
            .unwrap();
            assert_eq!(message.body, "Van fuel £62");
            assert!(message.has_photo);
            assert_eq!(message.document_id, None);
            assert_eq!(message.photo_type.as_deref(), Some("image/png"));
            assert_eq!(message.photo_name.as_deref(), Some("evil.png"));
            assert_eq!(message.photo_size, Some(PNG.len() as i64));
            let key = message.photo_key.clone().unwrap();
            assert!(
                key.starts_with(&format!("orgs/{org_id}/chat/{}/", conversation.id)),
                "{key}"
            );
            let stored: Vec<u8> = ctx.storage.download(Path::new(&key)).await.unwrap();
            assert_eq!(stored, PNG);
        }

        // A name with quotes, a line break and a right-to-left override.
        let message = messages::Model::create_photo(
            ctx,
            &group,
            alice.id,
            "",
            "a\"b\r\nc\u{202E}gpj.png",
            bytes::Bytes::from_static(PNG),
        )
        .await
        .unwrap();
        let name = message.photo_name.unwrap();
        assert!(name.ends_with(".png"), "{name}");
        assert!(!name.contains(['\r', '\n', '\u{202E}']), "{name}");

        assert_eq!(documents::Entity::find().count(&ctx.db).await.unwrap(), 0);
        let folders = folders::Entity::find().all(&ctx.db).await.unwrap();
        assert!(!folders.iter().any(|f| f.name == "Chat photos"));
        assert_eq!(stored_chat_files(ctx, org_id).await.len(), 3);
    }

    #[tokio::test]
    #[serial]
    async fn general_photos_still_go_to_documents() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let (org_id, alice, _, _) = acme(ctx).await;
        let general = conversations::Model::find_general(&ctx.db, org_id)
            .await
            .unwrap();
        let message = messages::Model::create_photo(
            ctx,
            &general,
            alice.id,
            "",
            "office.png",
            bytes::Bytes::from_static(PNG),
        )
        .await
        .unwrap();
        assert!(message.document_id.is_some());
        assert_eq!(message.photo_key, None);
        assert!(stored_chat_files(ctx, org_id).await.is_empty());
    }

    #[tokio::test]
    #[serial]
    async fn a_failed_save_deletes_the_stored_photo() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let (org_id, alice, group, _) = acme(ctx).await;
        // A stale model: the group is gone, so the insert fails on its foreign key.
        group.clone().delete(&ctx.db).await.unwrap();
        let result = messages::Model::create_photo(
            ctx,
            &group,
            alice.id,
            "",
            "a.png",
            bytes::Bytes::from_static(PNG),
        )
        .await;
        assert!(result.is_err());
        assert!(stored_chat_files(ctx, org_id).await.is_empty());
        assert_eq!(messages::Entity::find().count(&ctx.db).await.unwrap(), 0);
    }

    #[tokio::test]
    #[serial]
    async fn only_photos_are_stored() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let (org_id, alice, group, _) = acme(ctx).await;
        let refused = [
            ("a.png", b"%PDF-1.7\n".to_vec()),
            (
                "a.svg",
                br#"<svg xmlns="http://www.w3.org/2000/svg"></svg>"#.to_vec(),
            ),
            ("a.png", Vec::new()),
        ];
        for (name, bytes) in refused {
            let result = messages::Model::create_photo(
                ctx,
                &group,
                alice.id,
                "",
                name,
                bytes::Bytes::from(bytes),
            )
            .await;
            assert!(matches!(result, Err(ModelError::Message(_))), "{name}");
        }
        let long = "x".repeat(1001);
        let result = messages::Model::create_photo(
            ctx,
            &group,
            alice.id,
            &long,
            "a.png",
            bytes::Bytes::from_static(PNG),
        )
        .await;
        assert!(result.is_err());
        assert!(stored_chat_files(ctx, org_id).await.is_empty());
        assert_eq!(messages::Entity::find().count(&ctx.db).await.unwrap(), 0);
    }
}
