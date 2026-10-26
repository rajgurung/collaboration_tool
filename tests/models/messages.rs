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

    async fn photo(
        ctx: &AppContext,
        conversation: &conversations::Model,
        user_id: i64,
        caption: &str,
    ) -> messages::Model {
        messages::Model::create_photo(
            ctx,
            conversation,
            user_id,
            caption,
            "receipt.png",
            bytes::Bytes::from_static(PNG),
        )
        .await
        .unwrap()
    }

    async fn reload(ctx: &AppContext, message: &messages::Model) -> messages::Model {
        messages::Entity::find_by_id(message.id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap()
    }

    fn gone(stored: Result<Vec<u8>, loco_rs::storage::StorageError>) -> bool {
        stored.is_err()
    }

    #[tokio::test]
    #[serial]
    async fn deleting_a_group_photo_removes_the_bytes_then_the_columns() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let (org_id, alice, group, dm) = acme(ctx).await;
        for conversation in [&group, &dm] {
            let message = photo(ctx, conversation, alice.id, "Van fuel £62").await;
            let key = message.photo_key.clone().unwrap();
            let (after, changed) = message.clone().delete_photo(ctx).await.unwrap();
            assert!(changed);
            assert!(gone(ctx.storage.download(Path::new(&key)).await), "{key}");
            // Saved, so the messages-own-photo CHECK accepted the row.
            let saved = reload(ctx, &message).await;
            for row in [&after, &saved] {
                assert_eq!(row.photo_key, None);
                assert_eq!(row.photo_type, None);
                assert_eq!(row.photo_size, None);
                assert_eq!(row.photo_name, None);
                assert_eq!(row.document_id, None);
                assert!(row.has_photo);
                assert!(row.photo_removed());
                assert_eq!(row.body, "Van fuel £62");
            }
        }
        assert!(stored_chat_files(ctx, org_id).await.is_empty());
    }

    #[tokio::test]
    #[serial]
    async fn deleting_a_general_photo_deletes_its_documents_file() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let (org_id, alice, _, _) = acme(ctx).await;
        let general = conversations::Model::find_general(&ctx.db, org_id)
            .await
            .unwrap();
        let message = photo(ctx, &general, alice.id, "Whiteboard").await;
        let file = documents::Model::find_in_org(&ctx.db, org_id, message.document_id.unwrap())
            .await
            .unwrap();
        let (after, changed) = message.clone().delete_photo(ctx).await.unwrap();
        assert!(changed);
        assert_eq!(after.document_id, None);
        assert!(after.photo_removed());
        assert!(matches!(
            documents::Model::find_in_org(&ctx.db, org_id, file.id).await,
            Err(ModelError::EntityNotFound)
        ));
        assert!(gone(
            ctx.storage.download(Path::new(&file.storage_key)).await
        ));
        let saved = reload(ctx, &message).await;
        assert_eq!(saved.document_id, None, "the foreign key cleared it");
        assert!(saved.has_photo);
        assert_eq!(saved.body, "Whiteboard");
    }

    #[tokio::test]
    #[serial]
    async fn deleting_twice_is_a_no_op() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let (org_id, alice, group, _) = acme(ctx).await;
        let general = conversations::Model::find_general(&ctx.db, org_id)
            .await
            .unwrap();
        for conversation in [&group, &general] {
            let message = photo(ctx, conversation, alice.id, "").await;
            let (first, changed) = message.delete_photo(ctx).await.unwrap();
            assert!(changed);
            let saved = reload(ctx, &first).await;
            let (second, changed) = first.clone().delete_photo(ctx).await.unwrap();
            assert!(!changed, "{}", conversation.kind);
            assert_eq!(reload(ctx, &first).await, saved, "nothing written");
            assert_eq!(second.photo_key, None);
            assert_eq!(second.document_id, None);
        }

        // The memory driver, like OpenDAL's S3, deletes a missing key without
        // an error: the retry after a failed update depends on it.
        ctx.storage
            .delete(Path::new(&format!("orgs/{org_id}/chat/never-stored")))
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn two_deletes_at_once_both_succeed() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let (org_id, alice, group, _) = acme(ctx).await;
        let general = conversations::Model::find_general(&ctx.db, org_id)
            .await
            .unwrap();
        for conversation in [&group, &general] {
            let message = photo(ctx, conversation, alice.id, "").await;
            let (a, b) = tokio::join!(
                message.clone().delete_photo(ctx),
                message.clone().delete_photo(ctx),
            );
            a.unwrap();
            b.unwrap();
            let saved = reload(ctx, &message).await;
            assert!(saved.photo_removed(), "{}", conversation.kind);
        }
        assert!(stored_chat_files(ctx, org_id).await.is_empty());
        assert_eq!(documents::Entity::find().count(&ctx.db).await.unwrap(), 0);
    }

    #[tokio::test]
    #[serial]
    async fn a_text_message_has_no_photo_to_delete() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let (org_id, alice, group, _) = acme(ctx).await;
        let kept = photo(ctx, &group, alice.id, "").await;
        let text = messages::Model::create(
            &ctx.db,
            &group,
            alice.id,
            &messages::MessageParams {
                body: "Morning all".to_string(),
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            text.clone().delete_photo(ctx).await,
            Err(ModelError::EntityNotFound)
        ));
        assert_eq!(reload(ctx, &text).await, text);
        assert_eq!(reload(ctx, &kept).await, kept);
        assert_eq!(stored_chat_files(ctx, org_id).await.len(), 1);
    }
}

/// Deleting a message for good. The row goes, and the database takes its
/// reactions, its notices and, for a thread parent, its replies. Saves that
/// race a delete wait on the message's row lock, so whichever comes second
/// sees the other's result.
mod deleting {
    use std::{path::Path, time::Duration};

    use collab::{
        app::App,
        controllers::notifications::message_sent,
        models::{
            conversation_members,
            conversations::{self, GroupParams},
            documents, messages, notifications,
            organisations::{self, SignupParams},
            reactions,
            users::{self, RegisterParams},
        },
    };
    use loco_rs::{app::AppContext, prelude::ModelError, testing::prelude::*};
    use sea_orm::{
        ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseConnection,
        EntityTrait, IntoActiveModel, PaginatorTrait, QueryFilter,
    };
    use serial_test::serial;

    use crate::locks::{hold, waiting_locks};

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    /// Acme: alice (the owner) and bob, both in #general and a group.
    struct Acme {
        org_id: i64,
        alice: users::Model,
        bob: users::Model,
        general: conversations::Model,
        group: conversations::Model,
    }

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
        let bob = users::Model::create_with_password(
            &ctx.db,
            &RegisterParams {
                email: "bob@example.com".to_string(),
                password: "correct-horse".to_string(),
                name: "bob".to_string(),
            },
        )
        .await
        .unwrap();
        let general = conversations::Model::find_general(&ctx.db, org.id)
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
        for conversation in [&general, &group] {
            conversation_members::Model::add(&ctx.db, conversation, bob.id)
                .await
                .unwrap();
        }
        Acme {
            org_id: org.id,
            alice,
            bob,
            general,
            group,
        }
    }

    async fn say(
        ctx: &AppContext,
        conversation: &conversations::Model,
        who: i64,
        body: &str,
    ) -> messages::Model {
        messages::Model::create(
            &ctx.db,
            conversation,
            who,
            &messages::MessageParams {
                body: body.to_string(),
            },
        )
        .await
        .unwrap()
    }

    async fn answer(
        db: &DatabaseConnection,
        conversation: &conversations::Model,
        parent: &messages::Model,
        who: i64,
        body: &str,
    ) -> Result<messages::Model, ModelError> {
        messages::Model::create_reply(
            db,
            conversation,
            parent,
            who,
            &messages::MessageParams {
                body: body.to_string(),
            },
        )
        .await
    }

    async fn photo(
        ctx: &AppContext,
        conversation: &conversations::Model,
        who: i64,
    ) -> messages::Model {
        messages::Model::create_photo(
            ctx,
            conversation,
            who,
            "Courier invoice from Tuesday, £18.40",
            "invoice.png",
            bytes::Bytes::from_static(PNG),
        )
        .await
        .unwrap()
    }

    async fn find(ctx: &AppContext, id: i64) -> Option<messages::Model> {
        messages::Entity::find_by_id(id).one(&ctx.db).await.unwrap()
    }

    async fn notices_about(ctx: &AppContext, ids: &[i64]) -> u64 {
        notifications::Entity::find()
            .filter(notifications::Column::MessageId.is_in(ids.to_vec()))
            .count(&ctx.db)
            .await
            .unwrap()
    }

    async fn reactions_on(ctx: &AppContext, ids: &[i64]) -> u64 {
        reactions::Entity::find()
            .filter(reactions::Column::MessageId.is_in(ids.to_vec()))
            .count(&ctx.db)
            .await
            .unwrap()
    }

    /// The spawned side has not finished, and Postgres says it is waiting.
    async fn assert_waiting<T>(task: &tokio::task::JoinHandle<T>, conn: &DatabaseConnection) {
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!task.is_finished(), "it waits for the lock");
        assert!(waiting_locks(conn).await > 0);
    }

    #[tokio::test]
    #[serial]
    async fn deleting_removes_the_row_reactions_and_notices() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let acme = acme(ctx).await;
        let message = say(
            ctx,
            &acme.general,
            acme.bob.id,
            "@alice I quoted Leeds £4.20 a tin by mistake",
        )
        .await;
        let kept = say(ctx, &acme.general, acme.bob.id, "Morning all").await;
        message_sent(ctx, acme.org_id, &acme.general, &message)
            .await
            .unwrap();
        assert_eq!(notices_about(ctx, &[message.id]).await, 1);
        reactions::Model::set(&ctx.db, &message, acme.alice.id, "👀", true)
            .await
            .unwrap();
        reactions::Model::set(&ctx.db, &kept, acme.alice.id, "👍", true)
            .await
            .unwrap();

        let deleted = message.clone().delete_message(ctx, false).await.unwrap();
        assert_eq!(deleted.replies, 0);
        assert_eq!(deleted.notified, vec![acme.alice.id]);
        assert_eq!(find(ctx, message.id).await, None);
        assert_eq!(reactions_on(ctx, &[message.id]).await, 0);
        assert_eq!(notices_about(ctx, &[message.id]).await, 0);
        assert_eq!(find(ctx, kept.id).await, Some(kept.clone()));
        assert_eq!(reactions_on(ctx, &[kept.id]).await, 1);

        assert!(matches!(
            message.delete_message(ctx, true).await,
            Err(ModelError::EntityNotFound)
        ));
    }

    #[tokio::test]
    #[serial]
    async fn deleting_a_parent_cascades_its_replies() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let acme = acme(ctx).await;
        let parent = say(
            ctx,
            &acme.general,
            acme.bob.id,
            "Who can do the stall from 7 to 10 on Saturday?",
        )
        .await;
        let mut ids = vec![parent.id];
        for body in ["I can do 7 to 8", "@bob me after 8", "Same"] {
            let reply = answer(&ctx.db, &acme.general, &parent, acme.alice.id, body)
                .await
                .unwrap();
            reactions::Model::set(&ctx.db, &reply, acme.bob.id, "✅", true)
                .await
                .unwrap();
            notifications::Model::notify(
                &ctx.db,
                acme.org_id,
                acme.alice.id,
                &[acme.bob.id],
                &notifications::Notice {
                    kind: notifications::kind::REPLY,
                    body: "replied to your message\nSame".to_string(),
                    link: format!("/chat/{}/threads/{}", acme.general.id, parent.id),
                    message_id: Some(reply.id),
                },
            )
            .await
            .unwrap();
            ids.push(reply.id);
        }
        let other = say(ctx, &acme.general, acme.bob.id, "Van booked").await;

        let deleted = parent.delete_message(ctx, true).await.unwrap();
        assert_eq!(deleted.replies, 3);
        assert_eq!(deleted.notified, vec![acme.bob.id]);
        for id in &ids {
            assert_eq!(find(ctx, *id).await, None);
        }
        assert_eq!(reactions_on(ctx, &ids).await, 0);
        assert_eq!(notices_about(ctx, &ids).await, 0);
        assert!(find(ctx, other.id).await.is_some());
    }

    #[tokio::test]
    #[serial]
    async fn the_sender_cannot_delete_a_message_with_replies() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let acme = acme(ctx).await;
        let parent = photo(ctx, &acme.group, acme.bob.id).await;
        answer(&ctx.db, &acme.group, &parent, acme.alice.id, "Paid")
            .await
            .unwrap();
        let key = parent.photo_key.clone().unwrap();

        let err = parent.clone().delete_message(ctx, false).await.unwrap_err();
        assert!(
            matches!(&err, ModelError::Message(text) if text == messages::HAS_REPLIES),
            "{err:?}"
        );
        assert_eq!(
            find(ctx, parent.id).await,
            Some(parent.clone()),
            "the photo too"
        );
        assert!(ctx
            .storage
            .download::<Vec<u8>>(Path::new(&key))
            .await
            .is_ok());
    }

    #[tokio::test]
    #[serial]
    async fn deleting_a_group_photo_message_removes_the_bytes() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let acme = acme(ctx).await;
        let message = photo(ctx, &acme.group, acme.bob.id).await;
        let key = message.photo_key.clone().unwrap();
        message.clone().delete_message(ctx, false).await.unwrap();
        assert!(ctx
            .storage
            .download::<Vec<u8>>(Path::new(&key))
            .await
            .is_err());
        assert_eq!(find(ctx, message.id).await, None);
    }

    #[tokio::test]
    #[serial]
    async fn deleting_a_general_photo_message_deletes_its_documents_file() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let acme = acme(ctx).await;
        let message = photo(ctx, &acme.general, acme.bob.id).await;
        let file =
            documents::Model::find_in_org(&ctx.db, acme.org_id, message.document_id.unwrap())
                .await
                .unwrap();
        message.clone().delete_message(ctx, false).await.unwrap();
        assert!(matches!(
            documents::Model::find_in_org(&ctx.db, acme.org_id, file.id).await,
            Err(ModelError::EntityNotFound)
        ));
        assert!(ctx
            .storage
            .download::<Vec<u8>>(Path::new(&file.storage_key))
            .await
            .is_err());
        assert_eq!(find(ctx, message.id).await, None);
    }

    #[tokio::test]
    #[serial]
    async fn a_general_file_that_is_no_longer_a_photo_is_left_in_documents() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let acme = acme(ctx).await;
        let message = photo(ctx, &acme.general, acme.bob.id).await;
        let file =
            documents::Model::find_in_org(&ctx.db, acme.org_id, message.document_id.unwrap())
                .await
                .unwrap();
        let mut row = file.clone().into_active_model();
        row.content_type = ActiveValue::Set("application/pdf".to_string());
        row.update(&ctx.db).await.unwrap();

        message.clone().delete_message(ctx, false).await.unwrap();
        assert_eq!(find(ctx, message.id).await, None);
        assert!(documents::Model::find_in_org(&ctx.db, acme.org_id, file.id)
            .await
            .is_ok());
        assert!(ctx
            .storage
            .download::<Vec<u8>>(Path::new(&file.storage_key))
            .await
            .is_ok());
    }

    /// The photo goes first; if the transaction then fails, the message
    /// stays as "Photo removed" with its caption, and a retry finishes.
    #[tokio::test]
    #[serial]
    async fn a_failed_transaction_after_the_photo_keeps_the_message() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let acme = acme(ctx).await;
        let message = photo(ctx, &acme.group, acme.bob.id).await;
        let key = message.photo_key.clone().unwrap();
        // KEY SHARE lets the photo's update through and stops the delete's
        // FOR UPDATE, as a reaction's foreign key check would.
        let (_conn, txn) = hold(ctx, message.id, "FOR KEY SHARE").await;

        let err = message
            .clone()
            .delete_message(ctx, false)
            .await
            .unwrap_err();
        assert!(format!("{err:?}").contains("lock timeout"), "{err:?}");
        assert!(ctx
            .storage
            .download::<Vec<u8>>(Path::new(&key))
            .await
            .is_err());
        let kept = find(ctx, message.id).await.unwrap();
        assert!(kept.photo_removed());
        assert_eq!(kept.body, "Courier invoice from Tuesday, £18.40");

        txn.rollback().await.unwrap();
        kept.delete_message(ctx, false).await.unwrap();
        assert_eq!(find(ctx, message.id).await, None);
    }

    #[tokio::test]
    #[serial]
    async fn a_reply_waiting_on_a_delete_is_refused() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let acme = acme(ctx).await;
        let parent = say(ctx, &acme.general, acme.bob.id, "Who can do the stall?").await;
        let (conn, txn) = hold(ctx, parent.id, "FOR UPDATE").await;
        txn.execute_unprepared(&format!("DELETE FROM messages WHERE id = {}", parent.id))
            .await
            .unwrap();

        let (db, general, waiting_parent, alice) = (
            ctx.db.clone(),
            acme.general.clone(),
            parent.clone(),
            acme.alice.id,
        );
        let reply =
            tokio::spawn(
                async move { answer(&db, &general, &waiting_parent, alice, "I can").await },
            );
        assert_waiting(&reply, &conn).await;
        txn.commit().await.unwrap();
        assert!(matches!(
            reply.await.unwrap(),
            Err(ModelError::EntityNotFound)
        ));
        assert_eq!(messages::Entity::find().count(&ctx.db).await.unwrap(), 0);
    }

    #[tokio::test]
    #[serial]
    async fn a_reply_in_flight_stops_the_senders_delete() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let acme = acme(ctx).await;
        let parent = say(ctx, &acme.general, acme.bob.id, "Who can do the stall?").await;
        // What create_reply does, held open: the parent locked FOR SHARE and
        // the reply inserted but not committed.
        let (conn, txn) = hold(ctx, parent.id, "FOR SHARE").await;
        messages::ActiveModel {
            body: ActiveValue::Set("I can".to_string()),
            conversation_id: ActiveValue::Set(acme.general.id),
            user_id: ActiveValue::Set(acme.alice.id),
            parent_id: ActiveValue::Set(Some(parent.id)),
            organisation_id: ActiveValue::Set(acme.org_id),
            ..Default::default()
        }
        .insert(&txn)
        .await
        .unwrap();

        let (task_ctx, sent) = (ctx.clone(), parent.clone());
        let delete = tokio::spawn(async move { sent.delete_message(&task_ctx, false).await });
        assert_waiting(&delete, &conn).await;
        txn.commit().await.unwrap();
        let err = delete.await.unwrap().unwrap_err();
        assert!(
            matches!(&err, ModelError::Message(text) if text == messages::HAS_REPLIES),
            "{err:?}"
        );
        assert!(find(ctx, parent.id).await.is_some());
    }

    #[tokio::test]
    #[serial]
    async fn a_reaction_waiting_on_a_delete_is_refused() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let acme = acme(ctx).await;
        let message = say(ctx, &acme.general, acme.bob.id, "Leeds £4.20").await;
        let (conn, txn) = hold(ctx, message.id, "FOR UPDATE").await;
        txn.execute_unprepared(&format!("DELETE FROM messages WHERE id = {}", message.id))
            .await
            .unwrap();

        let (db, reacted, alice) = (ctx.db.clone(), message.clone(), acme.alice.id);
        let reaction =
            tokio::spawn(
                async move { reactions::Model::set(&db, &reacted, alice, "👍", true).await },
            );
        assert_waiting(&reaction, &conn).await;
        txn.commit().await.unwrap();
        assert!(matches!(
            reaction.await.unwrap(),
            Err(ModelError::EntityNotFound)
        ));
        assert_eq!(reactions::Entity::find().count(&ctx.db).await.unwrap(), 0);
    }

    #[tokio::test]
    #[serial]
    async fn a_notice_waiting_on_a_delete_is_skipped() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let acme = acme(ctx).await;
        let message = say(ctx, &acme.general, acme.bob.id, "@alice Leeds £4.20").await;
        let (conn, txn) = hold(ctx, message.id, "FOR UPDATE").await;
        txn.execute_unprepared(&format!("DELETE FROM messages WHERE id = {}", message.id))
            .await
            .unwrap();

        let (task_ctx, general, sent, org_id) = (
            ctx.clone(),
            acme.general.clone(),
            message.clone(),
            acme.org_id,
        );
        let notice =
            tokio::spawn(async move { message_sent(&task_ctx, org_id, &general, &sent).await });
        assert_waiting(&notice, &conn).await;
        txn.commit().await.unwrap();
        notice.await.unwrap().unwrap();
        assert_eq!(
            notifications::Entity::find().count(&ctx.db).await.unwrap(),
            0
        );
    }

    /// A notice and a reaction saving on a reply hold FOR SHARE on that
    /// reply. Its parent's delete waits for them, then takes them with it,
    /// and counts the notice's reader among the badges to refresh.
    #[tokio::test]
    #[serial]
    async fn saves_on_a_reply_during_its_parents_delete_go_with_it() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let acme = acme(ctx).await;
        let parent = say(ctx, &acme.general, acme.bob.id, "Who can do the stall?").await;
        let reply = answer(&ctx.db, &acme.general, &parent, acme.alice.id, "I can")
            .await
            .unwrap();
        // What reply_sent and reactions::set do, held open.
        let (conn, txn) = hold(ctx, reply.id, "FOR SHARE").await;
        notifications::Model::notify(
            &txn,
            acme.org_id,
            acme.alice.id,
            &[acme.bob.id],
            &notifications::Notice {
                kind: notifications::kind::REPLY,
                body: "replied to your message\nI can".to_string(),
                link: format!("/chat/{}/threads/{}", acme.general.id, parent.id),
                message_id: Some(reply.id),
            },
        )
        .await
        .unwrap();
        reactions::ActiveModel {
            emoji: ActiveValue::Set("👍".to_string()),
            message_id: ActiveValue::Set(reply.id),
            user_id: ActiveValue::Set(acme.bob.id),
            organisation_id: ActiveValue::Set(acme.org_id),
            ..Default::default()
        }
        .insert(&txn)
        .await
        .unwrap();

        let (task_ctx, deleting) = (ctx.clone(), parent.clone());
        let delete = tokio::spawn(async move { deleting.delete_message(&task_ctx, true).await });
        assert_waiting(&delete, &conn).await;
        txn.commit().await.unwrap();
        let deleted = delete.await.unwrap().unwrap();
        assert_eq!(deleted.replies, 1);
        assert_eq!(deleted.notified, vec![acme.bob.id], "the badge to refresh");
        assert_eq!(find(ctx, reply.id).await, None);
        assert_eq!(notices_about(ctx, &[reply.id]).await, 0);
        assert_eq!(reactions_on(ctx, &[reply.id]).await, 0);
        assert_eq!(
            notifications::Entity::find().count(&ctx.db).await.unwrap(),
            0
        );
    }

    /// If a reply ever carried a photo, the cascade would delete its row
    /// and leave its stored bytes behind, so the delete refuses.
    #[tokio::test]
    #[serial]
    async fn a_reply_with_a_photo_stops_the_delete() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let acme = acme(ctx).await;
        let parent = say(ctx, &acme.general, acme.bob.id, "Who can do the stall?").await;
        let reply = answer(&ctx.db, &acme.general, &parent, acme.alice.id, "I can")
            .await
            .unwrap();
        ctx.db
            .execute_unprepared(&format!(
                "UPDATE messages SET has_photo = true WHERE id = {}",
                reply.id
            ))
            .await
            .unwrap();
        let err = parent.clone().delete_message(ctx, true).await.unwrap_err();
        assert!(
            matches!(&err, ModelError::Message(text) if text == messages::REPLY_HAS_PHOTO),
            "{err:?}"
        );
        assert!(find(ctx, parent.id).await.is_some());
        assert!(find(ctx, reply.id).await.is_some());
    }

    /// The cascade deletes replies without looking for photos: only a
    /// top-level message can have one.
    #[tokio::test]
    #[serial]
    async fn replies_never_carry_photos() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let acme = acme(ctx).await;
        let parent = photo(ctx, &acme.group, acme.bob.id).await;
        let reply = answer(&ctx.db, &acme.group, &parent, acme.alice.id, "Paid")
            .await
            .unwrap();
        assert!(!reply.has_photo);
        assert_eq!(reply.document_id, None);
        assert_eq!(reply.photo_key, None);
        assert_eq!(reply.photo_type, None);
        assert_eq!(reply.photo_size, None);
        assert_eq!(reply.photo_name, None);
    }
}
