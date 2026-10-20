//! Logins: who can add, see and copy them, the app password unlock and its
//! lock, the activity log, and what the database holds.
use collab::{
    app::App,
    extractors::current_member::CurrentMember,
    models::{
        folders, login_events, logins, memberships, org_keys, organisations, secrets::crypto::Kek,
        users,
    },
};
use loco_rs::{app::AppContext, testing::prelude::*, TestServer};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, EntityTrait, IntoActiveModel, PaginatorTrait,
};
use serial_test::serial;

use super::prepare_data::{join, login, sign_up_member, AsMember, USER_PASSWORD};

use super::prepare_data::TestMember;

const SECRET: &str = "example-pass-123";
const CODES: &str = "alpha-1111\nbravo-2222";

/// Acme with alice (owner), bob (member), carol (admin) and dave (member),
/// and its Logins folder.
struct Acme {
    alice: TestMember,
    bob: TestMember,
    carol: TestMember,
    dave: TestMember,
    org: organisations::Model,
    folder: folders::Model,
}

async fn user_id(ctx: &AppContext, email: &str) -> i64 {
    users::Model::find_by_email(&ctx.db, email)
        .await
        .unwrap()
        .id
}

async fn acme(request: &TestServer, ctx: &AppContext) -> Acme {
    let alice = sign_up_member(request, "Acme", "alice", "alice@example.com").await;
    let mut people = Vec::new();
    for (name, role) in [("bob", "member"), ("carol", "admin"), ("dave", "member")] {
        let email = format!("{name}@example.com");
        let cookie = join(request, ctx, "acme", name, &email).await;
        let id = user_id(ctx, &email).await;
        let pending = memberships::Model::default_for_user(&ctx.db, id)
            .await
            .unwrap()
            .unwrap();
        let mut active = pending
            .approve(&ctx.db, id)
            .await
            .unwrap()
            .into_active_model();
        active.role = ActiveValue::Set(role.to_string());
        active.update(&ctx.db).await.unwrap();
        people.push(cookie);
    }
    let org = organisations::Model::find_by_slug(&ctx.db, "acme")
        .await
        .unwrap();
    let folder = folders::Model::list_for_org(&ctx.db, org.id)
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.folder.name == "Logins")
        .unwrap()
        .folder;
    assert_eq!(
        folder.note.as_deref(),
        Some("Shared logins for the team. Never put passwords in notes.")
    );
    let dave = people.pop().unwrap();
    let carol = people.pop().unwrap();
    let bob = people.pop().unwrap();
    Acme {
        alice,
        bob,
        carol,
        dave,
        org,
        folder,
    }
}

/// Adds "Companies House" naming dave, as `cookie`.
async fn add(
    request: &TestServer,
    cookie: &TestMember,
    folder_id: i64,
    extra: &[(&str, String)],
) -> axum_test::TestResponse {
    let mut form: Vec<(&str, String)> = vec![
        ("title", "Companies House".to_string()),
        ("username", "office@example.com".to_string()),
        (
            "url",
            "find-and-update.company-information.service.gov.uk".to_string(),
        ),
        (
            "notes",
            "Authentication code is in the blue box file.".to_string(),
        ),
        ("password", SECRET.to_string()),
        ("recovery", CODES.to_string()),
    ];
    for (key, value) in extra {
        form.retain(|(k, _)| k != key || *key == "access");
        form.push((key, value.clone()));
    }
    request
        .post(&format!("/documents/folders/{folder_id}/logins"))
        .as_member(cookie)
        .form(&form)
        .await
}

async fn only_login(ctx: &AppContext) -> logins::Model {
    let all = logins::Entity::find().all(&ctx.db).await.unwrap();
    assert_eq!(all.len(), 1);
    all.into_iter().next().unwrap()
}

async fn post(
    request: &TestServer,
    cookie: &TestMember,
    path: &str,
    form: &[(&str, &str)],
) -> axum_test::TestResponse {
    request.post(path).as_member(cookie).form(&form).await
}

async fn unlock(
    request: &TestServer,
    cookie: &TestMember,
    password: &str,
) -> axum_test::TestResponse {
    post(
        request,
        cookie,
        "/documents/logins/unlock",
        &[("password", password)],
    )
    .await
}

async fn reveal(request: &TestServer, cookie: &TestMember, id: i64) -> axum_test::TestResponse {
    post(
        request,
        cookie,
        &format!("/documents/logins/{id}/reveal"),
        &[("field", "password")],
    )
    .await
}

async fn events(ctx: &AppContext, action: &str) -> Vec<login_events::Model> {
    login_events::Entity::find()
        .all(&ctx.db)
        .await
        .unwrap()
        .into_iter()
        .filter(|e| e.action == action)
        .collect()
}

fn deliveries(ctx: &AppContext) -> Vec<String> {
    ctx.mailer
        .as_ref()
        .unwrap()
        .deliveries()
        .messages
        .iter()
        .map(|m| m.replace("=\r\n", ""))
        .collect()
}

#[tokio::test]
#[serial]
async fn owners_add_logins_and_the_database_holds_no_plaintext() {
    request::<App, _, _>(|request, ctx| async move {
        let acme = acme(&request, &ctx).await;
        let dave_id = user_id(&ctx, "dave@example.com").await;
        let res = add(
            &request,
            &acme.alice,
            acme.folder.id,
            &[("access", dave_id.to_string())],
        )
        .await;
        assert_eq!(res.status_code(), 303, "{}", res.text());
        let login = only_login(&ctx).await;
        assert_eq!(
            res.headers()["location"],
            format!("/documents/logins/{}", login.id).as_str()
        );
        assert_eq!(
            login.url,
            "https://find-and-update.company-information.service.gov.uk/"
        );
        assert!(login.has_recovery());

        // No column of any login or event holds the password or the codes.
        for table in ["logins", "login_events", "login_accesses", "org_keys"] {
            let rows = ctx
                .db
                .query_all_raw(sea_orm::Statement::from_string(
                    sea_orm::DbBackend::Postgres,
                    format!("SELECT row_to_json(t)::text AS row FROM {table} t"),
                ))
                .await
                .unwrap();
            for row in rows {
                let text: String = row.try_get("", "row").unwrap();
                assert!(!text.contains(SECRET), "{table}: {text}");
                assert!(!text.contains("alpha-1111"), "{table}: {text}");
            }
        }
        let bytes = |b: &[u8], needle: &[u8]| b.windows(needle.len()).any(|w| w == needle);
        assert!(!bytes(&login.password_ct, SECRET.as_bytes()));
        assert!(!bytes(login.recovery_ct.as_ref().unwrap(), b"alpha"));
        assert_eq!(events(&ctx, "create").await.len(), 1);

        // The folder lists it as a login, with its username and never the password.
        let page = request
            .get(&format!("/documents/folders/{}", acme.folder.id))
            .add_header(acme.bob.0.clone(), acme.bob.1.clone())
            .await
            .text();
        assert!(page.contains("Companies House"));
        assert!(page.contains("Login · office@example.com"));
        assert!(page.contains("1 login"));
        assert!(!page.contains(SECRET));
        let list = request
            .get("/documents")
            .add_header(acme.bob.0.clone(), acme.bob.1.clone())
            .await
            .text();
        assert!(list.contains("1 login · Updated"), "{list}");

        // Members cannot add or change logins.
        assert_eq!(
            add(&request, &acme.bob, acme.folder.id, &[])
                .await
                .status_code(),
            403
        );
        let res = post(
            &request,
            &acme.dave,
            &format!("/documents/logins/{}", login.id),
            &[("title", "Mine now")],
        )
        .await;
        assert_eq!(res.status_code(), 403);
        let res = post(
            &request,
            &acme.dave,
            &format!("/documents/logins/{}/delete", login.id),
            &[],
        )
        .await;
        assert_eq!(res.status_code(), 403);
        assert_eq!(logins::Entity::find().count(&ctx.db).await.unwrap(), 1);

        // An admin changes the plain fields; the password stays when left empty.
        let res = post(
            &request,
            &acme.carol,
            &format!("/documents/logins/{}", login.id),
            &[
                ("title", "Companies House (WebFiling)"),
                ("username", "office@example.com"),
                ("url", "https://example.com"),
                ("notes", ""),
                ("password", ""),
                ("recovery", ""),
            ],
        )
        .await;
        assert_eq!(res.status_code(), 303, "{}", res.text());
        let edited = only_login(&ctx).await;
        assert_eq!(edited.title, "Companies House (WebFiling)");
        assert_eq!(edited.password_ct, login.password_ct);
        assert!(edited.has_recovery());
        let kek = Kek::get(&ctx).unwrap();
        assert_eq!(
            secrecy::ExposeSecret::expose_secret(
                &edited.reveal(&ctx.db, &kek, "password").await.unwrap()
            ),
            SECRET
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn javascript_urls_and_people_from_other_organisations_are_refused() {
    request::<App, _, _>(|request, ctx| async move {
        let acme = acme(&request, &ctx).await;
        sign_up_member(&request, "Globex", "gina", "gina@example.com").await;
        let gina = user_id(&ctx, "gina@example.com").await;

        let res = add(
            &request,
            &acme.alice,
            acme.folder.id,
            &[("url", "javascript:alert(document.cookie)".to_string())],
        )
        .await;
        assert_eq!(res.status_code(), 422);
        assert!(res.text().contains("starting with http:// or https://"));
        assert!(
            !res.text().contains(SECRET),
            "the password is never sent back"
        );

        let res = add(
            &request,
            &acme.alice,
            acme.folder.id,
            &[("access", gina.to_string())],
        )
        .await;
        assert_eq!(res.status_code(), 422);
        assert!(res
            .text()
            .contains("Choose people who are members of your organisation."));

        let res = add(
            &request,
            &acme.alice,
            acme.folder.id,
            &[("password", String::new())],
        )
        .await;
        assert_eq!(res.status_code(), 422);
        assert!(res.text().contains("Enter the password."));
        assert_eq!(logins::Entity::find().count(&ctx.db).await.unwrap(), 0);
        assert_eq!(events(&ctx, "create").await.len(), 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn showing_a_password_needs_an_unlock_from_the_same_session() {
    request::<App, _, _>(|request, ctx| async move {
        let acme = acme(&request, &ctx).await;
        add(&request, &acme.alice, acme.folder.id, &[]).await;
        let id = only_login(&ctx).await.id;

        let res = reveal(&request, &acme.alice, id).await;
        assert_eq!(res.status_code(), 423);
        assert!(!res.text().contains(SECRET));
        assert!(events(&ctx, "reveal").await.is_empty());

        let res = unlock(&request, &acme.alice, "wrong-password").await;
        assert_eq!(res.status_code(), 422);
        assert_eq!(
            res.text(),
            "That's not your password. 4 more tries, then your account is locked."
        );
        assert_eq!(
            unlock(&request, &acme.alice, USER_PASSWORD)
                .await
                .status_code(),
            200
        );

        let res = reveal(&request, &acme.alice, id).await;
        assert_eq!(res.status_code(), 200);
        assert!(res.text().contains(SECRET));
        assert_eq!(res.headers()["cache-control"], "no-store");
        assert_eq!(res.headers()["x-frame-options"], "DENY");
        let seen = events(&ctx, "reveal").await;
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].login_title.as_deref(), Some("Companies House"));
        assert_eq!(seen[0].username, "alice");

        let res = post(
            &request,
            &acme.alice,
            &format!("/documents/logins/{id}/copy"),
            &[("field", "recovery")],
        )
        .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(res.text(), CODES);
        assert_eq!(res.headers()["cache-control"], "no-store");
        assert_eq!(res.headers()["x-content-type-options"], "nosniff");
        assert_eq!(events(&ctx, "copy").await.len(), 1);

        // The owner hears about each one; the email never carries the secret.
        let mail = deliveries(&ctx);
        let told: Vec<&String> = mail
            .iter()
            .filter(|m| m.contains("alice saw the password for Companies House"))
            .collect();
        assert_eq!(told.len(), 1, "{mail:?}");
        assert!(mail
            .iter()
            .any(|m| m.contains("alice copied the recovery codes for Companies House")));
        assert!(mail
            .iter()
            .all(|m| !m.contains(SECRET) && !m.contains("alpha-1111")));

        // Another sign-in of the same person is not unlocked.
        let other = login(&request, "alice@example.com", USER_PASSWORD).await;
        let other = TestMember::new(other, acme.alice.org_id);
        assert_eq!(reveal(&request, &other, id).await.status_code(), 423);

        // The unlock runs out.
        let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();
        let mut active = alice.clone().into_active_model();
        active.secrets_unlocked_until = ActiveValue::Set(Some(
            (chrono::Utc::now() - chrono::Duration::seconds(1)).into(),
        ));
        active.update(&ctx.db).await.unwrap();
        assert_eq!(reveal(&request, &acme.alice, id).await.status_code(), 423);

        // Signing out everywhere ends it too.
        assert_eq!(
            unlock(&request, &acme.alice, USER_PASSWORD)
                .await
                .status_code(),
            200
        );
        let res = post(&request, &acme.alice, "/account/sign-out-everywhere", &[]).await;
        let fresh = TestMember::new(
            super::prepare_data::cookie_from(&res.headers().clone()),
            acme.alice.org_id,
        );
        assert_eq!(reveal(&request, &fresh, id).await.status_code(), 423);
        assert_eq!(events(&ctx, "sign_out_everywhere").await.len(), 1);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn only_owners_admins_and_named_members_can_see_secrets() {
    request::<App, _, _>(|request, ctx| async move {
        let acme = acme(&request, &ctx).await;
        let dave_id = user_id(&ctx, "dave@example.com").await;
        add(&request, &acme.alice, acme.folder.id, &[("access", dave_id.to_string())]).await;
        let id = only_login(&ctx).await.id;

        // bob is not named: he sees the entry, not the password.
        let page = request
            .get(&format!("/documents/logins/{id}"))
            .add_header(acme.bob.0.clone(), acme.bob.1.clone())
            .await
            .text();
        assert!(page.contains("office@example.com"));
        assert!(page.contains("Only owners, admins and people named on this login can see the password. Ask alice if you need it."));
        assert!(!page.contains("data-secret-show"));
        assert_eq!(unlock(&request, &acme.bob, USER_PASSWORD).await.status_code(), 200);
        let res = reveal(&request, &acme.bob, id).await;
        assert_eq!(res.status_code(), 403);
        assert!(!res.text().contains(SECRET));
        let res = post(&request, &acme.bob, &format!("/documents/logins/{id}/copy"), &[]).await;
        assert_eq!(res.status_code(), 403);

        for cookie in [&acme.carol, &acme.dave] {
            assert_eq!(unlock(&request, cookie, USER_PASSWORD).await.status_code(), 200);
            let res = reveal(&request, cookie, id).await;
            assert_eq!(res.status_code(), 200);
            assert!(res.text().contains(SECRET));
        }
        assert_eq!(events(&ctx, "reveal").await.len(), 2);

        // A bearer token is not a session, even for an owner.
        let alice_id = user_id(&ctx, "alice@example.com").await;
        let membership = memberships::Model::default_for_user(&ctx.db, alice_id).await.unwrap().unwrap();
        let alice = users::Model::find_by_id(&ctx.db, alice_id).await.unwrap();
        let bearer = CurrentMember::from_membership(alice, acme.org.clone(), membership);
        let login = only_login(&ctx).await;
        assert!(!login.can_reveal(&ctx.db, &bearer).await.unwrap());
        let token = super::mcp::token_for(&ctx, "alice@example.com").await;
        let res = request
            .post(&format!("/documents/logins/{id}/reveal"))
            .add_header("authorization", format!("Bearer {token}"))
            .await;
        assert_eq!(res.status_code(), 303, "bearer tokens never reach logins");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn no_secret_leaves_unless_its_log_entry_was_written() {
    request::<App, _, _>(|request, ctx| async move {
        let acme = acme(&request, &ctx).await;
        add(&request, &acme.alice, acme.folder.id, &[]).await;
        let id = only_login(&ctx).await.id;
        assert_eq!(unlock(&request, &acme.alice, USER_PASSWORD).await.status_code(), 200);
        for sql in [
            "CREATE OR REPLACE FUNCTION refuse_reveal_events() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN IF NEW.action IN ('reveal', 'copy') THEN RAISE EXCEPTION 'log is down'; END IF; RETURN NEW; END $$;",
            "CREATE TRIGGER refuse_reveal_events BEFORE INSERT ON login_events FOR EACH ROW EXECUTE FUNCTION refuse_reveal_events();",
        ] {
            ctx.db.execute_unprepared(sql).await.unwrap();
        }
        let res = reveal(&request, &acme.alice, id).await;
        assert_eq!(res.status_code(), 500);
        assert!(!res.text().contains(SECRET));
        let res = post(&request, &acme.alice, &format!("/documents/logins/{id}/copy"), &[]).await;
        assert_eq!(res.status_code(), 500);
        assert!(!res.text().contains(SECRET));
        ctx.db
            .execute_unprepared("DROP TRIGGER refuse_reveal_events ON login_events; DROP FUNCTION refuse_reveal_events();")
            .await
            .unwrap();
        assert!(deliveries(&ctx).iter().all(|m| !m.contains("saw the password")));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_log_cannot_be_changed_or_cleared() {
    request::<App, _, _>(|request, ctx| async move {
        let acme = acme(&request, &ctx).await;
        add(&request, &acme.alice, acme.folder.id, &[]).await;
        let login = only_login(&ctx).await;
        for sql in [
            "UPDATE login_events SET username = 'nobody'",
            "UPDATE login_events SET login_id = NULL, action = 'edit'",
            "DELETE FROM login_events",
        ] {
            let err = ctx.db.execute_unprepared(sql).await.expect_err(sql);
            assert!(err.to_string().contains("append-only"), "{sql}: {err}");
        }
        // Deleting the login keeps its history, with its title.
        let res = post(
            &request,
            &acme.carol,
            &format!("/documents/logins/{}/delete", login.id),
            &[],
        )
        .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(logins::Entity::find().count(&ctx.db).await.unwrap(), 0);
        let all = login_events::Entity::find().all(&ctx.db).await.unwrap();
        assert_eq!(all.len(), 2, "create and delete");
        assert!(all
            .iter()
            .all(|e| e.login_id.is_none() && e.login_title.as_deref() == Some("Companies House")));

        // The activity page is for owners only.
        let page = request
            .get("/documents/activity")
            .add_header(acme.alice.0.clone(), acme.alice.1.clone())
            .await;
        assert_eq!(page.status_code(), 200);
        let page = page.text();
        assert!(
            page.contains("<strong>alice</strong> added <strong>Companies House</strong>"),
            "{page}"
        );
        assert!(page.contains("<strong>carol</strong> deleted <strong>Companies House</strong>"));
        for cookie in [&acme.carol, &acme.bob] {
            let res = request
                .get("/documents/activity")
                .add_header(cookie.0.clone(), cookie.1.clone())
                .await;
            assert_eq!(res.status_code(), 403);
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn five_wrong_app_passwords_lock_the_account_until_a_reset() {
    request::<App, _, _>(|request, ctx| async move {
        let acme = acme(&request, &ctx).await;
        add(&request, &acme.alice, acme.folder.id, &[]).await;
        let id = only_login(&ctx).await.id;
        let before = deliveries(&ctx).len();
        for left in [4, 3, 2] {
            let res = unlock(&request, &acme.carol, "wrong").await;
            assert_eq!(res.status_code(), 422);
            assert!(res.text().contains(&format!("{left} more tries")));
        }
        assert!(unlock(&request, &acme.carol, "wrong").await.text().contains("1 more try,"));
        let res = unlock(&request, &acme.carol, "wrong").await;
        assert_eq!(res.status_code(), 401);
        let failed = events(&ctx, "reauth_failed").await;
        assert_eq!(failed.len(), 5);
        assert_eq!(failed[2].detail.as_deref(), Some("3 of 5"));
        assert_eq!(events(&ctx, "lock").await.len(), 1);
        let carol = users::Model::find_by_email(&ctx.db, "carol@example.com").await.unwrap();
        assert!(carol.is_locked());

        // Her sessions ended, the right password no longer helps, and the owner was told once.
        let res = request.get("/documents").add_header(acme.carol.0.clone(), acme.carol.1.clone()).await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(unlock(&request, &acme.carol, USER_PASSWORD).await.status_code(), 303);
        let res = request
            .post("/login")
            .form(&serde_json::json!({ "email": "carol@example.com", "password": USER_PASSWORD }))
            .await;
        assert_eq!(res.status_code(), 403);
        assert!(res.text().contains("Your account is locked"));
        let mail = deliveries(&ctx);
        let locked: Vec<&String> = mail[before..].iter().filter(|m| m.contains("carol's account was locked")).collect();
        assert_eq!(locked.len(), 1, "one email per lock, to alice the only owner");
        assert!(locked[0].contains("To: alice@example.com"));

        // A reset unlocks it. carol can see logins, so the owner hears about the reset too.
        request.post("/forgot").form(&serde_json::json!({ "email": "carol@example.com" })).await;
        let token = users::Model::find_by_email(&ctx.db, "carol@example.com").await.unwrap().reset_token.unwrap();
        let res = request
            .post(&format!("/reset/{token}"))
            .form(&serde_json::json!({ "password": "new-password-1", "password_confirmation": "new-password-1" }))
            .await;
        assert_eq!(res.status_code(), 303);
        assert!(!users::Model::find_by_email(&ctx.db, "carol@example.com").await.unwrap().is_locked());
        let carol = TestMember::new(
            login(&request, "carol@example.com", "new-password-1").await,
            acme.carol.org_id,
        );
        assert_eq!(unlock(&request, &carol, "new-password-1").await.status_code(), 200);
        assert_eq!(reveal(&request, &carol, id).await.status_code(), 200);
        let mail = deliveries(&ctx);
        assert!(mail.iter().any(|m| m.contains("carol's password was changed") && m.contains("To: alice@example.com")));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn keys_are_bound_to_their_login_and_version_and_made_once() {
    request::<App, _, _>(|request, ctx| async move {
        let acme = acme(&request, &ctx).await;
        add(&request, &acme.alice, acme.folder.id, &[]).await;
        let mut second = vec![
            ("title", "Bank".to_string()),
            ("password", "bank-pass-999".to_string()),
        ];
        second.push(("recovery", String::new()));
        add(&request, &acme.alice, acme.folder.id, &second).await;
        let mut all = logins::Entity::find().all(&ctx.db).await.unwrap();
        all.sort_by_key(|l| l.id);
        let (first, bank) = (&all[0], &all[1]);
        let kek = Kek::get(&ctx).unwrap();
        assert!(first.reveal(&ctx.db, &kek, "password").await.is_ok());

        // The same ciphertext under another login id does not open.
        let mut moved = bank.clone().into_active_model();
        moved.password_ct = ActiveValue::Set(first.password_ct.clone());
        moved.password_nonce = ActiveValue::Set(first.password_nonce.clone());
        let moved = moved.update(&ctx.db).await.unwrap();
        assert!(moved.reveal(&ctx.db, &kek, "password").await.is_err());

        // Nor under another key version.
        let mut versioned = first.clone().into_active_model();
        versioned.dek_version = ActiveValue::Set(2);
        let versioned = versioned.update(&ctx.db).await.unwrap();
        assert!(versioned.reveal(&ctx.db, &kek, "password").await.is_err());

        // Many first uses at once agree on one key.
        sign_up_member(&request, "Globex", "gina", "gina@example.com").await;
        let globex = organisations::Model::find_by_slug(&ctx.db, "globex")
            .await
            .unwrap();
        let url = ctx.config.database.uri.clone();
        let mut options = sea_orm::ConnectOptions::new(url);
        options.max_connections(8);
        let pool = sea_orm::Database::connect(options).await.unwrap();
        let keys = futures_util::future::join_all(
            (0..8).map(|_| org_keys::Model::dek_for(&pool, &kek, globex.id)),
        )
        .await;
        let first_key = secrecy::ExposeSecret::expose_secret(&keys[0].as_ref().unwrap().0).to_vec();
        for key in &keys {
            assert_eq!(
                secrecy::ExposeSecret::expose_secret(&key.as_ref().unwrap().0).to_vec(),
                first_key
            );
        }
        let rows = org_keys::Entity::find_by_id(globex.id)
            .all(&ctx.db)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn removing_someone_lists_the_logins_to_change() {
    request::<App, _, _>(|request, ctx| async move {
        let acme = acme(&request, &ctx).await;
        let dave_id = user_id(&ctx, "dave@example.com").await;
        add(
            &request,
            &acme.alice,
            acme.folder.id,
            &[("access", dave_id.to_string())],
        )
        .await;
        let mut mailbox = vec![
            ("title", "Office mailbox".to_string()),
            ("url", "mail.example.com".to_string()),
        ];
        mailbox.push(("recovery", String::new()));
        add(&request, &acme.alice, acme.folder.id, &mailbox).await;
        let id = logins::Entity::find()
            .all(&ctx.db)
            .await
            .unwrap()
            .iter()
            .map(|l| l.id)
            .min()
            .unwrap();
        assert_eq!(
            unlock(&request, &acme.dave, USER_PASSWORD)
                .await
                .status_code(),
            200
        );
        assert_eq!(reveal(&request, &acme.dave, id).await.status_code(), 200);

        let dave_m = memberships::Model::default_for_user(&ctx.db, dave_id)
            .await
            .unwrap()
            .unwrap();
        let res = post(
            &request,
            &acme.carol,
            &format!("/members/{}/remove", dave_m.id),
            &[],
        )
        .await;
        let to = format!("/members/{}/rotate?removed=1", dave_m.id);
        assert_eq!(res.headers()["location"], to.as_str());
        let page = request
            .get(&to)
            .add_header(acme.carol.0.clone(), acme.carol.1.clone())
            .await
            .text();
        assert!(page.contains("dave has been removed."));
        assert!(page.contains("Change these at the source"));
        assert!(page.contains("Companies House"));
        assert!(page.contains("Last seen"));
        assert!(!page.contains("Office mailbox"), "dave could not see it");
        let removed = events(&ctx, "member_removed").await;
        assert_eq!(removed[0].username, "carol");
        assert_eq!(removed[0].detail.as_deref(), Some("dave"));

        // Owners see it too; other members do not.
        let path = format!("/members/{}/rotate", dave_m.id);
        let res = request
            .get(&path)
            .add_header(acme.alice.0.clone(), acme.alice.1.clone())
            .await;
        assert_eq!(res.status_code(), 200);
        let res = request
            .get(&path)
            .add_header(acme.bob.0.clone(), acme.bob.1.clone())
            .await;
        assert_eq!(res.status_code(), 403);

        // Removing an admin lists every login, mailbox first.
        let carol_m =
            memberships::Model::default_for_user(&ctx.db, user_id(&ctx, "carol@example.com").await)
                .await
                .unwrap()
                .unwrap();
        post(
            &request,
            &acme.alice,
            &format!("/members/{}/remove", carol_m.id),
            &[],
        )
        .await;
        let page = request
            .get(&format!("/members/{}/rotate", carol_m.id))
            .add_header(acme.alice.0.clone(), acme.alice.1.clone())
            .await
            .text();
        let mailbox_at = page.find("Office mailbox").unwrap();
        assert!(mailbox_at < page.find("Companies House").unwrap());
        assert!(page.contains("Start with the mailbox at the top."));

        // Someone who could see nothing gets the empty state.
        let bob_m =
            memberships::Model::default_for_user(&ctx.db, user_id(&ctx, "bob@example.com").await)
                .await
                .unwrap()
                .unwrap();
        post(
            &request,
            &acme.alice,
            &format!("/members/{}/remove", bob_m.id),
            &[],
        )
        .await;
        let page = request
            .get(&format!("/members/{}/rotate", bob_m.id))
            .add_header(acme.alice.0.clone(), acme.alice.1.clone())
            .await
            .text();
        assert!(page.contains("No passwords to change"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_folder_with_a_login_cannot_be_deleted() {
    request::<App, _, _>(|request, ctx| async move {
        let acme = acme(&request, &ctx).await;
        add(&request, &acme.alice, acme.folder.id, &[]).await;
        let res = post(
            &request,
            &acme.alice,
            &format!("/documents/folders/{}/delete", acme.folder.id),
            &[],
        )
        .await;
        assert_eq!(res.status_code(), 422);
        assert!(res.text().contains("Only empty folders can be deleted."));
        assert!(
            folders::Model::find_in_org(&ctx.db, acme.org.id, acme.folder.id)
                .await
                .is_ok()
        );
    })
    .await;
}

/// Other organisations, logged-out visitors and a super admin acting inside
/// the organisation get nothing from any login route.
#[tokio::test]
#[serial]
async fn login_routes_need_your_own_membership_of_the_organisation() {
    request::<App, _, _>(|request, ctx| async move {
        let acme = acme(&request, &ctx).await;
        add(&request, &acme.alice, acme.folder.id, &[]).await;
        let id = only_login(&ctx).await.id;
        let gina = sign_up_member(&request, "Globex", "gina", "gina@example.com").await;
        assert_eq!(
            unlock(&request, &gina, USER_PASSWORD).await.status_code(),
            200
        );
        let admin = super::admin::super_admin_cookie(&request, &ctx).await;
        let acting = format!("{}; acting_org={}", admin.1.to_str().unwrap(), acme.org.id);

        for path in [
            format!("/documents/logins/{id}"),
            "/documents/activity".to_string(),
        ] {
            let res = request
                .get(&path)
                .add_header(gina.0.clone(), gina.1.clone())
                .await;
            if path.contains("/logins/") {
                assert_eq!(res.status_code(), 404, "other org GET {path}");
            }
            assert!(
                !res.text().contains("Companies House"),
                "other org GET {path}"
            );
            let res = request.get(&path).await;
            assert_eq!(res.status_code(), 303, "logged out GET {path}");
            let res = request
                .get(&path)
                .add_header("cookie", acting.clone())
                .await;
            assert_eq!(res.status_code(), 403, "acting GET {path}");
            assert!(!res.text().contains("Companies House"));
        }
        let posts = [
            format!("/documents/logins/{id}"),
            format!("/documents/logins/{id}/delete"),
            format!("/documents/logins/{id}/reveal"),
            format!("/documents/logins/{id}/copy"),
            format!("/documents/folders/{}/logins", acme.folder.id),
            "/documents/logins/unlock".to_string(),
        ];
        let form = [
            ("title", "hijacked"),
            ("password", USER_PASSWORD),
            ("field", "password"),
        ];
        for path in &posts {
            let res = request.post(path).as_member(&gina).form(&form).await;
            if !path.ends_with("/unlock") {
                assert_eq!(res.status_code(), 404, "other org POST {path}");
            }
            assert!(!res.text().contains(SECRET));
            assert_eq!(
                request.post(path).form(&form).await.status_code(),
                303,
                "logged out POST {path}"
            );
            let res = request
                .post(path)
                .add_header("cookie", acting.clone())
                .add_header("x-org-id", acme.org.id.to_string())
                .form(&form)
                .await;
            assert_eq!(res.status_code(), 403, "acting POST {path}");
            assert!(!res.text().contains(SECRET));
        }
        let login = only_login(&ctx).await;
        assert_eq!(login.title, "Companies House");
        assert!(events(&ctx, "reveal").await.is_empty());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_logins_folder_takes_no_files() {
    request::<App, _, _>(|request, ctx| async move {
        let acme = acme(&request, &ctx).await;
        let res = request
            .post(&format!("/documents/folders/{}/files", acme.folder.id))
            .as_member(&acme.alice)
            .multipart(
                axum_test::multipart::MultipartForm::new().add_part(
                    "file",
                    axum_test::multipart::Part::bytes(super::documents::PDF.to_vec())
                        .file_name("passwords.pdf"),
                ),
            )
            .await;
        assert_eq!(res.status_code(), 422);
        assert!(res.text().contains("The Logins folder holds logins only."));
        assert_eq!(
            collab::models::documents::Entity::find()
                .count(&ctx.db)
                .await
                .unwrap(),
            0
        );

        // No picker, drop zone or hint; owners are offered a login, members are told who adds them.
        let page = request
            .get(&format!("/documents/folders/{}", acme.folder.id))
            .add_header(acme.alice.0.clone(), acme.alice.1.clone())
            .await
            .text();
        assert!(!page.contains("data-upload"));
        assert!(!page.contains("data-drop-overlay"));
        assert!(!page.contains("Drag files here"));
        assert!(!page.contains("data-pick"));
        assert!(page.contains("Add a login"));
        let page = request
            .get(&format!("/documents/folders/{}", acme.folder.id))
            .add_header(acme.bob.0.clone(), acme.bob.1.clone())
            .await
            .text();
        assert!(page.contains("Owners and admins add shared logins here."));
        assert!(!page.contains("data-upload"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_locked_account_loses_its_claude_tokens() {
    request::<App, _, _>(|request, ctx| async move {
        let acme = acme(&request, &ctx).await;
        let before = super::mcp::token_for(&ctx, "carol@example.com").await;
        let list = |token: String| {
            let request = &request;
            async move {
                super::mcp::rpc(request, &token, "tools/list", serde_json::json!({}))
                    .await
                    .status
            }
        };
        assert_eq!(list(before.clone()).await, 200);
        for _ in 0..5 {
            unlock(&request, &acme.carol, "wrong").await;
        }
        let carol = users::Model::find_by_email(&ctx.db, "carol@example.com")
            .await
            .unwrap();
        assert!(carol.is_locked());
        assert_eq!(list(before).await, 401, "revoked when the account locked");
        // A token made later is refused too, while the account is locked.
        let after = super::mcp::token_for(&ctx, "carol@example.com").await;
        assert_eq!(list(after).await, 401);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn an_old_cookie_without_a_sign_in_id_gets_one_when_it_unlocks() {
    request::<App, _, _>(|request, ctx| async move {
        let acme = acme(&request, &ctx).await;
        add(&request, &acme.alice, acme.folder.id, &[]).await;
        let id = only_login(&ctx).await.id;
        let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();
        let config = ctx.config.get_jwt_config().unwrap();
        // How tokens looked before sign-ins had ids: a session version only.
        let mut claims = serde_json::Map::new();
        claims.insert("sv".to_string(), alice.session_version.into());
        let old = loco_rs::auth::jwt::JWT::new(&config.secret)
            .generate_token(config.expiration, alice.pid.to_string(), claims)
            .unwrap();
        let old = TestMember::new(
            (
                axum::http::HeaderName::from_static("cookie"),
                axum::http::HeaderValue::from_str(&format!("auth_token={old}")).unwrap(),
            ),
            acme.alice.org_id,
        );
        assert_eq!(reveal(&request, &old, id).await.status_code(), 423);
        let res = unlock(&request, &old, USER_PASSWORD).await;
        assert_eq!(res.status_code(), 200);
        let new = TestMember::new(
            super::prepare_data::cookie_from(&res.headers().clone()),
            acme.alice.org_id,
        );
        let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();
        assert!(alice.secrets_unlocked_sid.is_some());
        let res = reveal(&request, &new, id).await;
        assert_eq!(res.status_code(), 200);
        assert!(res.text().contains(SECRET));
        assert_eq!(
            reveal(&request, &old, id).await.status_code(),
            423,
            "the old cookie stays locked"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn editing_a_login_keeps_what_removed_people_could_see() {
    request::<App, _, _>(|request, ctx| async move {
        let acme = acme(&request, &ctx).await;
        let dave_id = user_id(&ctx, "dave@example.com").await;
        add(
            &request,
            &acme.alice,
            acme.folder.id,
            &[("access", dave_id.to_string())],
        )
        .await;
        let login = only_login(&ctx).await;
        let dave_m = memberships::Model::default_for_user(&ctx.db, dave_id)
            .await
            .unwrap()
            .unwrap();
        post(
            &request,
            &acme.alice,
            &format!("/members/{}/remove", dave_m.id),
            &[],
        )
        .await;
        let res = post(
            &request,
            &acme.alice,
            &format!("/documents/logins/{}", login.id),
            &[
                ("title", "Companies House"),
                ("password", ""),
                ("recovery", ""),
            ],
        )
        .await;
        assert_eq!(res.status_code(), 303, "{}", res.text());
        let named = collab::models::login_accesses::Model::user_ids(&ctx.db, login.id)
            .await
            .unwrap();
        assert_eq!(named, vec![dave_id]);
        let page = request
            .get(&format!("/members/{}/rotate", dave_m.id))
            .add_header(acme.alice.0.clone(), acme.alice.1.clone())
            .await
            .text();
        assert!(page.contains("Could see it, never opened it"), "{page}");
    })
    .await;
}

/// The rotate list belongs to its organisation, and needs your own membership.
#[tokio::test]
#[serial]
async fn the_rotate_list_stays_inside_its_organisation() {
    request::<App, _, _>(|request, ctx| async move {
        let acme = acme(&request, &ctx).await;
        let dave_m =
            memberships::Model::default_for_user(&ctx.db, user_id(&ctx, "dave@example.com").await)
                .await
                .unwrap()
                .unwrap();
        post(
            &request,
            &acme.alice,
            &format!("/members/{}/remove", dave_m.id),
            &[],
        )
        .await;
        let path = format!("/members/{}/rotate", dave_m.id);
        let gina = sign_up_member(&request, "Globex", "gina", "gina@example.com").await;
        let res = request
            .get(&path)
            .add_header(gina.0.clone(), gina.1.clone())
            .await;
        assert_eq!(res.status_code(), 404);
        assert!(!res.text().contains("dave"));
        let admin = super::admin::super_admin_cookie(&request, &ctx).await;
        let acting = format!("{}; acting_org={}", admin.1.to_str().unwrap(), acme.org.id);
        let res = request.get(&path).add_header("cookie", acting).await;
        assert_eq!(res.status_code(), 403);
        assert_eq!(request.get(&path).await.status_code(), 303, "logged out");
    })
    .await;
}

/// A Claude token and an unused OAuth code for this person in this org.
async fn claude_access(
    ctx: &AppContext,
    client: &collab::models::oauth_clients::Model,
    org_id: i64,
    user_id: i64,
) -> (String, String) {
    use collab::models::{access_tokens, oauth_codes};
    let token = access_tokens::Model::create_personal(
        &ctx.db,
        org_id,
        user_id,
        &access_tokens::PersonalParams {
            name: "Laptop".to_string(),
        },
        super::mcp::RESOURCE,
    )
    .await
    .unwrap()
    .1;
    let (_, code) = oauth_codes::Model::issue(
        &ctx.db,
        &oauth_codes::Consent {
            client,
            org_id,
            user_id,
            redirect_uri: super::oauth::CLAUDE,
            code_challenge: &oauth_codes::challenge_for(&"v".repeat(43)),
            resource: super::mcp::RESOURCE,
            scope: "mcp",
        },
    )
    .await
    .unwrap();
    (token, code)
}

#[tokio::test]
#[serial]
async fn locking_revokes_claude_tokens_in_every_org() {
    use collab::models::{access_tokens, oauth_clients, oauth_codes};
    request::<App, _, _>(|request, ctx| async move {
        let acme = acme(&request, &ctx).await;
        let carol = users::Model::find_by_email(&ctx.db, "carol@example.com")
            .await
            .unwrap();
        let lumen = organisations::Model::create_for_user(
            &ctx.db,
            &carol,
            &organisations::NewOrgParams {
                organisation_name: "Lumen Café".to_string(),
                name: "carol".to_string(),
            },
            None,
        )
        .await
        .unwrap();
        let client = oauth_clients::Model::register(
            &ctx.db,
            &oauth_clients::RegisterParams {
                client_name: None,
                redirect_uris: vec![super::oauth::CLAUDE.to_string()],
            },
        )
        .await
        .unwrap();
        let in_acme = claude_access(&ctx, &client, acme.org.id, carol.id).await;
        let in_lumen = claude_access(&ctx, &client, lumen.id, carol.id).await;
        let usable = |token: String| {
            let db = ctx.db.clone();
            async move {
                access_tokens::Model::find_usable(&db, &token, super::mcp::RESOURCE)
                    .await
                    .unwrap()
                    .is_some()
            }
        };
        assert!(usable(in_acme.0.clone()).await && usable(in_lumen.0.clone()).await);

        for _ in 0..5 {
            unlock(&request, &acme.carol, "wrong").await;
        }
        assert!(users::Model::find_by_email(&ctx.db, "carol@example.com")
            .await
            .unwrap()
            .is_locked());
        for (token, code) in [&in_acme, &in_lumen] {
            let row = access_tokens::Entity::find()
                .all(&ctx.db)
                .await
                .unwrap()
                .into_iter()
                .find(|t| t.token_hash == access_tokens::hash(token))
                .unwrap();
            assert!(row.revoked_at.is_some(), "revoked in org {}", row.organisation_id);
            let code_row = oauth_codes::Entity::find()
                .all(&ctx.db)
                .await
                .unwrap()
                .into_iter()
                .find(|c| c.code_hash == access_tokens::hash(code))
                .unwrap();
            assert!(code_row.used_at.is_some(), "code used in org {}", code_row.organisation_id);
        }

        // After a reset, neither old token works.
        request
            .post("/forgot")
            .form(&serde_json::json!({ "email": "carol@example.com" }))
            .await;
        let token = users::Model::find_by_email(&ctx.db, "carol@example.com")
            .await
            .unwrap()
            .reset_token
            .unwrap();
        let res = request
            .post(&format!("/reset/{token}"))
            .form(&serde_json::json!({ "password": "new-password-1", "password_confirmation": "new-password-1" }))
            .await;
        assert_eq!(res.status_code(), 303);
        assert!(!usable(in_acme.0).await, "Acme's token");
        assert!(!usable(in_lumen.0).await, "the created org's token");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_creator_resetting_their_password_gets_no_owner_notice_from_their_own_org() {
    request::<App, _, _>(|request, ctx| async move {
        let acme = acme(&request, &ctx).await;
        // carol becomes a second owner of Acme, so Acme has someone to tell.
        let carol_id = user_id(&ctx, "carol@example.com").await;
        let mut carol = memberships::Model::active_in(&ctx.db, acme.org.id, carol_id)
            .await
            .unwrap()
            .unwrap()
            .into_active_model();
        carol.role = ActiveValue::Set(memberships::role::OWNER.to_string());
        carol.update(&ctx.db).await.unwrap();
        add(&request, &acme.alice, acme.folder.id, &[]).await;

        let res = request
            .post("/orgs")
            .as_member(&acme.alice)
            .form(&[("organisation_name", "Lumen Café"), ("name", "ali")])
            .await;
        assert_eq!(res.status_code(), 303);
        let lumen = organisations::Model::find_by_slug(&ctx.db, "lumen-caf")
            .await
            .unwrap();
        let lumen_logins = folders::Model::list_for_org(&ctx.db, lumen.id)
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.folder.name == "Logins")
            .unwrap()
            .folder;
        let in_lumen = super::multi_org::with_org(&acme.alice, lumen.id);
        assert_eq!(
            add(&request, &in_lumen, lumen_logins.id, &[]).await.status_code(),
            303
        );

        let before = deliveries(&ctx).len();
        request
            .post("/forgot")
            .form(&serde_json::json!({ "email": "alice@example.com" }))
            .await;
        let token = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap()
            .reset_token
            .unwrap();
        let res = request
            .post(&format!("/reset/{token}"))
            .form(&serde_json::json!({ "password": "new-password-1", "password_confirmation": "new-password-1" }))
            .await;
        assert_eq!(res.status_code(), 303);
        let notices: Vec<String> = deliveries(&ctx)[before..]
            .iter()
            .filter(|m| m.contains("alice's password was changed"))
            .cloned()
            .collect();
        assert_eq!(notices.len(), 1, "Acme's other owner only");
        assert!(notices[0].contains("To: carol@example.com"));
        assert!(!notices.iter().any(|m| m.contains("To: alice@example.com")));
    })
    .await;
}

/// The emails since `before` that mention `text`, by recipient.
fn emails_about(ctx: &AppContext, before: usize, text: &str) -> Vec<String> {
    deliveries(ctx)[before..]
        .iter()
        .filter(|m| m.contains(text))
        .cloned()
        .collect()
}

#[tokio::test]
#[serial]
async fn locking_emails_the_person_and_the_lock_orgs_owners_only() {
    request::<App, _, _>(|request, ctx| async move {
        let acme = acme(&request, &ctx).await;
        // carol also owns Lumen, with dave as a second owner there.
        let carol = users::Model::find_by_email(&ctx.db, "carol@example.com")
            .await
            .unwrap();
        let lumen = organisations::Model::create_for_user(
            &ctx.db,
            &carol,
            &organisations::NewOrgParams {
                organisation_name: "Lumen Café".to_string(),
                name: "carol".to_string(),
            },
            None,
        )
        .await
        .unwrap();
        let dave = memberships::ActiveModel {
            user_id: ActiveValue::Set(user_id(&ctx, "dave@example.com").await),
            username: ActiveValue::Set("dave".to_string()),
            role: ActiveValue::Set(memberships::role::OWNER.to_string()),
            status: ActiveValue::Set(memberships::status::ACTIVE.to_string()),
            ..Default::default()
        };
        loco_rs::prelude::TenantActiveModelExt::set_tenant(dave, lumen.id)
            .unwrap()
            .insert(&ctx.db)
            .await
            .unwrap();

        let before = deliveries(&ctx).len();
        for _ in 0..5 {
            unlock(&request, &acme.carol, "wrong").await;
        }

        let owners = emails_about(&ctx, before, "carol's account was locked");
        assert_eq!(owners.len(), 1, "Acme's owner only");
        assert!(owners[0].contains("To: alice@example.com"));
        let hers = emails_about(&ctx, before, "Your Collaboration Tool account was locked");
        assert_eq!(hers.len(), 1);
        assert!(hers[0].contains("To: carol@example.com"));
        assert!(hers[0].contains("trying to see logins in Acme"));
        assert!(hers[0].contains("Claude was disconnected."));
        assert!(hers[0].contains("/forgot"));
        assert!(!hers[0].contains("/reset/"), "never a reset token");
        assert!(
            deliveries(&ctx)[before..]
                .iter()
                .all(|m| !m.contains("To: dave@example.com")),
            "Lumen's owners are not told"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn an_owner_who_locks_themselves_gets_one_email() {
    request::<App, _, _>(|request, ctx| async move {
        let acme = acme(&request, &ctx).await;
        let before = deliveries(&ctx).len();
        for _ in 0..5 {
            unlock(&request, &acme.alice, "wrong").await;
        }
        assert!(users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap()
            .is_locked());

        let to_alice = emails_about(&ctx, before, "To: alice@example.com");
        assert_eq!(to_alice.len(), 1, "{to_alice:?}");
        assert!(to_alice[0].contains("Your Collaboration Tool account was locked"));
        assert!(emails_about(&ctx, before, "alice's account was locked").is_empty());
    })
    .await;
}
