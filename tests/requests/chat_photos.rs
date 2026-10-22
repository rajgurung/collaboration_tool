//! Photos in chat: channels only, stored in the "Chat photos" folder in
//! Documents, shown in the feed and named in lists when there's no caption.
use axum_test::multipart::{MultipartForm, Part};
use collab::{
    app::App,
    data::chat_hub::{ChatEvent, ChatHub},
    models::{
        conversations::{self, GroupParams},
        documents, folders, messages,
    },
};
use loco_rs::{app::AppContext, testing::prelude::*, TestServer};
use sea_orm::{EntityTrait, PaginatorTrait};
use serial_test::serial;

use super::{
    chat::{acme_id, acme_with_bob, general, user_id},
    documents::{PDF, PNG},
    prepare_data::{sign_up_member, AsMember, TestMember},
};

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

async fn only_message(ctx: &AppContext) -> messages::Model {
    let all = messages::Entity::find().all(&ctx.db).await.unwrap();
    assert_eq!(all.len(), 1);
    all.into_iter().next().unwrap()
}

async fn nothing_saved(ctx: &AppContext) {
    assert_eq!(messages::Entity::find().count(&ctx.db).await.unwrap(), 0);
    assert_eq!(documents::Entity::find().count(&ctx.db).await.unwrap(), 0);
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
        assert_eq!(live.photo, Some(file_id));

        let src = format!(r#"src="/o/acme/documents/files/{file_id}""#);
        assert!(res.text().contains(&src), "{}", res.text());
        let page = request
            .get(&bob.at(&format!("/chat/{}", general.id)))
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(page.contains(&src));
        assert!(page.contains(r#"loading="lazy""#));
        assert!(page.contains(r#"target="_blank""#));
        assert!(page.contains("New office"));

        let image = request
            .get(&bob.at(&format!("/documents/files/{file_id}")))
            .add_header(bob.0.clone(), bob.1.clone())
            .await;
        assert_eq!(image.status_code(), 200);
        assert_eq!(image.as_bytes().as_ref(), PNG);

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
async fn a_photo_with_no_caption_says_sent_a_photo() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let res = send_photo(&request, &alice, general.id, Some("  "), "office.png", PNG).await;
        assert_eq!(res.status_code(), 200);
        let message = only_message(&ctx).await;
        assert_eq!(message.body, "");

        let list = request
            .get(&bob.at("/chat"))
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(list.contains("alice sent a photo"), "{list}");
        let home = request
            .get(&bob.at("/dashboard"))
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(home.contains("alice sent a photo"));
        let own = request
            .get(&alice.at("/chat"))
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(own.contains("You sent a photo"));

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
async fn photos_are_refused_in_dms_and_groups() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _bob) = acme_with_bob(&request, &ctx).await;
        let org_id = acme_id(&ctx).await;
        let alice_id = user_id(&ctx, "alice@example.com").await;
        let bob_id = user_id(&ctx, "bob@example.com").await;
        let dm = conversations::Model::start_dm(&ctx.db, org_id, alice_id, bob_id)
            .await
            .unwrap();
        let group = conversations::Model::create_group(
            &ctx.db,
            org_id,
            alice_id,
            &GroupParams {
                name: "launch".to_string(),
                member_ids: vec![bob_id],
            },
        )
        .await
        .unwrap();
        for conversation in [&dm, &group] {
            let res = send_photo(&request, &alice, conversation.id, None, "a.png", PNG).await;
            assert_eq!(res.status_code(), 403, "{}", conversation.kind);
            let page = request
                .get(&alice.at(&format!("/chat/{}", conversation.id)))
                .add_header(alice.0.clone(), alice.1.clone())
                .await
                .text();
            assert!(
                !page.contains("data-photo-pick"),
                "no button in a {}",
                conversation.kind
            );
        }
        nothing_saved(&ctx).await;
        let folders = folders::Entity::find().all(&ctx.db).await.unwrap();
        assert!(!folders.iter().any(|f| f.name == "Chat photos"));

        let page = request
            .get(&alice.at(&format!("/chat/{}", general(&ctx).await.id)))
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(page.contains("data-photo-pick"), "channels have the button");
        assert!(page.contains(r#"accept="image/*""#));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn only_photos_and_short_captions_are_accepted() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;

        // A PDF named like a photo: the bytes decide.
        let res = send_photo(&request, &alice, general.id, None, "fake.png", PDF).await;
        assert_eq!(res.status_code(), 422);
        assert!(res.text().contains("Only photos"), "{}", res.text());
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"></svg>"#;
        let res = send_photo(&request, &alice, general.id, None, "a.svg", svg).await;
        assert_eq!(res.status_code(), 422);

        let long = "x".repeat(1001);
        let res = send_photo(&request, &alice, general.id, Some(&long), "a.png", PNG).await;
        assert_eq!(res.status_code(), 422);
        assert!(res.text().contains("1,000"), "{}", res.text());

        let res = request
            .post(&alice.at(&format!("/chat/{}/photos", general.id)))
            .as_member(&alice)
            .multipart(MultipartForm::new().add_text("body", "no file"))
            .await;
        assert_eq!(res.status_code(), 422);
        nothing_saved(&ctx).await;
    })
    .await;
}

#[tokio::test]
#[serial]
async fn people_outside_the_organisation_cannot_post_photos() {
    request::<App, _, _>(|request, ctx| async move {
        let (_alice, _bob) = acme_with_bob(&request, &ctx).await;
        let general = general(&ctx).await;
        let mallory = sign_up_member(&request, "Other", "mallory", "mallory@example.com").await;
        let res = send_photo(&request, &mallory, general.id, None, "a.png", PNG).await;
        assert_eq!(res.status_code(), 404);
        nothing_saved(&ctx).await;
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
        let page = request
            .get(&bob.at(&format!("/chat/{}", general.id)))
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(page.contains("Photo removed"));
        assert!(page.contains("Whiteboard"));
        assert!(!page.contains("/documents/files/"));
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
