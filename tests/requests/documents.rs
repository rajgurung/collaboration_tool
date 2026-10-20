//! Documents: folders and files. Who can add, open, rename and delete, which
//! files are refused, and the headers every file is sent with.
use axum_test::multipart::{MultipartForm, Part};
use collab::{
    app::App,
    models::{documents, folders, memberships, organisations, users},
};
use loco_rs::{app::AppContext, testing::prelude::*, TestServer};
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait, IntoActiveModel, PaginatorTrait};
use serial_test::serial;

use super::prepare_data::{approve_as_owner, join, sign_up_member, AsMember};

use super::prepare_data::TestMember;

pub const PNG: &[u8] =
    b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89";
pub const PDF: &[u8] = b"%PDF-1.7\n1 0 obj\n<<>>\nendobj\ntrailer\n<<>>\n%%EOF\n";
const DOCX: &[u8] = b"PK\x03\x04\x14\0\x06\0\x08\0\0\0!\0";

/// Acme with alice (owner), bob (member) and carol (admin).
async fn acme(
    request: &TestServer,
    ctx: &AppContext,
) -> (TestMember, TestMember, TestMember, folders::Model) {
    let alice = sign_up_member(request, "Acme", "alice", "alice@example.com").await;
    let mut people = Vec::new();
    for (name, role) in [("bob", "member"), ("carol", "admin")] {
        let email = format!("{name}@example.com");
        let cookie = join(request, ctx, "acme", name, &email).await;
        let user = users::Model::find_by_email(&ctx.db, &email).await.unwrap();
        let pending = memberships::Model::default_for_user(&ctx.db, user.id)
            .await
            .unwrap()
            .unwrap();
        let approved = approve_as_owner(ctx, &pending).await;
        let mut active = approved.into_active_model();
        active.role = ActiveValue::Set(role.to_string());
        active.update(&ctx.db).await.unwrap();
        people.push(cookie);
    }
    let org = organisations::Model::find_by_slug(&ctx.db, "acme")
        .await
        .unwrap();
    let company = folders::Model::list_for_org(&ctx.db, org.id)
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.folder.name == "Company")
        .unwrap()
        .folder;
    let carol = people.pop().unwrap();
    let bob = people.pop().unwrap();
    (alice, bob, carol, company)
}

async fn upload(
    request: &TestServer,
    cookie: &TestMember,
    folder_id: i64,
    name: &str,
    bytes: &[u8],
) -> axum_test::TestResponse {
    request
        .post(&cookie.at(&format!("/documents/folders/{folder_id}/files")))
        .as_member(cookie)
        .multipart(
            MultipartForm::new().add_part("file", Part::bytes(bytes.to_vec()).file_name(name)),
        )
        .await
}

async fn only_file(ctx: &AppContext) -> documents::Model {
    let all = documents::Entity::find().all(&ctx.db).await.unwrap();
    assert_eq!(all.len(), 1);
    all.into_iter().next().unwrap()
}

#[tokio::test]
#[serial]
async fn new_organisations_start_with_company_and_logins_folders() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob, _, company) = acme(&request, &ctx).await;
        let folder = request
            .get(&alice.at(&format!("/documents/folders/{}", company.id)))
            .add_header(alice.0.clone(), alice.1.clone())
            .await;
        assert_eq!(folder.status_code(), 200);
        assert!(folder.text().contains("Nothing in Company yet"));
        assert!(folder.text().contains(r#"hx-history="false""#));
        assert!(
            folder.text().contains("Drag files here to add them."),
            "empty folders take drops too"
        );
        let page = request
            .get(&alice.at("/documents"))
            .add_header(alice.0.clone(), alice.1.clone())
            .await;
        assert_eq!(page.status_code(), 200);
        let body = page.text();
        assert!(body.contains("Company"));
        assert!(body.contains("Logins"));
        assert!(body.contains("New folder"), "owners can add folders");
        assert_eq!(page.headers()["x-frame-options"], "DENY");
        assert_eq!(
            page.headers()["content-security-policy"],
            "frame-ancestors 'none'"
        );

        let body = request
            .get(&bob.at("/documents"))
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(!body.contains("New folder"), "members cannot");
        assert!(body.contains("Owners and admins look after folders."));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_member_adds_a_photo_and_everyone_can_open_it() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob, _, company) = acme(&request, &ctx).await;
        let res = upload(&request, &bob, company.id, "Share certificate.PNG", PNG).await;
        assert_eq!(res.status_code(), 200);
        assert!(res.headers()["hx-trigger"].to_str().unwrap().contains("Added Share certificate.png"));
        assert!(res.text().contains("Share certificate.png"));
        assert!(res.text().contains("Drag files here to add them."));

        let page = request
            .get(&bob.at(&format!("/documents/folders/{}", company.id)))
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(page.contains("data-drop-overlay hidden"), "members can drop files too");
        assert!(page.contains("Drop to add to Company"));
        let input = page.split(r#"<input type="file""#).nth(1).unwrap();
        let input = &input[..input.find('>').unwrap()];
        assert!(input.split_whitespace().any(|attr| attr == "multiple"), "{input}");

        let file = only_file(&ctx).await;
        assert_eq!(file.content_type, "image/png");
        assert_eq!(file.size_bytes, PNG.len() as i64);
        let prefix = format!("orgs/{}/docs/", company.organisation_id);
        assert!(file.storage_key.starts_with(&prefix), "{}", file.storage_key);
        assert!(!file.storage_key.contains("certificate"), "the key never uses the name");

        let res = request
            .get(&alice.at(&format!("/documents/files/{}", file.id)))
            .add_header(alice.0.clone(), alice.1.clone())
            .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(res.as_bytes().as_ref(), PNG);
        let headers = res.headers();
        assert_eq!(headers["content-type"], "image/png");
        assert_eq!(headers["content-length"], PNG.len().to_string().as_str());
        assert_eq!(
            headers["content-disposition"],
            r#"inline; filename="Share certificate.png"; filename*=UTF-8''Share%20certificate%2Epng"#
        );
        assert_eq!(headers["x-content-type-options"], "nosniff");
        assert_eq!(headers["cache-control"], "private, no-store");
        assert_eq!(headers["cross-origin-resource-policy"], "same-origin");
        assert_eq!(
            headers["content-security-policy"],
            "default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; sandbox; frame-ancestors 'none'"
        );
        assert_eq!(headers["x-frame-options"], "DENY");

        let res = request
            .get(&alice.at(&format!("/documents/files/{}?download=1", file.id)))
            .add_header(alice.0.clone(), alice.1.clone())
            .await;
        assert!(res.headers()["content-disposition"].to_str().unwrap().starts_with("attachment;"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn office_files_always_download() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _, _, company) = acme(&request, &ctx).await;
        assert_eq!(
            upload(&request, &alice, company.id, "Lease.docx", DOCX)
                .await
                .status_code(),
            200
        );
        let file = only_file(&ctx).await;
        assert_eq!(
            file.content_type,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        );
        let res = request
            .get(&alice.at(&format!("/documents/files/{}", file.id)))
            .add_header(alice.0.clone(), alice.1.clone())
            .await;
        assert!(res.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment;"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn pdfs_open_in_place_without_a_sandbox() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _, _, company) = acme(&request, &ctx).await;
        upload(&request, &alice, company.id, "Articles.pdf", PDF).await;
        let file = only_file(&ctx).await;
        let res = request
            .get(&alice.at(&format!("/documents/files/{}", file.id)))
            .add_header(alice.0.clone(), alice.1.clone())
            .await;
        assert!(res.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("inline;"));
        assert_eq!(
            res.headers()["content-security-policy"],
            "default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; frame-ancestors 'none'"
        );
        assert_eq!(res.headers()["x-content-type-options"], "nosniff");

        // The folder list counts it.
        let list = request
            .get(&alice.at("/documents"))
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(list.contains("1 file · Updated"), "{list}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn unsafe_and_unknown_files_are_refused() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _, _, company) = acme(&request, &ctx).await;
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"></svg>"#;
        let html = b"<!DOCTYPE html><html><script>alert(1)</script></html>";
        let exe = b"MZ\x90\0\x03\0\0\0\x04\0\0\0\xff\xff\0\0";
        for (name, bytes) in [
            ("logo.svg", &svg[..]),
            ("page.html", &html[..]),
            ("invoice.pdf", &exe[..]),
            ("notes.txt", &b"plain text"[..]),
        ] {
            let res = upload(&request, &alice, company.id, name, bytes).await;
            assert_eq!(res.status_code(), 422, "{name}");
            assert_eq!(res.headers()["hx-retarget"], "#uploads");
            assert!(res.text().contains("This kind of file can"), "{name}");
        }
        assert_eq!(documents::Entity::find().count(&ctx.db).await.unwrap(), 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn files_over_25_mb_are_refused_but_the_route_allows_more_than_the_global_limit() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _, _, company) = acme(&request, &ctx).await;
        // Bigger than Loco's 2 MB body limit for every other route.
        let mut three_mb = PDF.to_vec();
        three_mb.resize(3 * 1024 * 1024, b' ');
        assert_eq!(
            upload(&request, &alice, company.id, "big.pdf", &three_mb)
                .await
                .status_code(),
            200
        );

        let mut over = PDF.to_vec();
        over.resize(documents::MAX_BYTES + 1, b' ');
        let res = upload(&request, &alice, company.id, "huge.pdf", &over).await;
        assert_eq!(res.status_code(), 422);
        assert!(res.text().contains("Too big"));

        // Past the route's own body limit too.
        over.resize(27 * 1024 * 1024, b' ');
        let res = upload(&request, &alice, company.id, "huger.pdf", &over).await;
        assert_eq!(res.status_code(), 422);
        assert!(res.text().contains("Too big"));
        assert_eq!(documents::Entity::find().count(&ctx.db).await.unwrap(), 1);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn renaming_changes_the_name_but_never_the_extension() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob, _, company) = acme(&request, &ctx).await;
        upload(&request, &alice, company.id, "IMG_0001.png", PNG).await;
        let file = only_file(&ctx).await;
        let rename = |name: &'static str| {
            request
                .post(&bob.at(&format!("/documents/files/{}/rename", file.id)))
                .as_member(&bob)
                .form(&serde_json::json!({ "name": name }))
        };
        assert_eq!(rename("Office front").await.status_code(), 200);
        assert_eq!(only_file(&ctx).await.name, "Office front.png");
        rename("Office front.png").await;
        assert_eq!(only_file(&ctx).await.name, "Office front.png");
        rename("setup.exe").await;
        assert_eq!(only_file(&ctx).await.name, "setup.exe.png");
        rename("photo\u{202E}gpj").await;
        assert_eq!(only_file(&ctx).await.name, "photogpj.png");
        let res = rename("  ").await;
        assert_eq!(res.status_code(), 422);
        assert_eq!(only_file(&ctx).await.name, "photogpj.png");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn only_the_uploader_owners_and_admins_delete_files() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob, carol, company) = acme(&request, &ctx).await;
        upload(&request, &alice, company.id, "alices.pdf", PDF).await;
        let alices = only_file(&ctx).await;
        let delete = |cookie: &TestMember, id: i64| {
            request
                .post(&cookie.at(&format!("/documents/files/{id}/delete")))
                .as_member(cookie)
        };
        // A member cannot delete someone else's file, and is not offered it.
        assert_eq!(delete(&bob, alices.id).await.status_code(), 403);
        let page = request
            .get(&bob.at(&format!("/documents/folders/{}", company.id)))
            .add_header(bob.0.clone(), bob.1.clone())
            .await
            .text();
        assert!(!page.contains(&format!("/documents/files/{}/delete", alices.id)));

        // The uploader can.
        documents::Entity::delete_many()
            .exec(&ctx.db)
            .await
            .unwrap();
        upload(&request, &bob, company.id, "bobs.pdf", PDF).await;
        let bobs = only_file(&ctx).await;
        assert_eq!(delete(&bob, bobs.id).await.status_code(), 200);
        assert_eq!(documents::Entity::find().count(&ctx.db).await.unwrap(), 0);
        assert!(
            ctx.storage
                .download::<Vec<u8>>(std::path::Path::new(&bobs.storage_key))
                .await
                .is_err(),
            "the stored bytes are gone"
        );

        // So can an admin, for anyone's file.
        upload(&request, &bob, company.id, "bobs-again.pdf", PDF).await;
        let again = only_file(&ctx).await;
        let res = delete(&carol, again.id).await;
        assert_eq!(res.status_code(), 200);
        assert!(res.headers()["hx-trigger"]
            .to_str()
            .unwrap()
            .contains("Deleted bobs-again.pdf"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn owners_and_admins_manage_folders_and_only_empty_ones_go() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, bob, carol, company) = acme(&request, &ctx).await;
        let create = |cookie: &TestMember, name: &'static str| {
            request
                .post(&cookie.at("/documents/folders"))
                .as_member(cookie)
                .form(&serde_json::json!({ "name": name, "note": "Paper copies are in the blue box." }))
        };
        assert_eq!(create(&bob, "Bobs").await.status_code(), 403);
        let res = create(&carol, "Insurance").await;
        assert_eq!(res.status_code(), 303);
        let org = organisations::Model::find_by_slug(&ctx.db, "acme").await.unwrap();
        let insurance = folders::Model::list_for_org(&ctx.db, org.id)
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.folder.name == "Insurance")
            .unwrap()
            .folder;
        assert_eq!(
            res.headers()["location"],
            carol.at(&format!("/documents/folders/{}", insurance.id)).as_str()
        );
        assert_eq!(insurance.note.as_deref(), Some("Paper copies are in the blue box."));

        // Hidden characters are stripped from names and notes; line breaks in notes stay.
        let res = request
            .post(&alice.at("/documents/folders"))
            .as_member(&alice)
            .form(&serde_json::json!({ "name": "Tax\u{200B} and\u{202E} VAT\u{FEFF}", "note": "Line one\u{2028}\r\nLine two" }))
            .await;
        assert_eq!(res.status_code(), 303);
        let tax = folders::Model::list_for_org(&ctx.db, org.id)
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.folder.name.starts_with("Tax"))
            .unwrap()
            .folder;
        assert_eq!(tax.name, "Tax and VAT");
        assert_eq!(tax.note.as_deref(), Some("Line one\nLine two"));

        let res = create(&alice, "insurance").await;
        assert_eq!(res.status_code(), 422);
        assert!(res.text().contains("already a folder with that name."), "{}", res.text());

        let res = request
            .post(&alice.at(&format!("/documents/folders/{}", insurance.id)))
            .as_member(&alice)
            .form(&serde_json::json!({ "name": "Insurance policies", "note": "" }))
            .await;
        assert_eq!(res.status_code(), 303);
        let renamed = folders::Model::find_in_org(&ctx.db, org.id, insurance.id).await.unwrap();
        assert_eq!(renamed.name, "Insurance policies");
        assert_eq!(renamed.note, None);

        upload(&request, &bob, company.id, "a.pdf", PDF).await;
        let delete = |cookie: &TestMember, id: i64| {
            request
                .post(&cookie.at(&format!("/documents/folders/{id}/delete")))
                .as_member(cookie)
        };
        assert_eq!(delete(&bob, insurance.id).await.status_code(), 403);
        let res = delete(&alice, company.id).await;
        assert_eq!(res.status_code(), 422, "Company still has a file");
        assert!(res.text().contains("Only empty folders can be deleted."));
        let res = delete(&alice, insurance.id).await;
        assert_eq!(res.status_code(), 303);
        assert!(folders::Model::find_in_org(&ctx.db, org.id, insurance.id).await.is_err());
    })
    .await;
}
