//! What a plain member can reach in Documents, walked route by route. Members
//! open only the Shared folder and the logins they are named on; owners and
//! admins open everything. Refusals never name what was refused.
use axum::http::Method;
use axum_test::multipart::{MultipartForm, Part};
use collab::{
    app::App,
    models::{documents, folders, logins},
};
use loco_rs::{
    app::{AppContext, Hooks},
    testing::prelude::*,
    TestServer,
};
use sea_orm::EntityTrait;
use serial_test::serial;

use super::{
    chat_photos::{latest_message, photo_path, secret_png, send_photo},
    documents::{PDF, PNG},
    juniper::{juniper, Team},
    prepare_data::{sign_up_member, AsMember, TestMember, USER_PASSWORD},
};

/// Every `/documents` route, as `App::routes` names it. A new route fails
/// [`every_documents_route_is_walked`] until it is listed here and checked
/// below.
const WALKED: [(&str, &str); 18] = [
    ("GET", "/documents"),
    ("GET", "/documents/shared-with-you"),
    ("GET", "/documents/activity"),
    ("POST", "/documents/folders"),
    ("GET", "/documents/folders/{id}"),
    ("POST", "/documents/folders/{id}"),
    ("POST", "/documents/folders/{id}/delete"),
    ("POST", "/documents/folders/{id}/files"),
    ("POST", "/documents/folders/{id}/logins"),
    ("GET", "/documents/files/{id}"),
    ("POST", "/documents/files/{id}/rename"),
    ("POST", "/documents/files/{id}/delete"),
    ("POST", "/documents/logins/unlock"),
    ("GET", "/documents/logins/{id}"),
    ("POST", "/documents/logins/{id}"),
    ("POST", "/documents/logins/{id}/delete"),
    ("POST", "/documents/logins/{id}/reveal"),
    ("POST", "/documents/logins/{id}/copy"),
];

const FOLDER_CLOSED: &str = "Only owners and admins can see this folder.";
const LOGIN_CLOSED: &str = "Only owners, admins and people named on this login can see it.";
const NOT_YOURS: &str = "Only the person who added a file, owners and admins can change it.";

/// Juniper's Documents: Shared holds Tom's receipt and Leo's price list;
/// Company holds Ana's lease and Tom's old rota. Ana names Tom on the
/// courier login, not on the bank one.
struct Docs {
    team: Team,
    shared: folders::Model,
    company: folders::Model,
    logins: folders::Model,
    toms_receipt: i64,
    leos_list: i64,
    anas_lease: i64,
    toms_rota: i64,
    courier: i64,
    bank: i64,
}

async fn add_file(ctx: &AppContext, folder: &folders::Model, user_id: i64, name: &str) -> i64 {
    documents::Model::upload(ctx, folder, user_id, name, bytes::Bytes::from_static(PDF))
        .await
        .unwrap()
        .id
}

async fn add_login(
    request: &TestServer,
    ctx: &AppContext,
    owner: &TestMember,
    folder_id: i64,
    title: &str,
    access: Option<i64>,
) -> i64 {
    let mut form = vec![
        ("title", title.to_string()),
        ("username", format!("{}@example.com", title.to_lowercase())),
        ("password", "example-pass-123".to_string()),
    ];
    if let Some(id) = access {
        form.push(("access", id.to_string()));
    }
    let res = request
        .post(&owner.at(&format!("/documents/folders/{folder_id}/logins")))
        .as_member(owner)
        .form(&form)
        .await;
    assert_eq!(res.status_code(), 303, "{}", res.text());
    logins::Entity::find()
        .all(&ctx.db)
        .await
        .unwrap()
        .into_iter()
        .find(|l| l.title == title)
        .unwrap()
        .id
}

async fn docs(request: &TestServer, ctx: &AppContext) -> Docs {
    let team = juniper(request, ctx).await;
    let all = folders::Model::list_for_org(&ctx.db, team.org_id)
        .await
        .unwrap();
    let named = |name: &str| {
        all.iter()
            .find(|s| s.folder.name == name)
            .unwrap()
            .folder
            .clone()
    };
    let (shared, company, logins) = (named("Shared"), named("Company"), named("Logins"));
    assert!(shared.is_shared());
    let toms_receipt = add_file(ctx, &shared, team.tom_id, "Receipt from the market.pdf").await;
    let leos_list = add_file(ctx, &shared, team.leo_id, "Leo price list.pdf").await;
    let anas_lease = add_file(ctx, &company, team.ana_id, "Shop lease.pdf").await;
    let toms_rota = add_file(ctx, &company, team.tom_id, "Old rota.pdf").await;
    let courier = add_login(
        request,
        ctx,
        &team.ana,
        logins.id,
        "Courier",
        Some(team.tom_id),
    )
    .await;
    let bank = add_login(request, ctx, &team.ana, logins.id, "Bank", None).await;
    Docs {
        team,
        shared,
        company,
        logins,
        toms_receipt,
        leos_list,
        anas_lease,
        toms_rota,
        courier,
        bank,
    }
}

async fn get(request: &TestServer, who: &TestMember, path: &str) -> axum_test::TestResponse {
    request
        .get(&who.at(path))
        .add_header(who.0.clone(), who.1.clone())
        .add_header("accept", "text/html")
        .await
}

async fn get_hx(request: &TestServer, who: &TestMember, path: &str) -> axum_test::TestResponse {
    request
        .get(&who.at(path))
        .as_member(who)
        .add_header("hx-request", "true")
        .await
}

async fn post(
    request: &TestServer,
    who: &TestMember,
    path: &str,
    form: &[(&str, &str)],
) -> axum_test::TestResponse {
    request
        .post(&who.at(path))
        .as_member(who)
        .add_header("hx-request", "true")
        .form(&form)
        .await
}

async fn upload(request: &TestServer, who: &TestMember, folder_id: i64) -> axum_test::TestResponse {
    request
        .post(&who.at(&format!("/documents/folders/{folder_id}/files")))
        .as_member(who)
        .add_header("hx-request", "true")
        .multipart(
            MultipartForm::new().add_part("file", Part::bytes(PNG.to_vec()).file_name("till.png")),
        )
        .await
}

async fn unlock(request: &TestServer, who: &TestMember, login_id: &str) -> axum_test::TestResponse {
    let mut form = vec![("password", USER_PASSWORD)];
    if !login_id.is_empty() {
        form.push(("login_id", login_id));
    }
    post(request, who, "/documents/logins/unlock", &form).await
}

/// An HTMX success that moves the page on.
fn moved_on(res: &axum_test::TestResponse) {
    assert_eq!(res.status_code(), 200, "{}", res.text());
    assert!(res.headers().contains_key("hx-redirect"));
}

/// A refusal that says why and names nothing in Documents.
fn refused(res: &axum_test::TestResponse, message: &str) {
    assert_eq!(res.status_code(), 403, "{}", res.text());
    let body = res.text();
    let toast = res
        .headers()
        .get("hx-trigger")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    assert!(
        body.contains(message) || toast.contains(message),
        "{body} {toast}"
    );
    for name in [
        "Company",
        "Logins",
        "Shop lease",
        "Old rota",
        "Courier",
        "Bank",
        "Leo price",
    ] {
        assert!(!body.contains(name), "{name} in {body}");
        assert!(!toast.contains(name), "{name} in {toast}");
    }
}

#[tokio::test]
#[serial]
async fn every_documents_route_is_walked() {
    request::<App, _, _>(|_request, ctx| async move {
        let mut found: Vec<(String, String)> = Vec::new();
        for route in App::routes(&ctx).collect() {
            if !route.uri.starts_with("/documents") {
                continue;
            }
            for method in route.actions {
                found.push((
                    method.to_string(),
                    route.uri.trim_end_matches('/').to_string(),
                ));
            }
        }
        found.sort();
        let mut walked: Vec<(String, String)> = WALKED
            .iter()
            .map(|(m, u)| ((*m).to_string(), (*u).to_string()))
            .collect();
        walked.sort();
        assert_eq!(found, walked);
        assert!(WALKED
            .iter()
            .all(|(m, _)| *m == Method::GET.as_str() || *m == Method::POST.as_str()));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn members_see_shared_and_the_logins_they_are_named_on() {
    request::<App, _, _>(|request, ctx| async move {
        let d = docs(&request, &ctx).await;
        let (tom, maya, ana) = (&d.team.tom, &d.team.maya, &d.team.ana);

        // GET /documents: two rows for Tom, every folder for managers.
        let page = get(&request, tom, "/documents").await;
        assert_eq!(page.status_code(), 200);
        let body = page.text();
        assert!(
            body.contains("Shared with you") && body.contains("1 login"),
            "{body}"
        );
        assert!(body.contains(&format!("/documents/folders/{}", d.shared.id)));
        for hidden in ["Company", ">Logins<", "Shop lease"] {
            assert!(!body.contains(hidden), "{hidden}");
        }
        for who in [maya, ana] {
            let body = get(&request, who, "/documents").await.text();
            assert!(body.contains("Company") && body.contains("Shared") && body.contains("Logins"));
            assert!(!body.contains("Shared with you"));
        }

        // GET /documents/shared-with-you
        let res = get(&request, tom, "/documents/shared-with-you").await;
        assert_eq!(res.status_code(), 200);
        assert!(res.text().contains("Courier"));
        assert!(!res.text().contains("Bank"));
        assert!(!res.text().contains("data-upload"), "nothing to add here");
        let leo = get(&request, &d.team.leo, "/documents/shared-with-you")
            .await
            .text();
        assert!(leo.contains("No logins shared with you"));
        for who in [maya, ana] {
            let res = get(&request, who, "/documents/shared-with-you").await;
            assert_eq!(res.status_code(), 303);
        }

        // GET /documents/activity: owners only, as before.
        assert_eq!(
            get(&request, tom, "/documents/activity")
                .await
                .status_code(),
            403
        );
        assert_eq!(
            get(&request, maya, "/documents/activity")
                .await
                .status_code(),
            403
        );
        assert_eq!(
            get(&request, ana, "/documents/activity")
                .await
                .status_code(),
            200
        );

        // GET /documents/folders/{id}: Shared for all; the rest for managers.
        let res = get(
            &request,
            tom,
            &format!("/documents/folders/{}", d.shared.id),
        )
        .await;
        assert_eq!(res.status_code(), 200);
        assert!(res.text().contains("Leo price list.pdf"));
        for folder in [&d.company, &d.logins] {
            let path = format!("/documents/folders/{}", folder.id);
            let res = get(&request, tom, &path).await;
            refused(&res, FOLDER_CLOSED);
            assert!(res.text().contains("Back to Documents"), "the 403 page");
            refused(&get_hx(&request, tom, &path).await, FOLDER_CLOSED);
            for who in [maya, ana] {
                assert_eq!(get(&request, who, &path).await.status_code(), 200);
            }
        }

        // GET /documents/files/{id}
        let file = |id: i64| format!("/documents/files/{id}");
        assert_eq!(
            get(&request, tom, &file(d.leos_list)).await.status_code(),
            200
        );
        for id in [d.anas_lease, d.toms_rota] {
            refused(&get(&request, tom, &file(id)).await, FOLDER_CLOSED);
            assert_eq!(get(&request, maya, &file(id)).await.status_code(), 200);
        }

        // GET /documents/logins/{id}, page and side sheet.
        let login = |id: i64| format!("/documents/logins/{id}");
        let res = get(&request, tom, &login(d.courier)).await;
        assert_eq!(res.status_code(), 200);
        assert!(res.text().contains("courier@example.com"));
        assert!(
            !res.text().contains("Company"),
            "no folder names for members"
        );
        assert_eq!(
            get_hx(&request, tom, &login(d.courier)).await.status_code(),
            200
        );
        refused(&get(&request, tom, &login(d.bank)).await, LOGIN_CLOSED);
        refused(&get_hx(&request, tom, &login(d.bank)).await, LOGIN_CLOSED);
        for who in [maya, ana] {
            assert_eq!(get(&request, who, &login(d.bank)).await.status_code(), 200);
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn members_add_to_shared_and_change_only_their_own_files_there() {
    request::<App, _, _>(|request, ctx| async move {
        let d = docs(&request, &ctx).await;
        let (tom, maya, ana) = (&d.team.tom, &d.team.maya, &d.team.ana);
        let rename = |id: i64| format!("/documents/files/{id}/rename");
        let delete = |id: i64| format!("/documents/files/{id}/delete");

        // POST /documents/folders/{id}/files
        assert_eq!(upload(&request, tom, d.shared.id).await.status_code(), 200);
        let res = upload(&request, tom, d.company.id).await;
        assert_eq!(res.status_code(), 403);
        assert!(res
            .text()
            .contains("Only owners and admins can add to this folder."));
        for who in [maya, ana] {
            assert_eq!(upload(&request, who, d.shared.id).await.status_code(), 200);
            assert_eq!(upload(&request, who, d.company.id).await.status_code(), 200);
        }

        // Someone else's file in Shared: refused for Tom, not offered.
        let name = [("name", "Mine now")];
        refused(
            &post(&request, tom, &rename(d.leos_list), &name).await,
            NOT_YOURS,
        );
        refused(
            &post(&request, tom, &delete(d.leos_list), &[]).await,
            NOT_YOURS,
        );
        let page = get(
            &request,
            tom,
            &format!("/documents/folders/{}", d.shared.id),
        )
        .await
        .text();
        assert!(!page.contains(&rename(d.leos_list)));
        assert!(!page.contains(&delete(d.leos_list)));
        assert!(page.contains(&rename(d.toms_receipt)));

        // Outside Shared, even his own old file is managers only now.
        for id in [d.anas_lease, d.toms_rota] {
            refused(
                &post(&request, tom, &rename(id), &name).await,
                FOLDER_CLOSED,
            );
            refused(&post(&request, tom, &delete(id), &[]).await, FOLDER_CLOSED);
        }
        let names: Vec<String> = documents::Entity::find()
            .all(&ctx.db)
            .await
            .unwrap()
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert!(
            !names.iter().any(|n| n.starts_with("Mine now")),
            "{names:?}"
        );

        // His own file in Shared.
        let res = post(
            &request,
            tom,
            &rename(d.toms_receipt),
            &[("name", "Market receipt")],
        )
        .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(
            post(&request, tom, &delete(d.toms_receipt), &[])
                .await
                .status_code(),
            200
        );

        // Managers change anyone's, anywhere.
        let res = post(&request, maya, &rename(d.leos_list), &[("name", "Prices")]).await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(
            post(&request, maya, &delete(d.leos_list), &[])
                .await
                .status_code(),
            200
        );
        assert_eq!(
            post(&request, ana, &rename(d.toms_rota), &name)
                .await
                .status_code(),
            200
        );
        assert_eq!(
            post(&request, ana, &delete(d.anas_lease), &[])
                .await
                .status_code(),
            200
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn built_in_folders_stay_and_only_managers_change_folders() {
    request::<App, _, _>(|request, ctx| async move {
        let d = docs(&request, &ctx).await;
        let (tom, maya, ana) = (&d.team.tom, &d.team.maya, &d.team.ana);
        let edit = |id: i64| format!("/documents/folders/{id}");
        let delete = |id: i64| format!("/documents/folders/{id}/delete");

        // POST /documents/folders
        let new = [("name", "Suppliers"), ("note", "")];
        assert_eq!(
            post(&request, tom, "/documents/folders", &new)
                .await
                .status_code(),
            403
        );
        moved_on(&post(&request, maya, "/documents/folders", &new).await);

        // POST /documents/folders/{id} and /delete
        for id in [d.shared.id, d.company.id] {
            let res = post(&request, tom, &edit(id), &[("name", "Mine"), ("note", "")]).await;
            assert_eq!(res.status_code(), 403);
            assert_eq!(
                post(&request, tom, &delete(id), &[]).await.status_code(),
                403
            );
        }
        for who in [maya, ana] {
            for folder in [&d.shared, &d.logins] {
                let res = post(
                    &request,
                    who,
                    &edit(folder.id),
                    &[("name", "Papers"), ("note", "")],
                )
                .await;
                assert_eq!(res.status_code(), 422);
                assert!(res.text().contains("Built-in folders keep their name."));
                let res = post(&request, who, &delete(folder.id), &[]).await;
                assert_eq!(res.status_code(), 422);
                assert!(res.text().contains("is built in, so it can't be deleted."));
            }
        }
        let res = post(
            &request,
            maya,
            &edit(d.company.id),
            &[("name", "Shared"), ("note", "")],
        )
        .await;
        assert_eq!(res.status_code(), 422, "the name is taken");
        let res = post(
            &request,
            maya,
            &edit(d.shared.id),
            &[("name", "Shared"), ("note", "Receipts here")],
        )
        .await;
        moved_on(&res);
        let shared = folders::Model::shared(&ctx.db, d.team.org_id)
            .await
            .unwrap();
        assert_eq!(
            shared.note.as_deref(),
            Some("Receipts here"),
            "the note still changes"
        );
        let page = get(&request, maya, &edit(d.shared.id)).await.text();
        assert!(page.contains("Built-in folders keep their name."));
        assert!(page.contains("Everyone in Juniper Tea Co can see this folder and add to it."));
        assert!(!page.contains("Delete folder"));
        let page = get(&request, maya, &edit(d.company.id)).await.text();
        assert!(page.contains("Only owners and admins can see this folder."));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn members_reach_only_the_logins_they_are_named_on() {
    request::<App, _, _>(|request, ctx| async move {
        let d = docs(&request, &ctx).await;
        let (tom, maya, ana) = (&d.team.tom, &d.team.maya, &d.team.ana);

        // POST /documents/folders/{id}/logins
        let login = [("title", "Wholesaler"), ("password", "example-pass-123")];
        let into = |id: i64| format!("/documents/folders/{id}/logins");
        assert_eq!(
            post(&request, tom, &into(d.logins.id), &login)
                .await
                .status_code(),
            403
        );
        let res = post(&request, maya, &into(d.shared.id), &login).await;
        assert_eq!(res.status_code(), 422);
        assert!(
            res.text()
                .contains("t go in Shared. Choose another folder."),
            "{}",
            res.text()
        );
        moved_on(&post(&request, maya, &into(d.logins.id), &login).await);

        // POST /documents/logins/{id} and /delete
        let change = [("title", "Courier"), ("folder_id", "")];
        for id in [d.courier, d.bank] {
            assert_eq!(
                post(&request, tom, &format!("/documents/logins/{id}"), &change)
                    .await
                    .status_code(),
                403
            );
            assert_eq!(
                post(
                    &request,
                    tom,
                    &format!("/documents/logins/{id}/delete"),
                    &[]
                )
                .await
                .status_code(),
                403
            );
        }
        let shared_id = d.shared.id.to_string();
        let res = post(
            &request,
            ana,
            &format!("/documents/logins/{}", d.bank),
            &[("title", "Bank"), ("folder_id", shared_id.as_str())],
        )
        .await;
        assert_eq!(res.status_code(), 422);
        assert!(res.text().contains("t go in Shared."));
        assert_eq!(
            logins::Model::find_in_org(&ctx.db, d.team.org_id, d.bank)
                .await
                .unwrap()
                .folder_id,
            d.logins.id
        );

        // POST /documents/logins/unlock: none, another's, another org's.
        let bea = sign_up_member(&request, "Birch Bakery", "bea", "bea@example.com").await;
        let birch_logins = folders::Model::list_for_org(&ctx.db, bea.org_id)
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.folder.is_logins())
            .unwrap()
            .folder;
        let elsewhere = add_login(&request, &ctx, &bea, birch_logins.id, "Flour", None).await;
        for id in [String::new(), d.bank.to_string(), elsewhere.to_string()] {
            let res = unlock(&request, tom, &id).await;
            assert_eq!(res.status_code(), 403);
            assert_eq!(res.text(), LOGIN_CLOSED);
        }
        let res = post(
            &request,
            maya,
            "/documents/logins/unlock",
            &[("password", USER_PASSWORD)],
        )
        .await;
        assert_eq!(res.status_code(), 200, "managers as before");

        // POST /documents/logins/{id}/reveal and /copy
        assert_eq!(
            unlock(&request, tom, &d.courier.to_string())
                .await
                .status_code(),
            200
        );
        for action in ["reveal", "copy"] {
            let res = post(
                &request,
                tom,
                &format!("/documents/logins/{}/{action}", d.bank),
                &[],
            )
            .await;
            assert_eq!(res.status_code(), 403, "{action}");
            assert!(!res.text().contains("example-pass-123"));
            let res = post(
                &request,
                tom,
                &format!("/documents/logins/{}/{action}", d.courier),
                &[],
            )
            .await;
            assert_eq!(res.status_code(), 200, "{action}");
            assert!(res.text().contains("example-pass-123"));
        }
        for action in ["reveal", "copy"] {
            let res = post(
                &request,
                maya,
                &format!("/documents/logins/{}/{action}", d.bank),
                &[],
            )
            .await;
            assert_eq!(res.status_code(), 200, "{action}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn members_delete_their_general_photos_through_chat_not_documents() {
    request::<App, _, _>(|request, ctx| async move {
        let d = docs(&request, &ctx).await;
        let general = d.team.general.id;
        let res = send_photo(
            &request,
            &d.team.tom,
            general,
            None,
            "stall.png",
            &secret_png(),
        )
        .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let photo = latest_message(&ctx).await;
        let file = photo.document_id.unwrap();

        let res = post(
            &request,
            &d.team.tom,
            &format!("/documents/files/{file}/delete"),
            &[],
        )
        .await;
        refused(&res, FOLDER_CLOSED);
        let res = post(
            &request,
            &d.team.tom,
            &format!("{}/delete", photo_path(general, photo.id)),
            &[],
        )
        .await;
        assert_eq!(res.status_code(), 200);
        assert!(documents::Entity::find_by_id(file)
            .one(&ctx.db)
            .await
            .unwrap()
            .is_none());
    })
    .await;
}
