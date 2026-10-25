//! Deleting a chat photo. The sender, and owners and admins who are in the
//! conversation, can delete it for everyone: the stored bytes go (for
//! #general, the Documents file), the message stays and says "Photo
//! removed", and open pages hear about it over the socket. Everyone else is
//! refused, and nothing changes.
use std::path::Path;

use collab::{
    app::App,
    data::chat_hub::{ChatEvent, ChatHub},
    models::{
        conversations, documents, folders,
        memberships::{self, role},
        messages,
    },
};
use loco_rs::{app::AppContext, testing::prelude::*, TestServer};
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait, IntoActiveModel, PaginatorTrait};
use serial_test::serial;
use tokio::sync::broadcast::Receiver;

use super::{
    chat::user_id,
    chat_photos::{
        group_of, latest_message, leaks, open_photo, page, photo_path, secret_png, send_photo,
        stored_chat_files,
    },
    documents::PDF,
    prepare_data::{
        approve_as_owner, join, login, sign_up_member, AsMember, TestMember, USER_PASSWORD,
    },
};

const REFUSED: &str = "Only the person who sent a photo, owners and admins can delete it.";

/// Juniper Tea Co: Ana (owner), Maya (admin), Tom and Leo (members). #finance
/// is Ana's group with Maya and Tom; Leo is not in it.
struct Team {
    org_id: i64,
    ana: TestMember,
    maya: TestMember,
    tom: TestMember,
    leo: TestMember,
    ana_id: i64,
    maya_id: i64,
    tom_id: i64,
    leo_id: i64,
    general: conversations::Model,
    finance: conversations::Model,
}

async fn juniper(request: &TestServer, ctx: &AppContext) -> Team {
    let ana = sign_up_member(request, "Juniper Tea Co", "ana", "ana@example.com").await;
    let org_id = ana.org_id;
    let mut people = Vec::new();
    for name in ["maya", "tom", "leo"] {
        let email = format!("{name}@example.com");
        let who = join(request, ctx, &ana.slug, name, &email).await;
        let id = user_id(ctx, &email).await;
        let pending = memberships::Model::default_for_user(&ctx.db, id)
            .await
            .unwrap()
            .unwrap();
        let approved = approve_as_owner(ctx, &pending).await;
        if name == "maya" {
            let mut admin = approved.into_active_model();
            admin.role = ActiveValue::Set(role::ADMIN.to_string());
            admin.update(&ctx.db).await.unwrap();
        }
        people.push((who, id));
    }
    let [(maya, maya_id), (tom, tom_id), (leo, leo_id)] = people.try_into().unwrap();
    let ana_id = user_id(ctx, "ana@example.com").await;
    let general = conversations::Model::find_general(&ctx.db, org_id)
        .await
        .unwrap();
    let finance = group_of(ctx, org_id, ana_id, "finance", vec![maya_id, tom_id]).await;
    Team {
        org_id,
        ana,
        maya,
        tom,
        leo,
        ana_id,
        maya_id,
        tom_id,
        leo_id,
        general,
        finance,
    }
}

/// `who` posts a photo whose bytes end in the secret, and gets its message.
async fn post_photo(
    request: &TestServer,
    ctx: &AppContext,
    who: &TestMember,
    conversation_id: i64,
    caption: Option<&str>,
) -> messages::Model {
    let res = send_photo(
        request,
        who,
        conversation_id,
        caption,
        "whiteboard-tuesday.png",
        &secret_png(),
    )
    .await;
    assert_eq!(res.status_code(), 200, "{}", res.text());
    latest_message(ctx).await
}

fn delete_path(conversation_id: i64, message_id: i64) -> String {
    format!("{}/delete", photo_path(conversation_id, message_id))
}

async fn delete(
    request: &TestServer,
    who: &TestMember,
    conversation_id: i64,
    message_id: i64,
) -> axum_test::TestResponse {
    request
        .post(&who.at(&delete_path(conversation_id, message_id)))
        .as_member(who)
        .add_header("HX-Request", "true")
        .await
}

/// A plain form post with no org claim, as from the waiting page. Refusals
/// come back as pages (403, 404) rather than an HTMX move.
async fn plain_post(
    request: &TestServer,
    who: &TestMember,
    conversation_id: i64,
    message_id: i64,
) -> axum_test::TestResponse {
    request
        .post(&who.at(&delete_path(conversation_id, message_id)))
        .add_header(who.cookie.0.clone(), who.cookie.1.clone())
        .await
}

async fn sheet(
    request: &TestServer,
    who: &TestMember,
    conversation_id: i64,
    message_id: i64,
    query: &str,
) -> String {
    request
        .get(&who.at(&format!(
            "/chat/{conversation_id}/messages/{message_id}/actions{query}"
        )))
        .as_member(who)
        .add_header("HX-Request", "true")
        .await
        .text()
}

async fn reload(ctx: &AppContext, message: &messages::Model) -> messages::Model {
    messages::Entity::find_by_id(message.id)
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap()
}

async fn bytes_exist(ctx: &AppContext, key: &str) -> bool {
    ctx.storage
        .download::<Vec<u8>>(Path::new(key))
        .await
        .is_ok()
}

fn hub_events(ctx: &AppContext) -> Receiver<ChatEvent> {
    ctx.shared_store.get::<ChatHub>().unwrap().subscribe()
}

/// Every `PhotoRemoved` waiting on `events`, as (conversation, message).
/// Other events (page loads publish read receipts) are skipped.
fn removals(events: &mut Receiver<ChatEvent>) -> Vec<(i64, i64)> {
    let mut found = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let ChatEvent::PhotoRemoved {
            conversation_id,
            message_id,
        } = event
        {
            found.push((conversation_id, message_id));
        }
    }
    found
}

#[tokio::test]
#[serial]
async fn the_sender_deletes_a_group_photo_for_everyone() {
    request::<App, _, _>(|request, ctx| async move {
        let team = juniper(&request, &ctx).await;
        let finance = team.finance.id;
        let photo = post_photo(&request, &ctx, &team.tom, finance, Some("Van fuel £62")).await;
        let key = photo.photo_key.clone().unwrap();
        assert!(bytes_exist(&ctx, &key).await);
        let mut events = hub_events(&ctx);

        let res = delete(&request, &team.tom, finance, photo.id).await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        assert!(!bytes_exist(&ctx, &key).await, "the bytes are gone");
        assert!(stored_chat_files(&ctx, team.org_id).await.is_empty());
        assert_eq!(removals(&mut events), vec![(finance, photo.id)]);

        let saved = reload(&ctx, &photo).await;
        assert!(saved.photo_removed());
        assert_eq!(saved.body, "Van fuel £62");
        for who in [&team.tom, &team.maya] {
            let res = open_photo(&request, who, finance, photo.id).await;
            assert_eq!(res.status_code(), 404);
            assert!(!leaks(res.as_bytes()));
        }
        let feed = page(&request, &team.maya, &format!("/chat/{finance}")).await;
        assert!(feed.contains("Photo removed"), "{feed}");
        assert!(feed.contains("Van fuel £62"));
        assert!(feed.contains(&format!(r#"id="photo-{}""#, photo.id)));
        assert!(!feed.contains(&photo_path(finance, photo.id)), "{feed}");
        assert!(!leaks(feed.as_bytes()));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn owners_and_admins_in_the_conversation_delete_anyones_photo() {
    request::<App, _, _>(|request, ctx| async move {
        let team = juniper(&request, &ctx).await;
        let finance = team.finance.id;
        let photo = post_photo(&request, &ctx, &team.tom, finance, None).await;
        let feed = page(&request, &team.maya, &format!("/chat/{finance}")).await;
        assert!(
            feed.contains(&format!(
                r#"data-delete-url="/o/{}{}""#,
                team.maya.slug,
                delete_path(finance, photo.id)
            )),
            "{feed}"
        );
        let res = delete(&request, &team.maya, finance, photo.id).await;
        assert_eq!(res.status_code(), 200, "the admin");
        assert!(reload(&ctx, &photo).await.photo_removed());

        let dm = conversations::Model::start_dm(&ctx.db, team.org_id, team.leo_id, team.ana_id)
            .await
            .unwrap();
        let photo = post_photo(&request, &ctx, &team.leo, dm.id, Some("Courier invoice")).await;
        let res = delete(&request, &team.ana, dm.id, photo.id).await;
        assert_eq!(res.status_code(), 200, "the owner");
        assert!(reload(&ctx, &photo).await.photo_removed());
        assert!(stored_chat_files(&ctx, team.org_id).await.is_empty());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_member_cannot_delete_someone_elses_photo() {
    request::<App, _, _>(|request, ctx| async move {
        let team = juniper(&request, &ctx).await;
        let general = team.general.id;
        let photo = post_photo(&request, &ctx, &team.tom, general, Some("Tea tin")).await;
        let file_id = photo.document_id.unwrap();
        let file = documents::Model::find_in_org(&ctx.db, team.org_id, file_id)
            .await
            .unwrap();
        let mut events = hub_events(&ctx);

        let res = delete(&request, &team.leo, general, photo.id).await;
        assert_eq!(res.status_code(), 403);
        assert!(res.text().contains(REFUSED), "{}", res.text());
        assert_eq!(reload(&ctx, &photo).await, photo);
        assert!(documents::Model::find_in_org(&ctx.db, team.org_id, file_id)
            .await
            .is_ok());
        assert!(bytes_exist(&ctx, &file.storage_key).await);
        assert!(removals(&mut events).is_empty());

        let feed = page(&request, &team.leo, &format!("/chat/{general}")).await;
        assert!(feed.contains(&format!(r#"id="photo-{}""#, photo.id)));
        assert!(!feed.contains("data-delete-url"), "{feed}");
        let leos = sheet(&request, &team.leo, general, photo.id, "").await;
        assert!(!leos.contains("Delete photo"), "{leos}");
        for who in [&team.tom, &team.maya, &team.ana] {
            let html = sheet(&request, who, general, photo.id, "").await;
            assert!(html.contains("Delete photo"), "{html}");
            assert!(
                html.contains(&format!(r#"data-photo-delete="{}""#, photo.id)),
                "{html}"
            );
        }
        let toms = page(&request, &team.tom, &format!("/chat/{general}")).await;
        assert!(toms.contains("data-delete-url"), "{toms}");

        // In a DM, too: Tom can't delete the photo Leo sent him.
        let dm = conversations::Model::start_dm(&ctx.db, team.org_id, team.tom_id, team.leo_id)
            .await
            .unwrap();
        let leos_photo = post_photo(&request, &ctx, &team.leo, dm.id, None).await;
        let key = leos_photo.photo_key.clone().unwrap();
        let mut events = hub_events(&ctx);
        let res = delete(&request, &team.tom, dm.id, leos_photo.id).await;
        assert_eq!(res.status_code(), 403);
        assert!(res.text().contains(REFUSED), "{}", res.text());
        assert!(bytes_exist(&ctx, &key).await);
        assert_eq!(
            stored_chat_files(&ctx, team.org_id).await,
            vec![key.clone()],
            "still under orgs/{{org}}/chat/"
        );
        assert_eq!(reload(&ctx, &leos_photo).await, leos_photo);
        assert!(removals(&mut events).is_empty());
        let toms = page(&request, &team.tom, &format!("/chat/{}", dm.id)).await;
        assert!(toms.contains(&format!(r#"id="photo-{}""#, leos_photo.id)));
        assert!(!toms.contains("data-delete-url"), "{toms}");

        // A #general photo is a Documents file, and Documents must agree:
        // a file someone else added stays, even for its message's sender.
        let mut moved = file.clone().into_active_model();
        moved.uploaded_by_id = ActiveValue::Set(Some(team.ana_id));
        moved.update(&ctx.db).await.unwrap();
        let res = delete(&request, &team.tom, general, photo.id).await;
        assert_eq!(res.status_code(), 403);
        assert!(bytes_exist(&ctx, &file.storage_key).await);
        assert_eq!(reload(&ctx, &photo).await.document_id, Some(file_id));
        assert!(removals(&mut events).is_empty());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn people_outside_the_conversation_cannot_delete() {
    request::<App, _, _>(|request, ctx| async move {
        let team = juniper(&request, &ctx).await;
        let suppliers = group_of(
            &ctx,
            team.org_id,
            team.tom_id,
            "suppliers",
            vec![team.leo_id],
        )
        .await;
        let photo = post_photo(
            &request,
            &ctx,
            &team.tom,
            suppliers.id,
            Some("Courier invoice from Tuesday, £18.40"),
        )
        .await;
        let key = photo.photo_key.clone().unwrap();
        let finance_photo = post_photo(&request, &ctx, &team.tom, team.finance.id, None).await;
        let text = messages::Model::create(
            &ctx.db,
            &suppliers,
            team.tom_id,
            &messages::MessageParams {
                body: "Morning all".to_string(),
            },
        )
        .await
        .unwrap();
        let reply = messages::Model::create_reply(
            &ctx.db,
            &suppliers,
            &photo,
            team.leo_id,
            &messages::MessageParams {
                body: "Paid".to_string(),
            },
        )
        .await
        .unwrap();
        let mut events = hub_events(&ctx);
        let mut refused = |what: &str, res: axum_test::TestResponse, status: u16| {
            assert_eq!(res.status_code(), status, "{what}: {}", res.text());
            assert!(!leaks(res.as_bytes()), "{what}");
            assert!(removals(&mut events).is_empty(), "{what} sent an event");
        };

        // Owner and admin, not in the group: role does not matter.
        for who in [&team.ana, &team.maya] {
            let res = delete(&request, who, suppliers.id, photo.id).await;
            refused("an owner or admin outside the group", res, 404);
        }
        // Crossed ids: a message from another conversation under this one's address.
        let res = delete(&request, &team.tom, suppliers.id, finance_photo.id).await;
        refused("#finance's photo at #suppliers", res, 404);
        let res = delete(&request, &team.tom, team.finance.id, photo.id).await;
        refused("#suppliers' photo at #finance", res, 404);
        // No photo to delete.
        let res = delete(&request, &team.tom, suppliers.id, text.id).await;
        refused("a text message", res, 404);
        let res = delete(&request, &team.leo, suppliers.id, reply.id).await;
        refused("a thread reply", res, 404);

        // Signed out.
        let res = request
            .post(&team.tom.at(&delete_path(suppliers.id, photo.id)))
            .await;
        assert_eq!(res.headers()["location"], "/login");
        refused("signed out", res, 303);

        // Waiting to join: the waiting page. From a script (HTMX or fetch),
        // a 409 that moves the tab to Home, which shows it.
        let pat = join(&request, &ctx, &team.tom.slug, "pat", "pat@example.com").await;
        let res = plain_post(&request, &pat, suppliers.id, photo.id).await;
        refused("pending", res, 403);
        let res = delete(&request, &pat, suppliers.id, photo.id).await;
        refused("pending, from a script", res, 409);

        // A member of another organisation only, even claiming Juniper's tab.
        let other = sign_up_member(&request, "Other Tea", "mallory", "mallory@example.com").await;
        let forged = TestMember::new(other.cookie.clone(), team.org_id, team.tom.slug.clone());
        let res = plain_post(&request, &forged, suppliers.id, photo.id).await;
        refused("another org", res, 404);
        let res = delete(&request, &forged, suppliers.id, photo.id).await;
        refused("another org, from a script", res, 409);

        // A super admin acting in Juniper Tea Co.
        let admin = super::admin::super_admin_cookie(&request, &ctx).await;
        let acting = format!("{}; acting_org={}", admin.1.to_str().unwrap(), team.org_id);
        let res = request
            .post(&team.tom.at(&delete_path(suppliers.id, photo.id)))
            .add_header("cookie", acting)
            .add_header("x-org-id", team.org_id.to_string())
            .await;
        refused("acting", res, 403);

        // A cross-site form or fetch, with Tom's cookie.
        let res = request
            .post(&team.tom.at(&delete_path(suppliers.id, photo.id)))
            .as_member(&team.tom)
            .add_header("Origin", "https://evil.example")
            .await;
        refused("cross-site", res, 403);

        // Leo is removed from Juniper Tea Co.
        let leo_m = memberships::Model::default_for_user(&ctx.db, team.leo_id)
            .await
            .unwrap()
            .unwrap();
        memberships::Model::remove(
            &ctx,
            team.org_id,
            team.ana_id,
            leo_m.id,
            &Default::default(),
        )
        .await
        .unwrap();
        let removed = TestMember::new(
            login(&request, "leo@example.com", USER_PASSWORD).await,
            team.org_id,
            team.tom.slug.clone(),
        );
        let res = plain_post(&request, &removed, suppliers.id, photo.id).await;
        refused("removed", res, 403);
        let res = delete(&request, &removed, suppliers.id, photo.id).await;
        refused("removed, from a script", res, 409);

        // Nothing changed.
        assert_eq!(reload(&ctx, &photo).await, photo);
        assert_eq!(reload(&ctx, &finance_photo).await, finance_photo);
        assert!(bytes_exist(&ctx, &key).await);
        let res = open_photo(&request, &team.tom, suppliers.id, photo.id).await;
        assert_eq!(res.status_code(), 200);
        assert!(leaks(res.as_bytes()), "the real bytes");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn deleting_a_general_photo_deletes_the_documents_file() {
    request::<App, _, _>(|request, ctx| async move {
        let team = juniper(&request, &ctx).await;
        let general = team.general.id;
        let chat_photos = folders::Model::chat_photos(&ctx.db, team.org_id)
            .await
            .unwrap();
        let elsewhere = folders::Model::create(
            &ctx.db,
            team.org_id,
            team.ana_id,
            &folders::FolderParams {
                name: "Market stall".to_string(),
                note: String::new(),
            },
        )
        .await
        .unwrap();

        for moved in [false, true] {
            let photo = post_photo(&request, &ctx, &team.tom, general, Some("Whiteboard")).await;
            let file =
                documents::Model::find_in_org(&ctx.db, team.org_id, photo.document_id.unwrap())
                    .await
                    .unwrap();
            let folder_id = if moved {
                // Filed somewhere else in Documents: still the same file.
                let mut row = file.clone().into_active_model();
                row.folder_id = ActiveValue::Set(elsewhere.id);
                row.update(&ctx.db).await.unwrap();
                elsewhere.id
            } else {
                chat_photos.id
            };
            let res = delete(&request, &team.maya, general, photo.id).await;
            assert_eq!(res.status_code(), 200, "moved: {moved}");

            assert!(documents::Entity::find_by_id(file.id)
                .one(&ctx.db)
                .await
                .unwrap()
                .is_none());
            assert!(!bytes_exist(&ctx, &file.storage_key).await);
            assert_eq!(reload(&ctx, &photo).await.document_id, None);
            let res = request
                .get(&team.tom.at(&format!("/documents/files/{}", file.id)))
                .add_header(team.tom.cookie.0.clone(), team.tom.cookie.1.clone())
                .await;
            assert_eq!(res.status_code(), 404);
            let shown = page(
                &request,
                &team.tom,
                &format!("/documents/folders/{folder_id}"),
            )
            .await;
            assert!(!shown.contains("whiteboard-tuesday"), "{shown}");
            let feed = page(&request, &team.tom, &format!("/chat/{general}")).await;
            assert!(feed.contains("Photo removed"), "{feed}");
            assert!(!feed.contains(&photo_path(general, photo.id)));
        }
        assert_eq!(documents::Entity::find().count(&ctx.db).await.unwrap(), 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_second_delete_succeeds_and_changes_nothing() {
    request::<App, _, _>(|request, ctx| async move {
        let team = juniper(&request, &ctx).await;
        for conversation_id in [team.finance.id, team.general.id] {
            let photo = post_photo(&request, &ctx, &team.tom, conversation_id, None).await;
            let mut events = hub_events(&ctx);
            let res = delete(&request, &team.tom, conversation_id, photo.id).await;
            assert_eq!(res.status_code(), 200);
            let first = reload(&ctx, &photo).await;
            assert_eq!(removals(&mut events), vec![(conversation_id, photo.id)]);

            let res = delete(&request, &team.tom, conversation_id, photo.id).await;
            assert_eq!(res.status_code(), 200, "a retry after a lost answer");
            assert_eq!(
                reload(&ctx, &photo).await,
                first,
                "same updated_at and columns"
            );
            assert!(removals(&mut events).is_empty(), "no second event");

            // The access check still comes first: an admin's repeat is 200,
            // a member's on someone else's photo is still 403.
            let res = delete(&request, &team.maya, conversation_id, photo.id).await;
            assert_eq!(res.status_code(), 200, "an admin may");
            if conversation_id == team.general.id {
                let res = delete(&request, &team.leo, conversation_id, photo.id).await;
                assert_eq!(res.status_code(), 403, "a member may not");
            }
            assert!(removals(&mut events).is_empty());
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_list_home_and_sheet_say_photo_removed() {
    request::<App, _, _>(|request, ctx| async move {
        let team = juniper(&request, &ctx).await;
        let finance = team.finance.id;
        let photo = post_photo(&request, &ctx, &team.tom, finance, Some("Van fuel £62")).await;
        delete(&request, &team.tom, finance, photo.id).await;

        for path in ["/chat", "/dashboard"] {
            let maya = page(&request, &team.maya, path).await;
            assert!(
                maya.contains("tom: Photo removed · Van fuel £62<"),
                "{path}: {maya}"
            );
            let tom = page(&request, &team.tom, path).await;
            assert!(
                tom.contains("You: Photo removed · Van fuel £62<"),
                "{path}: {tom}"
            );
        }
        let html = sheet(&request, &team.maya, finance, photo.id, "").await;
        assert!(
            html.contains("<p>Photo removed · Van fuel £62</p>"),
            "{html}"
        );
        assert!(!html.contains("Delete photo"), "{html}");

        // With no caption.
        let dm = conversations::Model::start_dm(&ctx.db, team.org_id, team.tom_id, team.leo_id)
            .await
            .unwrap();
        let photo = post_photo(&request, &ctx, &team.tom, dm.id, None).await;
        delete(&request, &team.tom, dm.id, photo.id).await;
        let tom = page(&request, &team.tom, "/chat").await;
        assert!(tom.contains("You: Photo removed<"), "{tom}");
        let leo = page(&request, &team.leo, "/chat").await;
        assert!(leo.contains(">Photo removed<"), "{leo}");
        let html = sheet(&request, &team.tom, dm.id, photo.id, "").await;
        assert!(html.contains("<p>Photo removed</p>"), "{html}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_confirm_names_where_the_photo_goes() {
    request::<App, _, _>(|request, ctx| async move {
        let team = juniper(&request, &ctx).await;
        let finance = page(&request, &team.tom, &format!("/chat/{}", team.finance.id)).await;
        assert!(finance.contains(r#"<dialog id="photo-delete" class="sheet""#), "{finance}");
        assert!(
            finance.contains("It's removed for everyone in #finance. This can't be undone."),
            "{finance}"
        );
        assert!(!finance.contains("deleted from Documents"));
        assert!(finance.contains("data-viewer-delete"));
        assert!(finance.contains("data-viewer-removed"));

        let dm = conversations::Model::start_dm(&ctx.db, team.org_id, team.leo_id, team.ana_id)
            .await
            .unwrap();
        let leos = page(&request, &team.leo, &format!("/chat/{}", dm.id)).await;
        assert!(
            leos.contains("It's removed for you and ana. This can't be undone."),
            "{leos}"
        );

        let general = page(&request, &team.maya, &format!("/chat/{}", team.general.id)).await;
        assert!(
            general.contains(
                "It's removed for everyone in #general. It's also deleted from Documents. This can't be undone."
            ),
            "{general}"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_thread_under_a_removed_photo_still_opens() {
    request::<App, _, _>(|request, ctx| async move {
        let team = juniper(&request, &ctx).await;
        let finance = team.finance.id;
        let photo = post_photo(&request, &ctx, &team.tom, finance, Some("Van fuel £62")).await;
        messages::Model::create_reply(
            &ctx.db,
            &team.finance,
            &photo,
            team.maya_id,
            &messages::MessageParams {
                body: "Logged it in the sheet".to_string(),
            },
        )
        .await
        .unwrap();
        let html = sheet(&request, &team.tom, finance, photo.id, "?thread=1").await;
        assert!(html.contains("Delete photo"), "the parent's sheet: {html}");

        let res = delete(&request, &team.tom, finance, photo.id).await;
        assert_eq!(res.status_code(), 200);
        let thread = request
            .get(
                &team
                    .tom
                    .at(&format!("/chat/{finance}/threads/{}", photo.id)),
            )
            .add_header(team.tom.cookie.0.clone(), team.tom.cookie.1.clone())
            .await;
        assert_eq!(thread.status_code(), 200);
        let thread = thread.text();
        assert!(thread.contains("Logged it in the sheet"), "{thread}");
        assert!(thread.contains("Photo removed"), "{thread}");
        assert!(thread.contains(r#"id="photo-delete""#), "{thread}");
        assert_eq!(
            messages::Entity::find().count(&ctx.db).await.unwrap(),
            2,
            "the photo message and its reply"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_documents_delete_tells_chat() {
    request::<App, _, _>(|request, ctx| async move {
        let team = juniper(&request, &ctx).await;
        let general = team.general.id;
        let photo = post_photo(&request, &ctx, &team.tom, general, None).await;
        let file_id = photo.document_id.unwrap();
        let mut events = hub_events(&ctx);
        let res = request
            .post(&team.tom.at(&format!("/documents/files/{file_id}/delete")))
            .as_member(&team.tom)
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(removals(&mut events), vec![(general, photo.id)]);

        // An ordinary file is not a chat photo: no event.
        let folder = folders::Model::create(
            &ctx.db,
            team.org_id,
            team.ana_id,
            &folders::FolderParams {
                name: "Suppliers".to_string(),
                note: String::new(),
            },
        )
        .await
        .unwrap();
        let menu = documents::Model::upload(
            &ctx,
            &folder,
            team.tom_id,
            "menu.pdf",
            bytes::Bytes::from_static(PDF),
        )
        .await
        .unwrap();
        let res = request
            .post(&team.tom.at(&format!("/documents/files/{}/delete", menu.id)))
            .as_member(&team.tom)
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 200);
        assert!(removals(&mut events).is_empty());
    })
    .await;
}

/// Bytes are only deleted in the models, so the bytes-first order lives in
/// one place.
#[test]
fn photo_bytes_are_only_deleted_by_the_models() {
    let mut offenders = Vec::new();
    let mut dirs = vec![std::path::PathBuf::from("src")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.to_string_lossy().replace('\\', "/");
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs")
                || name == "src/models/messages.rs"
                || name == "src/models/documents.rs"
            {
                continue;
            }
            // Calls split over lines count too.
            let text: String = std::fs::read_to_string(&path)
                .unwrap()
                .split_whitespace()
                .collect();
            if text.contains("storage.delete(") {
                offenders.push(name);
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:#?}");
}
