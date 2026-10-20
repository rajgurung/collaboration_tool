//! "New organisation": `GET /orgs/new` and `POST /orgs`.
use collab::{
    app::App,
    models::{
        folders, login_accesses, login_events, logins, memberships, org_keys, organisations, tasks,
        users,
    },
};
use loco_rs::{app::AppContext, testing::prelude::*, TestServer};
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
use serial_test::serial;

use super::{
    multi_org::{clears, set_cookie, with_org},
    prepare_data::{join, page_org_id, sign_up_member, AsMember, TestMember, USER_PASSWORD},
};

async fn get(request: &TestServer, who: &TestMember, path: &str) -> axum_test::TestResponse {
    request
        .get(path)
        .add_header(who.0.clone(), who.1.clone())
        .await
}

async fn htmx_get(request: &TestServer, who: &TestMember, path: &str) -> axum_test::TestResponse {
    request
        .get(path)
        .add_header(who.0.clone(), who.1.clone())
        .add_header("HX-Request", "true")
        .await
}

/// `POST /orgs` from a tab drawn for the member's org, as HTMX sends it.
async fn create(
    request: &TestServer,
    who: &TestMember,
    name: &str,
    username: &str,
) -> axum_test::TestResponse {
    request
        .post("/orgs")
        .as_member(who)
        .form(&[("organisation_name", name), ("name", username)])
        .await
}

/// The org a create's `Set-Cookie: current_org` points at.
fn created_id(res: &axum_test::TestResponse) -> i64 {
    let cookie = set_cookie(res, "current_org").expect("a current_org cookie");
    cookie["current_org=".len()..cookie.find(';').unwrap()]
        .parse()
        .unwrap()
}

/// Creates "Lumen Café" as `who`, under the username `ali`, and returns the
/// member as the browser now is: in the new org.
async fn lumen(request: &TestServer, who: &TestMember) -> TestMember {
    let res = create(request, who, "Lumen Café", "ali").await;
    assert_eq!(res.status_code(), 303, "{}", res.text());
    with_org(who, created_id(&res))
}

async fn alice(request: &TestServer) -> TestMember {
    sign_up_member(request, "Acme Studio", "alice", "alice@example.com").await
}

async fn user(ctx: &AppContext, email: &str) -> users::Model {
    users::Model::find_by_email(&ctx.db, email).await.unwrap()
}

async fn org_count(ctx: &AppContext) -> u64 {
    organisations::Entity::find().count(&ctx.db).await.unwrap()
}

/// A super admin's cookie with `acting_org` set.
fn acting_in(admin: &super::prepare_data::Cookie, org_id: i64) -> TestMember {
    TestMember::new(
        (
            admin.0.clone(),
            format!("{}; acting_org={org_id}", admin.1.to_str().unwrap())
                .parse()
                .unwrap(),
        ),
        org_id,
    )
}

/// Moves this person's created orgs out of the daily window.
async fn backdate(ctx: &AppContext, user_id: i64) {
    organisations::Entity::update_many()
        .col_expr(
            organisations::Column::CreatedAt,
            sea_orm::sea_query::Expr::value(chrono::Utc::now() - chrono::Duration::days(2)),
        )
        .filter(organisations::Column::CreatedById.eq(user_id))
        .exec(&ctx.db)
        .await
        .unwrap();
}

/// Gives this person `n` created orgs in all, through the model.
async fn created_up_to(ctx: &AppContext, who: &users::Model, n: u64) {
    while organisations::Model::created_count(&ctx.db, who.id)
        .await
        .unwrap()
        < n
    {
        backdate(ctx, who.id).await;
        let count = organisations::Model::created_count(&ctx.db, who.id)
            .await
            .unwrap();
        organisations::Model::create_for_user(
            &ctx.db,
            who,
            &organisations::NewOrgParams {
                organisation_name: format!("Org {count}"),
                name: "ali".to_string(),
            },
            None,
        )
        .await
        .unwrap();
    }
    backdate(ctx, who.id).await;
}

#[tokio::test]
#[serial]
async fn the_form_renders_with_your_username_and_the_org_field() {
    request::<App, _, _>(|request, _ctx| async move {
        let alice = alice(&request).await;
        let res = get(&request, &alice, "/orgs/new").await;
        assert_eq!(res.status_code(), 200);
        let page = res.text();
        assert!(page.contains("<html"), "a page without JavaScript");
        assert!(page.contains(r#"name="name" value="alice""#));
        assert!(page.contains(r#"name="organisation_name" value="""#));
        assert!(page.contains(&format!(
            r#"<input type="hidden" name="_org" value="{}">"#,
            alice.org_id
        )));
        assert!(page.contains(r#"method="post" action="/orgs" hx-post="/orgs""#));
        assert!(page.contains("How people in this organisation see you."));
        assert!(page.contains("Nothing from Acme Studio is shared with it."));
        assert!(page.contains("You'll sign in with the same email and password."));
        assert!(!page.contains("You're acting in"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_form_over_htmx_is_a_fragment() {
    request::<App, _, _>(|request, _ctx| async move {
        let alice = alice(&request).await;
        let res = htmx_get(&request, &alice, "/orgs/new").await;
        assert_eq!(res.status_code(), 200);
        assert!(!res.text().contains("<html"));
        assert!(res.text().contains(r#"id="org-new-form""#));
        assert_eq!(res.headers()["cache-control"], "no-store");
        assert_eq!(res.headers()["vary"], "HX-Request");
        let page = get(&request, &alice, "/orgs/new").await;
        assert_eq!(page.headers()["cache-control"], "no-store");
        assert_eq!(page.headers()["vary"], "HX-Request");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_form_at_the_cap_shows_the_limit_instead_of_fields() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = alice(&request).await;
        let page = get(&request, &alice, "/orgs/new").await.text();
        assert!(!page.contains("set up 10 organisations"));
        created_up_to(&ctx, &user(&ctx, "alice@example.com").await, 10).await;
        for page in [
            get(&request, &alice, "/orgs/new").await.text(),
            htmx_get(&request, &alice, "/orgs/new").await.text(),
        ] {
            assert!(page.contains("You've set up 10 organisations"));
            assert!(page.contains("That's the most one account can set up for now."));
            assert!(!page.contains(r#"name="organisation_name""#), "no fields");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn creating_lands_on_home_in_the_new_org_with_a_toast() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = alice(&request).await;
        let res = create(&request, &alice, "Lumen Café", "ali").await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(res.headers()["location"], "/dashboard?created=1");
        let id = created_id(&res);
        let org = organisations::Model::find_by_id(&ctx.db, id).await.unwrap();
        assert_eq!(org.name, "Lumen Café");
        let home = get(&request, &with_org(&alice, id), "/dashboard?created=1").await;
        assert_eq!(page_org_id(&home.text()), Some(id));
        assert_eq!(home.headers()["cache-control"], "no-store");
        assert!(home
            .text()
            .contains(r#"data-flash="info">Lumen Café is ready. You&#39;re the owner.</div>"#));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_created_flag_alone_changes_nothing() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = alice(&request).await;
        let before = org_count(&ctx).await;
        let res = get(&request, &alice, "/dashboard?created=1").await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(set_cookie(&res, "current_org"), None);
        assert!(
            res.text().contains("Acme Studio is ready."),
            "names the org it resolved"
        );
        assert_eq!(org_count(&ctx).await, before);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn creating_over_htmx_sends_hx_redirect_to_home() {
    request::<App, _, _>(|request, _ctx| async move {
        let alice = alice(&request).await;
        let res = request
            .post("/orgs")
            .as_member(&alice)
            .add_header("HX-Request", "true")
            .form(&[("organisation_name", "Lumen Café"), ("name", "ali")])
            .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(res.headers()["HX-Redirect"], "/dashboard?created=1");
        assert!(set_cookie(&res, "current_org").is_some());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_plain_form_post_with_org_creates() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = alice(&request).await;
        let before = org_count(&ctx).await;
        // No X-Org-Id and no HX-Request: what the page form sends without
        // JavaScript, with its server-rendered _org.
        let org_id = alice.org_id.to_string();
        let res = request
            .post("/orgs")
            .add_header(alice.0.clone(), alice.1.clone())
            .form(&[
                ("organisation_name", "Lumen Café"),
                ("name", "ali"),
                ("_org", org_id.as_str()),
            ])
            .await;
        assert_eq!(res.status_code(), 303, "{}", res.text());
        assert_eq!(res.headers()["location"], "/dashboard?created=1");
        assert_eq!(org_count(&ctx).await, before + 1);

        // Without the field it fails closed, like any plain form.
        let res = request
            .post("/orgs")
            .add_header(alice.0.clone(), alice.1.clone())
            .form(&[("organisation_name", "Northwind Bakery"), ("name", "ali")])
            .await;
        assert_eq!(res.status_code(), 303);
        assert!(res.headers()["location"]
            .to_str()
            .unwrap()
            .starts_with("/dashboard?from="));
        assert_eq!(org_count(&ctx).await, before + 1);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_stale_tab_cannot_create() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = alice(&request).await;
        let before = org_count(&ctx).await;
        let stale = TestMember::new(alice.cookie.clone(), 999_999);
        let res = request
            .post("/orgs")
            .as_member(&stale)
            .add_header("HX-Request", "true")
            .form(&[("organisation_name", "Lumen Café"), ("name", "ali")])
            .await;
        assert_eq!(res.status_code(), 409);
        assert_eq!(res.headers()["HX-Redirect"], "/dashboard?from=999999");
        assert_eq!(org_count(&ctx).await, before);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn creating_sets_current_org_and_clears_acting_org() {
    request::<App, _, _>(|request, _ctx| async move {
        let alice = alice(&request).await;
        let res = create(&request, &alice, "Lumen Café", "ali").await;
        let cookie = set_cookie(&res, "current_org").unwrap();
        assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Lax"));
        assert!(clears(&res, "acting_org"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn creating_ends_an_unlock_of_logins() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = alice(&request).await;
        let res = request
            .post("/documents/logins/unlock")
            .as_member(&alice)
            .form(&[("password", USER_PASSWORD)])
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let unlocked = user(&ctx, "alice@example.com").await;
        assert_eq!(unlocked.secrets_unlocked_org_id, Some(alice.org_id));
        lumen(&request, &alice).await;
        let after = user(&ctx, "alice@example.com").await;
        assert_eq!(after.secrets_unlocked_until, None);
        assert_eq!(after.secrets_unlocked_org_id, None);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn creating_copies_the_current_org_time_zone() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = alice(&request).await;
        organisations::Model::find_by_id(&ctx.db, alice.org_id)
            .await
            .unwrap()
            .set_timezone(&ctx.db, "Asia/Kathmandu")
            .await
            .unwrap();
        let lumen = lumen(&request, &alice).await;
        let org = organisations::Model::find_by_id(&ctx.db, lumen.org_id)
            .await
            .unwrap();
        assert_eq!(org.timezone, "Asia/Kathmandu");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_new_org_shows_the_owner_checklist() {
    request::<App, _, _>(|request, _ctx| async move {
        let alice = alice(&request).await;
        let lumen = lumen(&request, &alice).await;
        let home = get(&request, &lumen, "/dashboard").await.text();
        assert!(home.contains("Getting started"));
        assert!(home.contains("Invite your team"));
        assert!(home.contains(r#"aria-label="0 of 5 done""#));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_new_org_has_no_welcome_pop_up() {
    request::<App, _, _>(|request, _ctx| async move {
        let alice = alice(&request).await;
        let acme_home = get(&request, &alice, "/dashboard").await.text();
        assert!(acme_home.contains("data-welcome"), "signup keeps it");
        let lumen = lumen(&request, &alice).await;
        let home = get(&request, &lumen, "/dashboard").await.text();
        assert!(!home.contains("data-welcome"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn usernames_are_per_org() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = alice(&request).await;
        let lumen = lumen(&request, &alice).await;
        let id = user(&ctx, "alice@example.com").await.id;
        let in_acme = memberships::Model::active_in(&ctx.db, alice.org_id, id)
            .await
            .unwrap()
            .unwrap();
        let in_lumen = memberships::Model::active_in(&ctx.db, lumen.org_id, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(in_acme.username, "alice");
        assert_eq!(in_lumen.username, "ali");
        let acme_members = get(&request, &with_org(&alice, alice.org_id), "/members")
            .await
            .text();
        assert!(acme_members.contains(">alice<") && !acme_members.contains(">ali<"));
        let lumen_members = get(&request, &lumen, "/members").await.text();
        assert!(lumen_members.contains(">ali<") && !lumen_members.contains(">alice<"));
    })
    .await;
}

/// The field messages on a 422 page, in order.
fn messages(html: &str) -> Vec<String> {
    html.split(r#"<span class="field-error">"#)
        .skip(1)
        .map(|rest| {
            let text = &rest[..rest.find("</span>").unwrap()];
            // Drop the icon, if any.
            text.rsplit('>').next().unwrap().trim().to_string()
        })
        .collect()
}

#[tokio::test]
#[serial]
async fn bad_input_is_422_and_saves_nothing() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = alice(&request).await;
        let before = org_count(&ctx).await;
        let res = create(&request, &alice, "L", "2ali").await;
        assert_eq!(res.status_code(), 422);
        let page = res.text();
        assert!(
            page.contains(r#"name="organisation_name" value="L""#),
            "kept"
        );
        assert!(page.contains(r#"name="name" value="2ali""#), "kept");
        assert_eq!(
            messages(&page),
            [
                "Use 2 to 80 characters.",
                "Start with a letter and use 3 to 30 letters or numbers."
            ]
        );
        assert!(page.contains(r#"aria-invalid="true""#));
        // Over HTMX: the form alone, replacing itself.
        let res = request
            .post("/orgs")
            .as_member(&alice)
            .add_header("HX-Request", "true")
            .form(&[("organisation_name", "Lumen"), ("name", "everyone")])
            .await;
        assert_eq!(res.status_code(), 422);
        assert_eq!(res.headers()["HX-Retarget"], "#org-new-form");
        assert!(!res.text().contains("<html"));
        assert_eq!(messages(&res.text()), ["That name is reserved."]);
        assert_eq!(org_count(&ctx).await, before);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn signup_and_create_give_the_same_errors() {
    request::<App, _, _>(|request, _ctx| async move {
        let alice = alice(&request).await;
        for (name, username) in [
            ("   ", "alice"),
            ("L", "alice"),
            (&"a".repeat(81) as &str, "alice"),
            ("Lumen", "al"),
            ("Lumen", "everyone"),
        ] {
            let signup = request
                .post("/signup")
                .form(&serde_json::json!({
                    "organisation_name": name,
                    "name": username,
                    "email": "new@example.com",
                    "password": USER_PASSWORD,
                }))
                .await;
            assert_eq!(signup.status_code(), 422, "signup {name:?} {username}");
            let ours = create(&request, &alice, name, username).await;
            assert_eq!(ours.status_code(), 422, "create {name:?} {username}");
            assert_eq!(
                messages(&ours.text()),
                messages(&signup.text()),
                "{name:?} {username}"
            );
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_cap_is_a_422_and_saves_nothing() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = alice(&request).await;
        created_up_to(&ctx, &user(&ctx, "alice@example.com").await, 10).await;
        let before = org_count(&ctx).await;
        let res = create(&request, &alice, "Lumen Café", "ali").await;
        assert_eq!(res.status_code(), 422);
        assert!(res.text().contains("You've set up 10 organisations"));
        assert!(!res.text().contains(r#"name="organisation_name""#));
        assert_eq!(org_count(&ctx).await, before);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_daily_limit_is_a_422_with_its_message() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = alice(&request).await;
        assert_eq!(
            create(&request, &alice, "Two", "ali").await.status_code(),
            303
        );
        assert_eq!(
            create(&request, &alice, "Three", "ali").await.status_code(),
            303
        );
        let before = org_count(&ctx).await;
        let res = create(&request, &alice, "Four", "ali").await;
        assert_eq!(res.status_code(), 422);
        assert_eq!(
            messages(&res.text()),
            ["You&#39;ve set up 3 organisations today. Try again tomorrow."]
        );
        assert_eq!(org_count(&ctx).await, before);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_name_you_already_have_is_refused() {
    request::<App, _, _>(|request, _ctx| async move {
        let alice = alice(&request).await;
        let res = create(&request, &alice, "acme studio", "ali").await;
        assert_eq!(res.status_code(), 422);
        assert_eq!(
            messages(&res.text()),
            ["You already have an organisation called Acme Studio. Pick a name you can tell apart."]
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn creating_needs_a_signed_in_user() {
    request::<App, _, _>(|request, ctx| async move {
        alice(&request).await;
        let before = org_count(&ctx).await;
        let res = request.get("/orgs/new").await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(res.headers()["location"], "/login");
        let res = request
            .post("/orgs")
            .form(&[("organisation_name", "Lumen Café"), ("name", "ali")])
            .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(res.headers()["location"], "/login");
        assert_eq!(org_count(&ctx).await, before);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_pending_only_user_cannot_create() {
    request::<App, _, _>(|request, ctx| async move {
        alice(&request).await;
        let bob = join(&request, &ctx, "acme-studio", "bob", "bob@example.com").await;
        let before = org_count(&ctx).await;
        let res = get(&request, &bob, "/orgs/new").await;
        assert_eq!(res.status_code(), 403);
        assert!(res.text().contains("Acme Studio"), "the waiting page");
        // The form posts with the page's org claim, so it is sent Home,
        // where the waiting page shows.
        let res = create(&request, &bob, "Bob's Bikes", "bob").await;
        assert_eq!(res.status_code(), 409);
        assert_eq!(res.headers()["hx-redirect"], "/dashboard");
        assert_eq!(org_count(&ctx).await, before);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn acting_can_create_from_a_typed_link_and_acting_ends() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = alice(&request).await;
        let acme = alice.org_id;
        organisations::Model::find_by_id(&ctx.db, acme)
            .await
            .unwrap()
            .set_timezone(&ctx.db, "Asia/Kathmandu")
            .await
            .unwrap();
        let admin = super::admin::super_admin_cookie(&request, &ctx).await;
        let raj = user(&ctx, "gurungraj26@gmail.com").await;
        let acting = acting_in(&admin, acme);
        let form = get(&request, &acting, "/orgs/new").await.text();
        assert!(form.contains(
            "You're acting in <strong>Acme Studio</strong>. Creating this ends acting. The new organisation is yours, with you as owner."
        ));
        assert!(form.contains(&format!(r#"name="name" value="{}""#, raj.name)));
        let acme_members = memberships::Model::list_for_org(&ctx.db, acme)
            .await
            .unwrap()
            .len();

        let res = create(&request, &acting, "Zen Scale Labs", "raj").await;
        assert_eq!(res.status_code(), 303, "{}", res.text());
        assert!(clears(&res, "acting_org"));
        let org = organisations::Model::find_by_id(&ctx.db, created_id(&res))
            .await
            .unwrap();
        assert_eq!(org.timezone, "Europe/London", "nothing from Acme");
        assert_eq!(org.created_by_id, Some(raj.id));
        let owner = memberships::Model::active_in(&ctx.db, org.id, raj.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(owner.role, memberships::role::OWNER);
        assert_eq!(
            memberships::Model::list_for_org(&ctx.db, org.id)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            memberships::Model::list_for_org(&ctx.db, acme)
                .await
                .unwrap()
                .len(),
            acme_members,
            "Acme's members unchanged"
        );
        assert!(memberships::Model::active_in(&ctx.db, acme, raj.id)
            .await
            .unwrap()
            .is_none());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn creating_ignores_a_next_parameter() {
    request::<App, _, _>(|request, _ctx| async move {
        let alice = alice(&request).await;
        let res = request
            .post("/orgs?next=//evil.example")
            .as_member(&alice)
            .form(&[
                ("organisation_name", "Lumen Café"),
                ("name", "ali"),
                ("next", "https://evil.example"),
            ])
            .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(res.headers()["location"], "/dashboard?created=1");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn creating_sends_no_email() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = alice(&request).await;
        let before = ctx.mailer.as_ref().unwrap().deliveries().count;
        lumen(&request, &alice).await;
        assert_eq!(ctx.mailer.as_ref().unwrap().deliveries().count, before);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_new_org_is_separate_from_the_old_one() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = alice(&request).await;
        let acme = alice.org_id;
        let bob = join(&request, &ctx, "acme-studio", "bob", "bob@example.com").await;
        let pending =
            memberships::Model::default_for_user(&ctx.db, user(&ctx, "bob@example.com").await.id)
                .await
                .unwrap()
                .unwrap();
        pending
            .approve(&ctx.db, user(&ctx, "alice@example.com").await.id)
            .await
            .unwrap();
        assert_eq!(get(&request, &bob, "/dashboard").await.status_code(), 200);
        let project = super::tasks::project_in(&ctx, "acme-studio", "Studio website").await;
        let res = request
            .post("/tasks")
            .as_member(&alice)
            .form(&super::tasks::task_form(
                "Send the logo proofs",
                project.id,
                "",
            ))
            .await;
        assert!(res.status_code().as_u16() < 400, "{}", res.text());
        let task = tasks::Entity::find().one(&ctx.db).await.unwrap().unwrap();
        let logins_folder = folders::Model::list_for_org(&ctx.db, acme)
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.folder.name == "Logins")
            .unwrap()
            .folder;
        let res = request
            .post(&format!("/documents/folders/{}/logins", logins_folder.id))
            .as_member(&alice)
            .form(&[
                ("title", "Acme bank"),
                ("username", "office@example.com"),
                ("password", "acme-secret-123"),
            ])
            .await;
        assert_eq!(res.status_code(), 303, "{}", res.text());
        request
            .post("/documents/folders")
            .as_member(&alice)
            .form(&[("name", "Acme contracts")])
            .await;
        // A real activity row in Acme: an unlock, then a reveal of Acme bank.
        let acme_bank = logins::Entity::find()
            .filter(logins::Column::Title.eq("Acme bank"))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unlock(&request, &alice).await.status_code(), 200);
        let res = reveal(&request, &alice, acme_bank.id).await;
        assert_eq!(res.status_code(), 200, "{}", res.text());

        let lumen = lumen(&request, &alice).await;
        let res = get(&request, &lumen, &format!("/tasks/{}", task.id)).await;
        assert_eq!(res.status_code(), 404, "an Acme task");
        let list = get(&request, &lumen, "/tasks").await.text();
        assert!(!list.contains("Send the logo proofs"));
        let folders = folders::Model::list_for_org(&ctx.db, lumen.org_id)
            .await
            .unwrap();
        let mut names: Vec<&str> = folders.iter().map(|s| s.folder.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["Company", "Logins"]);
        assert!(folders.iter().all(|s| s.files == 0 && s.logins == 0));
        let docs = get(&request, &lumen, "/documents").await.text();
        assert!(!docs.contains("Acme contracts"));
        assert_eq!(
            logins::Entity::find()
                .filter(logins::Column::OrganisationId.eq(lumen.org_id))
                .count(&ctx.db)
                .await
                .unwrap(),
            0
        );
        let activity = get(&request, &lumen, "/documents/activity").await.text();
        assert!(!activity.contains("Acme bank"));
        let acme_activity = get(&request, &with_org(&alice, acme), "/documents/activity")
            .await
            .text();
        assert!(acme_activity.contains("Acme bank"), "Acme's own log has it");
        assert_eq!(
            login_events::Entity::find()
                .filter(login_events::Column::OrganisationId.eq(lumen.org_id))
                .count(&ctx.db)
                .await
                .unwrap(),
            0,
            "no login events in Lumen"
        );
        let lumen_login_ids: Vec<i64> = logins::Entity::find()
            .filter(logins::Column::OrganisationId.eq(lumen.org_id))
            .all(&ctx.db)
            .await
            .unwrap()
            .into_iter()
            .map(|l| l.id)
            .collect();
        assert!(lumen_login_ids.is_empty());
        assert_eq!(
            login_accesses::Entity::find()
                .filter(login_accesses::Column::LoginId.is_in(lumen_login_ids))
                .count(&ctx.db)
                .await
                .unwrap(),
            0,
            "no login accesses in Lumen"
        );

        // A login saved in Lumen gets Lumen's own key.
        let lumen_logins = folders.iter().find(|s| s.folder.name == "Logins").unwrap();
        let res = request
            .post(&format!(
                "/documents/folders/{}/logins",
                lumen_logins.folder.id
            ))
            .as_member(&lumen)
            .form(&[
                ("title", "Lumen supplier"),
                ("username", "orders@example.com"),
                ("password", "lumen-secret-456"),
            ])
            .await;
        assert_eq!(res.status_code(), 303, "{}", res.text());
        let keys_for = |org_id: i64| {
            let db = ctx.db.clone();
            async move {
                org_keys::Entity::find()
                    .filter(org_keys::Column::OrganisationId.eq(org_id))
                    .all(&db)
                    .await
                    .unwrap()
            }
        };
        let (acme_keys, lumen_keys) = (keys_for(acme).await, keys_for(lumen.org_id).await);
        assert_eq!(acme_keys.len(), 1);
        assert_eq!(lumen_keys.len(), 1, "one key for Lumen");
        assert_ne!(acme_keys[0].wrapped_dek, lumen_keys[0].wrapped_dek);
        // Acme's login still reveals its own password, in Acme.
        let in_acme = with_org(&alice, acme);
        assert_eq!(unlock(&request, &in_acme).await.status_code(), 200);
        let res = reveal(&request, &in_acme, acme_bank.id).await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        assert!(res.text().contains("acme-secret-123"));
        let members = get(&request, &lumen, "/members").await.text();
        assert!(members.contains(">ali<") && !members.contains(">bob<"));
        let chat = get(&request, &lumen, "/chat").await.text();
        assert!(chat.contains("general"));
        assert_eq!(
            collab::models::conversations::Entity::find()
                .filter(collab::models::conversations::Column::OrganisationId.eq(lumen.org_id))
                .count(&ctx.db)
                .await
                .unwrap(),
            1,
            "only #general"
        );

        // Switching back shows Acme's.
        let back = with_org(&alice, acme);
        let list = get(&request, &back, "/tasks").await.text();
        assert!(list.contains("Send the logo proofs"));
        let docs = get(&request, &back, "/documents").await.text();
        assert!(docs.contains("Acme contracts"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn org_names_are_escaped_everywhere() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = alice(&request).await;
        let name = "<script>alert(1)</script>";
        let escaped = "&lt;script&gt;alert(1)&lt;/script&gt;";
        let res = create(&request, &alice, name, "ali").await;
        assert_eq!(res.status_code(), 303, "{}", res.text());
        let inside = with_org(&alice, created_id(&res));
        let toast = get(&request, &inside, "/dashboard?created=1").await.text();
        let list = htmx_get(&request, &inside, "/orgs").await.text();
        let more = get(&request, &inside, "/more").await.text();
        // The duplicate-name error quotes the stored name.
        let dupe = create(&request, &inside, name, "ali").await;
        assert_eq!(dupe.status_code(), 422);
        let dupe = dupe.text();
        assert!(dupe.contains("You already have an organisation called &lt;script&gt;"));
        let admin = super::admin::super_admin_cookie(&request, &ctx).await;
        let admin_page = request
            .get("/admin")
            .add_header(admin.0.clone(), admin.1.clone())
            .await
            .text();
        for (what, page) in [
            ("the toast", &toast),
            ("the switcher", &list),
            ("the duplicate-name error", &dupe),
            ("More", &more),
            ("/admin", &admin_page),
        ] {
            assert!(page.contains(escaped), "{what}: escaped");
            assert!(!page.contains(name), "{what}: never raw");
        }
        assert!(toast.contains(&format!("{escaped} is ready.")));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_page_form_keeps_its_page_buttons_after_a_422() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = alice(&request).await;
        let page = get(&request, &alice, "/orgs/new").await.text();
        assert!(page.contains(r#"<input type="hidden" name="as_page" value="1">"#));
        let dialog = htmx_get(&request, &alice, "/orgs/new").await.text();
        assert!(!dialog.contains(r#"name="as_page""#));
        // The page's form, sent over HTMX, refused.
        let res = request
            .post("/orgs")
            .as_member(&alice)
            .add_header("HX-Request", "true")
            .form(&[
                ("organisation_name", "L"),
                ("name", "ali"),
                ("as_page", "1"),
            ])
            .await;
        assert_eq!(res.status_code(), 422);
        assert!(!res.text().contains("<html"), "the form alone");
        assert!(!res.text().contains("data-dialog-close"));
        assert!(res.text().contains(r#"name="as_page" value="1""#));
        // At the cap, the same: Back to Home, not a dead Close.
        created_up_to(&ctx, &user(&ctx, "alice@example.com").await, 10).await;
        let res = request
            .post("/orgs")
            .as_member(&alice)
            .add_header("HX-Request", "true")
            .form(&[
                ("organisation_name", "Lumen"),
                ("name", "ali"),
                ("as_page", "1"),
            ])
            .await;
        assert_eq!(res.status_code(), 422);
        assert!(!res.text().contains("data-dialog-close"));
        assert!(res.text().contains("Back to Home"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn an_old_org_link_is_an_app_404_naming_only_the_current_org() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = alice(&request).await;
        let project = super::tasks::project_in(&ctx, "acme-studio", "Studio website").await;
        request
            .post("/tasks")
            .as_member(&alice)
            .form(&super::tasks::task_form("Send the logo proofs", project.id, ""))
            .await;
        let task = tasks::Entity::find().one(&ctx.db).await.unwrap().unwrap();
        let lumen = lumen(&request, &alice).await;
        let path = format!("/tasks/{}", task.id);
        let res = request
            .get(&path)
            .add_header(lumen.0.clone(), lumen.1.clone())
            .add_header("Accept", "text/html,application/xhtml+xml,*/*;q=0.8")
            .await;
        assert_eq!(res.status_code(), 404);
        let page = res.text();
        assert!(page.contains("<html"), "a page");
        assert!(page.contains("We couldn't find that in Lumen Café"), "{page}");
        assert!(page.contains("It may be in another of your organisations. Switch from the menu, then open the link again."));
        assert!(page.contains(r#"href="/dashboard" class="btn btn-primary btn-tall mt-2">Go to Home</a>"#));
        assert!(!page.contains("Acme Studio"), "never names another org");
        assert_eq!(page_org_id(&page), Some(lumen.org_id));

        // HTMX and API 404s are left alone.
        let res = request
            .get(&path)
            .add_header(lumen.0.clone(), lumen.1.clone())
            .add_header("Accept", "text/html")
            .add_header("HX-Request", "true")
            .add_header("x-org-id", lumen.org_id.to_string())
            .await;
        assert_eq!(res.status_code(), 404);
        assert!(!res.text().contains("<html"));
        let res = request
            .get(&path)
            .add_header(lumen.0.clone(), lumen.1.clone())
            .await;
        assert_eq!(res.status_code(), 404);
        assert!(!res.text().contains("<html"), "no Accept: text/html");

        // Signed out: the plain page, no org at all.
        let res = request
            .get("/no-such-page")
            .add_header("Accept", "text/html")
            .await;
        assert_eq!(res.status_code(), 404);
        assert!(res.text().contains("We couldn't find that"));
        assert!(!res.text().contains("Acme Studio") && !res.text().contains("Lumen"));
    })
    .await;
}

async fn unlock(request: &TestServer, who: &TestMember) -> axum_test::TestResponse {
    request
        .post("/documents/logins/unlock")
        .as_member(who)
        .form(&[("password", USER_PASSWORD)])
        .await
}

async fn reveal(request: &TestServer, who: &TestMember, id: i64) -> axum_test::TestResponse {
    request
        .post(&format!("/documents/logins/{id}/reveal"))
        .as_member(who)
        .form(&[("field", "password")])
        .await
}
