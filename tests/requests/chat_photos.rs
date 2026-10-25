//! Photos in chat, in every conversation. #general photos are stored in the
//! "Chat photos" folder in Documents, which every member can open. Group and
//! DM photos are stored on the message and open only for the people in that
//! conversation, right now. Every photo is served from
//! `/chat/{conversation}/photos/{message}`.
use std::path::Path;

use axum_test::multipart::{MultipartForm, Part};
use collab::{
    app::App,
    data::chat_hub::{ChatEvent, ChatHub},
    models::{
        conversations::{self, GroupParams},
        documents, folders, memberships, messages, organisations,
    },
};
use loco_rs::{app::AppContext, testing::prelude::*, TestServer};
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait, IntoActiveModel, PaginatorTrait};
use serial_test::serial;

use super::{
    chat::{acme_id, acme_with_bob, general, user_id},
    documents::{PDF, PNG},
    prepare_data::{
        approve_as_owner, join, login, sign_up_member, AsMember, TestMember, USER_PASSWORD,
    },
};

/// Shows up in a response body if stored bytes leak.
const SECRET: &[u8] = b"ACME-PRIVATE-RECEIPT";

/// A PNG that ends with [`SECRET`].
fn secret_png() -> Vec<u8> {
    [PNG, SECRET].concat()
}

fn leaks(body: &[u8]) -> bool {
    body.windows(SECRET.len()).any(|w| w == SECRET)
}

async fn send_photo(
    request: &TestServer,
    who: &TestMember,
    conversation_id: i64,
    caption: Option<&str>,
    name: &str,
    bytes: &[u8],
) -> axum_test::TestResponse {
    let mut form = MultipartForm::new();
    if let Some(caption) = caption {
        form = form.add_text("body", caption);
    }
    request
        .post(&who.at(&format!("/chat/{conversation_id}/photos")))
        .as_member(who)
        .add_header("HX-Request", "true")
        .multipart(form.add_part("file", Part::bytes(bytes.to_vec()).file_name(name)))
        .await
}

fn photo_path(conversation_id: i64, message_id: i64) -> String {
    format!("/chat/{conversation_id}/photos/{message_id}")
}

async fn open_photo(
    request: &TestServer,
    who: &TestMember,
    conversation_id: i64,
    message_id: i64,
) -> axum_test::TestResponse {
    request
        .get(&who.at(&photo_path(conversation_id, message_id)))
        .add_header(who.cookie.0.clone(), who.cookie.1.clone())
        .await
}

async fn page(request: &TestServer, who: &TestMember, path: &str) -> String {
    request
        .get(&who.at(path))
        .add_header(who.cookie.0.clone(), who.cookie.1.clone())
        .await
        .text()
}

async fn only_message(ctx: &AppContext) -> messages::Model {
    let all = messages::Entity::find().all(&ctx.db).await.unwrap();
    assert_eq!(all.len(), 1);
    all.into_iter().next().unwrap()
}

async fn latest_message(ctx: &AppContext) -> messages::Model {
    messages::Entity::find()
        .all(&ctx.db)
        .await
        .unwrap()
        .into_iter()
        .max_by_key(|m| m.id)
        .unwrap()
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

async fn nothing_saved(ctx: &AppContext, org_id: i64) {
    assert_eq!(messages::Entity::find().count(&ctx.db).await.unwrap(), 0);
    assert_eq!(documents::Entity::find().count(&ctx.db).await.unwrap(), 0);
    assert!(stored_chat_files(ctx, org_id).await.is_empty());
}

async fn group_of(
    ctx: &AppContext,
    org_id: i64,
    creator_id: i64,
    name: &str,
    member_ids: Vec<i64>,
) -> conversations::Model {
    conversations::Model::create_group(
        &ctx.db,
        org_id,
        creator_id,
        &GroupParams {
            name: name.to_string(),
            member_ids,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn a_photo_posted_in_a_channel_is_stored_in_chat_photos_and_shown() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let mut events = ctx.shared_store.get::<ChatHub>().unwrap().subscribe();

        let res = send_photo(
            &request,
            &alice,
            general.id,
            Some(" New office "),
            "office.png",
            PNG,
        )
        .await;
        assert_eq!(res.status_code(), 200);

        let message = only_message(&ctx).await;
        assert_eq!(message.body, "New office");
        assert!(message.has_photo);
        assert_eq!(message.photo_key, None);
        let file_id = message.document_id.expect("the message points at its file");
        let file = documents::Model::find_in_org(&ctx.db, general.organisation_id, file_id)
            .await
            .unwrap();
        assert_eq!(file.content_type, "image/png");
        assert_eq!(
            file.uploaded_by_id,
            Some(user_id(&ctx, "alice@example.com").await)
        );
        let folder = folders::Model::find_in_org(&ctx.db, file.organisation_id, file.folder_id)
            .await
            .unwrap();
        assert_eq!(folder.name, "Chat photos");

        // The live socket gets the photo too.
        let ChatEvent::Message { message: live, .. } =
            events.try_recv().expect("the message is broadcast")
        else {
            panic!("expected a message event");
        };
        assert!(live.photo);

        let src = format!(r#"src="/o/acme{}""#, photo_path(general.id, message.id));
        assert!(res.text().contains(&src), "{}", res.text());
        let page = page(&request, &bob, &format!("/chat/{}", general.id)).await;
        assert!(page.contains(&src));
        assert!(page.contains("data-photo-view"));
        assert!(page.contains(r#"loading="lazy""#));
        assert!(
            !page.contains(r#"target="_blank""#),
            "opens in the same tab"
        );
        assert!(!page.contains("/documents/files/"), "one link shape");
        assert!(page.contains("New office"));

        let image = open_photo(&request, &bob, general.id, message.id).await;
        assert_eq!(image.status_code(), 200);
        assert_eq!(image.as_bytes().as_ref(), PNG);
        // Still in Documents, as before.
        let image = request
            .get(&bob.at(&format!("/documents/files/{file_id}")))
            .add_header(bob.cookie.0.clone(), bob.cookie.1.clone())
            .await;
        assert_eq!(image.status_code(), 200);

        // A second photo reuses the folder.
        send_photo(&request, &bob, general.id, None, "two.png", PNG).await;
        let folders = folders::Entity::find().all(&ctx.db).await.unwrap();
        assert_eq!(
            folders.iter().filter(|f| f.name == "Chat photos").count(),
            1
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn photos_work_in_dms_and_groups_for_members_only() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let org_id = acme_id(&ctx).await;
        let alice_id = user_id(&ctx, "alice@example.com").await;
        let bob_id = user_id(&ctx, "bob@example.com").await;
        let dm = conversations::Model::start_dm(&ctx.db, org_id, alice_id, bob_id)
            .await
            .unwrap();
        let group = group_of(&ctx, org_id, alice_id, "finance", vec![bob_id]).await;
        for conversation in [&dm, &group] {
            let page = page(&request, &alice, &format!("/chat/{}", conversation.id)).await;
            assert!(page.contains("data-photo-pick"), "{}", conversation.kind);
            assert!(page.contains(r#"accept="image/*""#));
            assert!(page.contains(r#"aria-label="Add a photo""#));
            assert!(
                page.contains("data-photo-attach hidden"),
                "no photo waiting yet"
            );

            let res = send_photo(&request, &alice, conversation.id, None, "a.png", PNG).await;
            assert_eq!(res.status_code(), 200, "{}", conversation.kind);
            let message = latest_message(&ctx).await;
            assert_eq!(message.conversation_id, conversation.id);
            assert!(message.photo_key.is_some());
            assert_eq!(message.document_id, None);
            assert_eq!(message.photo_type.as_deref(), Some("image/png"));

            let image = open_photo(&request, &bob, conversation.id, message.id).await;
            assert_eq!(image.status_code(), 200, "{}", conversation.kind);
            assert_eq!(image.as_bytes().as_ref(), PNG);
        }
        assert_eq!(documents::Entity::find().count(&ctx.db).await.unwrap(), 0);
        let folders = folders::Entity::find().all(&ctx.db).await.unwrap();
        assert!(!folders.iter().any(|f| f.name == "Chat photos"));
        assert_eq!(stored_chat_files(&ctx, org_id).await.len(), 2);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn group_and_dm_photos_stay_out_of_documents() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let org_id = acme_id(&ctx).await;
        let alice_id = user_id(&ctx, "alice@example.com").await;
        let bob_id = user_id(&ctx, "bob@example.com").await;
        let group = group_of(&ctx, org_id, alice_id, "finance", vec![bob_id]).await;
        let before = documents::Entity::find().count(&ctx.db).await.unwrap();
        let res = send_photo(&request, &alice, group.id, None, "Fuel-receipt.png", PNG).await;
        assert_eq!(res.status_code(), 200);
        let message = only_message(&ctx).await;
        let key = message.photo_key.clone().unwrap();

        assert_eq!(
            documents::Entity::find().count(&ctx.db).await.unwrap(),
            before
        );
        let keys: Vec<String> = documents::Entity::find()
            .all(&ctx.db)
            .await
            .unwrap()
            .into_iter()
            .map(|d| d.storage_key)
            .collect();
        assert!(!keys.contains(&key));
        for who in [&alice, &bob] {
            let index = page(&request, who, "/documents").await;
            assert!(!index.contains("Fuel-receipt"), "{index}");
            for folder in folders::Model::list_for_org(&ctx.db, org_id).await.unwrap() {
                let shown = page(
                    &request,
                    who,
                    &format!("/documents/folders/{}", folder.folder.id),
                )
                .await;
                assert!(!shown.contains("Fuel-receipt"), "{}", folder.folder.name);
            }
        }
        // The message id is not a Documents file id.
        let res = request
            .get(&alice.at(&format!("/documents/files/{}", message.id)))
            .add_header(alice.cookie.0.clone(), alice.cookie.1.clone())
            .await;
        assert_eq!(res.status_code(), 404);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_caption_and_a_photo_send_as_one_message() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _bob) = acme_with_bob(&request, &ctx).await;
        let org_id = acme_id(&ctx).await;
        let alice_id = user_id(&ctx, "alice@example.com").await;
        let group = group_of(&ctx, org_id, alice_id, "finance", vec![]).await;
        let res = send_photo(
            &request,
            &alice,
            group.id,
            Some("Van fuel £62"),
            "receipt.jpg",
            PNG,
        )
        .await;
        assert_eq!(res.status_code(), 200);
        let message = only_message(&ctx).await;
        assert_eq!(message.body, "Van fuel £62");
        assert!(message.has_photo);
        assert!(message.photo_key.is_some());
        assert_eq!(
            message.photo_name.as_deref(),
            Some("receipt.png"),
            "the bytes decide"
        );

        send_photo(&request, &alice, group.id, Some("  \n "), "b.png", PNG).await;
        assert_eq!(latest_message(&ctx).await.body, "");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn group_and_dm_photos_open_only_for_current_members() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let org_id = acme_id(&ctx).await;
        let carol = join(&request, &ctx, "acme", "carol", "carol@example.com").await;
        let alice_id = user_id(&ctx, "alice@example.com").await;
        let bob_id = user_id(&ctx, "bob@example.com").await;
        let carol_id = user_id(&ctx, "carol@example.com").await;
        let carol_m = memberships::Model::default_for_user(&ctx.db, carol_id)
            .await
            .unwrap()
            .unwrap();
        let carol_m = approve_as_owner(&ctx, &carol_m).await;
        let group = group_of(&ctx, org_id, alice_id, "finance", vec![bob_id]).await;
        let dm = conversations::Model::start_dm(&ctx.db, org_id, alice_id, bob_id)
            .await
            .unwrap();
        let general = general(&ctx).await;
        let secret = secret_png();
        send_photo(
            &request,
            &alice,
            group.id,
            Some("Receipt"),
            "r.png",
            &secret,
        )
        .await;
        let group_photo = latest_message(&ctx).await;
        send_photo(&request, &alice, dm.id, None, "d.png", &secret).await;
        let dm_photo = latest_message(&ctx).await;
        send_photo(&request, &carol, general.id, None, "g.png", PNG).await;
        let general_photo = latest_message(&ctx).await;
        let photos = [(group.id, group_photo.id), (dm.id, dm_photo.id)];

        // Carol is in Acme, not in either.
        for (conversation_id, message_id) in photos {
            let res = open_photo(&request, &carol, conversation_id, message_id).await;
            assert_eq!(res.status_code(), 404);
            assert!(!leaks(res.as_bytes()));
            let res = send_photo(&request, &carol, conversation_id, None, "x.png", PNG).await;
            assert_eq!(res.status_code(), 404);
        }
        // Crossed ids: a message from another conversation under this one's address.
        for who in [&carol, &bob] {
            let res = open_photo(&request, who, group.id, general_photo.id).await;
            assert_eq!(res.status_code(), 404);
            let res = open_photo(&request, who, general.id, group_photo.id).await;
            assert_eq!(res.status_code(), 404);
            assert!(!leaks(res.as_bytes()));
        }
        let res = open_photo(&request, &bob, dm.id, group_photo.id).await;
        assert_eq!(res.status_code(), 404);

        // Signed out.
        let res = request
            .get(&format!("/o/acme{}", photo_path(group.id, group_photo.id)))
            .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(res.headers()["location"], "/login");

        // Waiting to join.
        let dave = join(&request, &ctx, "acme", "dave", "dave@example.com").await;
        for (conversation_id, message_id) in photos {
            let res = open_photo(&request, &dave, conversation_id, message_id).await;
            assert_eq!(res.status_code(), 403);
            assert!(!leaks(res.as_bytes()));
        }

        // From another organisation: at Acme's address, and at her own.
        let mallory = sign_up_member(&request, "Other", "mallory", "mallory@example.com").await;
        for (conversation_id, message_id) in photos {
            let res = request
                .get(&format!(
                    "/o/acme{}",
                    photo_path(conversation_id, message_id)
                ))
                .add_header(mallory.cookie.0.clone(), mallory.cookie.1.clone())
                .await;
            assert_eq!(res.status_code(), 404);
            assert!(!leaks(res.as_bytes()));
            let res = open_photo(&request, &mallory, conversation_id, message_id).await;
            assert_eq!(res.status_code(), 404);
            assert!(!leaks(res.as_bytes()));
        }

        // A super admin acting in Acme.
        let admin = super::admin::super_admin_cookie(&request, &ctx).await;
        let acting = format!("{}; acting_org={org_id}", admin.1.to_str().unwrap());
        let res = request
            .get(&format!("/o/acme{}", photo_path(group.id, group_photo.id)))
            .add_header("cookie", acting.clone())
            .await;
        assert_eq!(res.status_code(), 403);
        assert!(!leaks(res.as_bytes()));
        let res = request
            .post(&format!("/o/acme/chat/{}/photos", general.id))
            .add_header("cookie", acting.clone())
            .add_header("x-org-id", org_id.to_string())
            .multipart(
                MultipartForm::new().add_part("file", Part::bytes(PNG.to_vec()).file_name("a.png")),
            )
            .await;
        assert_eq!(res.status_code(), 403);

        // Role does not matter: Carol as an admin is still not in the group.
        carol_m.change_role(&ctx.db, "admin").await.unwrap();
        let res = open_photo(&request, &carol, group.id, group_photo.id).await;
        assert_eq!(res.status_code(), 404);

        // Bob is removed, so both close.
        let bob_m = memberships::Model::default_for_user(&ctx.db, bob_id)
            .await
            .unwrap()
            .unwrap();
        memberships::Model::remove(&ctx, org_id, alice_id, bob_m.id, &Default::default())
            .await
            .unwrap();
        let removed = TestMember::new(
            login(&request, "bob@example.com", USER_PASSWORD).await,
            org_id,
            "acme",
        );
        for (conversation_id, message_id) in photos {
            let res = open_photo(&request, &removed, conversation_id, message_id).await;
            assert_eq!(res.status_code(), 403);
            assert!(!leaks(res.as_bytes()));
        }
        // Approved again: the DM comes back, the group does not.
        let org = organisations::Model::find_by_id(&ctx.db, org_id)
            .await
            .unwrap();
        let memberships::JoinOutcome::Requested(again) = memberships::Model::request_to_join(
            &ctx.db,
            &org,
            bob_id,
            &memberships::JoinParams {
                name: "bob".to_string(),
            },
        )
        .await
        .unwrap() else {
            panic!("bob asks again");
        };
        approve_as_owner(&ctx, &again).await;
        let back = TestMember::new(
            login(&request, "bob@example.com", USER_PASSWORD).await,
            org_id,
            "acme",
        );
        let res = open_photo(&request, &back, group.id, group_photo.id).await;
        assert_eq!(res.status_code(), 404);
        let res = open_photo(&request, &back, dm.id, dm_photo.id).await;
        assert_eq!(res.status_code(), 200);
        assert!(leaks(res.as_bytes()), "the real bytes");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn chat_photos_are_sent_with_safe_headers() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _bob) = acme_with_bob(&request, &ctx).await;
        let org_id = acme_id(&ctx).await;
        let alice_id = user_id(&ctx, "alice@example.com").await;
        let group = group_of(&ctx, org_id, alice_id, "finance", vec![]).await;
        let general = general(&ctx).await;
        send_photo(&request, &alice, group.id, None, "Receipt.png", PNG).await;
        let group_photo = latest_message(&ctx).await;
        send_photo(&request, &alice, general.id, None, "Office.png", PNG).await;
        let general_photo = latest_message(&ctx).await;

        for (conversation_id, message, name) in [
            (group.id, &group_photo, "Receipt.png"),
            (general.id, &general_photo, "Office.png"),
        ] {
            let res = open_photo(&request, &alice, conversation_id, message.id).await;
            assert_eq!(res.status_code(), 200);
            let headers = res.headers();
            assert_eq!(headers["content-type"], "image/png");
            assert_eq!(headers["content-length"], PNG.len().to_string());
            assert_eq!(headers["x-content-type-options"], "nosniff");
            assert_eq!(headers["cache-control"], "private, no-store");
            assert_eq!(headers["cross-origin-resource-policy"], "same-origin");
            assert_eq!(headers["x-frame-options"], "DENY");
            let csp = headers["content-security-policy"].to_str().unwrap();
            assert!(csp.contains("default-src 'none'"), "{csp}");
            assert!(csp.contains("sandbox"), "{csp}");
            assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
            assert_eq!(
                headers["content-disposition"],
                format!(
                    r#"inline; filename="{name}"; filename*=UTF-8''{}"#,
                    name.replace('.', "%2E")
                )
            );
            let res = request
                .get(&alice.at(&format!(
                    "{}?download=1",
                    photo_path(conversation_id, message.id)
                )))
                .add_header(alice.cookie.0.clone(), alice.cookie.1.clone())
                .await;
            let disposition = res.headers()["content-disposition"].to_str().unwrap();
            assert!(disposition.starts_with("attachment;"), "{disposition}");
        }

        // PNG bytes named like HTML, with quotes, a line break and a
        // right-to-left override: stored as a PNG, served with a valid header.
        let message = messages::Model::create_photo(
            &ctx,
            &group,
            alice_id,
            "",
            "evil\"name\r\n\u{202E}lmth.html",
            bytes::Bytes::from_static(PNG),
        )
        .await
        .unwrap();
        assert_eq!(message.photo_type.as_deref(), Some("image/png"));
        assert_eq!(message.photo_size, Some(PNG.len() as i64));
        let name = message.photo_name.clone().unwrap();
        assert!(name.ends_with(".png"), "{name}");
        assert!(!name.contains(['\r', '\n', '\u{202E}']), "{name}");
        let res = open_photo(&request, &alice, group.id, message.id).await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(res.headers()["content-type"], "image/png");
        let disposition = res.headers()["content-disposition"].to_str().unwrap();
        assert!(
            disposition.starts_with("inline; filename=\"evil_name"),
            "{disposition}"
        );
        assert!(
            disposition.contains(".png\"; filename*=UTF-8''"),
            "{disposition}"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn only_photos_and_short_captions_are_accepted() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _bob) = acme_with_bob(&request, &ctx).await;
        let org_id = acme_id(&ctx).await;
        let alice_id = user_id(&ctx, "alice@example.com").await;
        let group = group_of(&ctx, org_id, alice_id, "finance", vec![]).await;
        let general = general(&ctx).await;
        let too_big = vec![0u8; documents::MAX_BYTES + 1];

        for conversation in [&general, &group] {
            // A PDF named like a photo: the bytes decide.
            let res = send_photo(&request, &alice, conversation.id, None, "fake.png", PDF).await;
            assert_eq!(res.status_code(), 422);
            assert!(res.text().contains("Only photos"), "{}", res.text());
            let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"></svg>"#;
            let res = send_photo(&request, &alice, conversation.id, None, "a.svg", svg).await;
            assert_eq!(res.status_code(), 422);

            let long = "x".repeat(1001);
            let res =
                send_photo(&request, &alice, conversation.id, Some(&long), "a.png", PNG).await;
            assert_eq!(res.status_code(), 422);
            assert!(res.text().contains("1,000"), "{}", res.text());

            let res = request
                .post(&alice.at(&format!("/chat/{}/photos", conversation.id)))
                .as_member(&alice)
                .multipart(MultipartForm::new().add_text("body", "no file"))
                .await;
            assert_eq!(res.status_code(), 422);

            let res =
                send_photo(&request, &alice, conversation.id, None, "big.png", &too_big).await;
            assert_eq!(res.status_code(), 422);
            assert!(res.text().contains("Too big"), "{}", res.text());
        }
        nothing_saved(&ctx, org_id).await;
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_photo_with_no_caption_says_photo() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let res = send_photo(&request, &alice, general.id, Some("  "), "office.png", PNG).await;
        assert_eq!(res.status_code(), 200);
        let message = only_message(&ctx).await;
        assert_eq!(message.body, "");

        let list = page(&request, &bob, "/chat").await;
        assert!(list.contains("alice: Photo<"), "{list}");
        let home = page(&request, &bob, "/dashboard").await;
        assert!(home.contains("alice: Photo<"), "{home}");
        let own = page(&request, &alice, "/chat").await;
        assert!(own.contains("You: Photo<"), "{own}");

        // In a DM, the other person's photo needs no name.
        let org_id = acme_id(&ctx).await;
        let alice_id = user_id(&ctx, "alice@example.com").await;
        let bob_id = user_id(&ctx, "bob@example.com").await;
        let dm = conversations::Model::start_dm(&ctx.db, org_id, alice_id, bob_id)
            .await
            .unwrap();
        send_photo(&request, &alice, dm.id, None, "a.png", PNG).await;
        let list = page(&request, &bob, "/chat").await;
        assert!(list.contains(">Photo<"), "{list}");
        send_photo(
            &request,
            &alice,
            general.id,
            Some("New office"),
            "b.png",
            PNG,
        )
        .await;
        let list = page(&request, &bob, "/chat").await;
        assert!(list.contains("alice: Photo · New office<"), "{list}");

        let sheet = request
            .get(&bob.at(&format!(
                "/chat/{}/messages/{}/actions",
                general.id, message.id
            )))
            .as_member(&bob)
            .add_header("HX-Request", "true")
            .await
            .text();
        assert!(sheet.contains("<p>Photo</p>"), "{sheet}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn people_outside_the_organisation_cannot_post_photos() {
    request::<App, _, _>(|request, ctx| async move {
        let (_alice, _bob) = acme_with_bob(&request, &ctx).await;
        let org_id = acme_id(&ctx).await;
        let general = general(&ctx).await;
        let mallory = sign_up_member(&request, "Other", "mallory", "mallory@example.com").await;
        let res = send_photo(&request, &mallory, general.id, None, "a.png", PNG).await;
        assert_eq!(res.status_code(), 404);
        nothing_saved(&ctx, org_id).await;
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_deleted_photo_leaves_a_note() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        send_photo(
            &request,
            &alice,
            general.id,
            Some("Whiteboard"),
            "a.png",
            PNG,
        )
        .await;
        let file_id = only_message(&ctx).await.document_id.unwrap();
        let res = request
            .post(&alice.at(&format!("/documents/files/{file_id}/delete")))
            .as_member(&alice)
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 200);

        let message = only_message(&ctx).await;
        assert_eq!(message.document_id, None);
        assert!(message.has_photo);
        let page = page(&request, &bob, &format!("/chat/{}", general.id)).await;
        assert!(page.contains("Photo removed"));
        assert!(page.contains("Whiteboard"));
        assert!(!page.contains(&photo_path(general.id, message.id)));
        let res = open_photo(&request, &bob, general.id, message.id).await;
        assert_eq!(res.status_code(), 404);
    })
    .await;
}

/// A #general message only ever serves a photo, even if its Documents file
/// is something else.
#[tokio::test]
#[serial]
async fn a_general_message_pointed_at_a_pdf_is_not_served() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let org_id = acme_id(&ctx).await;
        let general = general(&ctx).await;
        send_photo(&request, &alice, general.id, None, "a.png", PNG).await;
        let message = only_message(&ctx).await;
        let folder = folders::Model::chat_photos(&ctx.db, org_id).await.unwrap();
        let alice_id = user_id(&ctx, "alice@example.com").await;
        let pdf = documents::Model::upload(
            &ctx,
            &folder,
            alice_id,
            "lease.pdf",
            bytes::Bytes::from_static(PDF),
        )
        .await
        .unwrap();
        let mut row = message.into_active_model();
        row.document_id = ActiveValue::Set(Some(pdf.id));
        let message = row.update(&ctx.db).await.unwrap();

        let res = open_photo(&request, &bob, general.id, message.id).await;
        assert_eq!(res.status_code(), 404);
        assert!(!res.text().contains("%PDF"));
    })
    .await;
}

/// A photo can start a thread, so the thread page has the viewer, but
/// replies are text only.
#[tokio::test]
#[serial]
async fn thread_replies_have_no_photo_button() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _bob) = acme_with_bob(&request, &ctx).await;
        let org_id = acme_id(&ctx).await;
        let alice_id = user_id(&ctx, "alice@example.com").await;
        let group = group_of(&ctx, org_id, alice_id, "finance", vec![]).await;
        send_photo(&request, &alice, group.id, Some("Receipt"), "a.png", PNG).await;
        let message = only_message(&ctx).await;
        let thread = page(
            &request,
            &alice,
            &format!("/chat/{}/threads/{}", group.id, message.id),
        )
        .await;
        assert!(!thread.contains("data-photo-pick"), "{thread}");
        assert!(!thread.contains("data-photo-input"));
        assert!(thread.contains(r#"id="photo-viewer""#));
        assert!(thread.contains(r##"data-place="#finance""##));
        assert!(thread.contains(&photo_path(group.id, message.id)));

        let show = page(&request, &alice, &format!("/chat/{}", group.id)).await;
        assert!(show.contains(r#"id="photo-viewer""#));
        let dm_id = conversations::Model::start_dm(
            &ctx.db,
            org_id,
            alice_id,
            user_id(&ctx, "bob@example.com").await,
        )
        .await
        .unwrap()
        .id;
        let dm = page(&request, &alice, &format!("/chat/{dm_id}")).await;
        assert!(dm.contains(r#"data-place="Direct message""#));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn two_first_photos_at_once_share_one_folder() {
    request::<App, _, _>(|request, ctx| async move {
        let (_alice, _bob) = acme_with_bob(&request, &ctx).await;
        let org_id = acme_id(&ctx).await;
        let (a, b) = tokio::join!(
            folders::Model::chat_photos(&ctx.db, org_id),
            folders::Model::chat_photos(&ctx.db, org_id),
        );
        assert_eq!(a.unwrap().id, b.unwrap().id);
    })
    .await;
}

/// A caption and a group name that look like HTML stay text, in the page,
/// the send response and the socket. Neither shows a storage key.
#[tokio::test]
#[serial]
async fn captions_and_group_names_are_escaped_and_keys_stay_private() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

    const NASTY: &str = r#""><img src=x onerror=alert(1)>"#;
    let (ctx, addr, cookie, alice, _general_socket) =
        super::live_sign_out::open_general_socket().await;
    let org_id = acme_id(&ctx).await;
    let group = group_of(&ctx, org_id, alice.id, NASTY, vec![]).await;
    let mut request = format!("ws://{addr}/o/acme/chat/{}/ws", group.id)
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    // Our own text message comes back once the socket is listening.
    socket
        .send(Message::text(r#"{"body":"hello"}"#))
        .await
        .unwrap();
    let echo = tokio::time::timeout(super::live_sign_out::WAIT, socket.next())
        .await
        .unwrap();
    assert!(matches!(echo, Some(Ok(Message::Text(_)))), "{echo:?}");

    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let caption =
        format!("--b\r\nContent-Disposition: form-data; name=\"body\"\r\n\r\n{NASTY}\r\n");
    let file = "--b\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\n";
    let form = [caption.as_bytes(), file.as_bytes(), PNG, b"\r\n--b--\r\n"].concat();
    let sent = http
        .post(format!("http://{addr}/o/acme/chat/{}/photos", group.id))
        .header("cookie", &cookie)
        .header("x-org-id", org_id.to_string())
        .header("HX-Request", "true")
        .header("content-type", "multipart/form-data; boundary=b")
        .body(form)
        .send()
        .await
        .unwrap();
    assert_eq!(sent.status(), 200);
    let sent = sent.text().await.unwrap();
    let message = latest_message(&ctx).await;
    let key = message.photo_key.clone().unwrap();

    let live = tokio::time::timeout(super::live_sign_out::WAIT, socket.next())
        .await
        .unwrap();
    let Some(Ok(Message::Text(live))) = live else {
        panic!("expected the photo message, got {live:?}");
    };
    let page = http
        .get(format!("http://{addr}/o/acme/chat/{}", group.id))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    for (what, html) in [
        ("page", &page),
        ("send", &sent),
        ("socket", &live.to_string()),
    ] {
        assert!(html.contains("data-photo-view"), "{what}: {html}");
        assert!(
            !html.contains("<img src=x"),
            "{what} draws the caption as HTML"
        );
        assert!(
            html.contains("&lt;img src=x onerror=alert(1)&gt;"),
            "{what}: {html}"
        );
        assert!(!html.contains(&key), "{what} shows the storage key");
        assert!(!html.contains("orgs/"), "{what} shows a storage path");
    }
}
