//! The organisation switcher, `/orgs`, and the org each page says it was
//! drawn for.
use collab::{
    app::App,
    models::{conversation_members, conversations, folders, memberships, organisations, users},
};
use loco_rs::{app::AppContext, prelude::*, TestServer};
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait};
use serial_test::serial;

use super::{
    multi_org::{alice_in_two_orgs, set_cookie, set_status, switch, with_org},
    prepare_data::{join, open_page, sign_up_cookie, sign_up_member, slug_of, Cookie},
};

/// A page load as the browser makes it: a bare app address is followed
/// into the last org.
async fn get(request: &TestServer, who: &Cookie, path: &str) -> axum_test::TestResponse {
    open_page(request, who, path).await.0
}

/// An HTMX call from a page of the cookie's last org: at that org's address.
async fn htmx_get(request: &TestServer, who: &Cookie, path: &str) -> axum_test::TestResponse {
    let path = if path.starts_with("/o/") {
        path.to_string()
    } else {
        let (_, home) = open_page(request, who, "/dashboard").await;
        let slug = slug_of(&home).expect("a last org").to_string();
        collab::views::links::org_path(&slug, path)
    };
    request
        .get(&path)
        .add_header(who.0.clone(), who.1.clone())
        .add_header("HX-Request", "true")
        .await
}

/// The page's `<body ...>` tag.
fn body_tag(html: &str) -> &str {
    let start = html.find("<body").expect("a body");
    &html[start..start + html[start..].find('>').unwrap()]
}

fn assert_drawn_for(html: &str, org_id: i64, what: &str) {
    let body = body_tag(html);
    assert!(
        body.contains(&format!(r#"data-org-id="{org_id}""#)),
        "{what}: {body}"
    );
    assert!(
        body.contains(&format!(r#"hx-headers='{{"X-Org-Id": {org_id}}}'"#)),
        "{what}: {body}"
    );
}

/// A super admin's cookie with `acting_org` set.
fn acting_in(admin: &Cookie, org_id: i64) -> Cookie {
    (
        admin.0.clone(),
        format!("{}; acting_org={org_id}", admin.1.to_str().unwrap())
            .parse()
            .unwrap(),
    )
}

/// Another org for this user, through a membership row, as PR 4 will make.
async fn add_org(
    ctx: &AppContext,
    request: &TestServer,
    name: &str,
    user_id: i64,
    status: &str,
) -> i64 {
    let slug = name.to_lowercase().replace(' ', "-");
    let owner = slug.replace('-', "");
    sign_up_cookie(request, name, &owner, &format!("{owner}@example.com")).await;
    let org = organisations::Model::find_by_slug(&ctx.db, &slug)
        .await
        .unwrap();
    memberships::ActiveModel {
        user_id: ActiveValue::Set(user_id),
        username: ActiveValue::Set("alice".to_string()),
        role: ActiveValue::Set(memberships::role::MEMBER.to_string()),
        status: ActiveValue::Set(status.to_string()),
        ..Default::default()
    }
    .set_tenant(org.id)
    .unwrap()
    .insert(&ctx.db)
    .await
    .unwrap();
    org.id
}

#[tokio::test]
#[serial]
async fn app_pages_carry_their_org_on_body() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, alice, _, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let beta = in_beta.organisation_id;
        let tab = with_org(&ctx, &cookie, beta).await;
        let general = conversations::Model::find_general(&ctx.db, beta)
            .await
            .unwrap();
        conversation_members::Model::add(&ctx.db, &general, alice)
            .await
            .unwrap();
        let folder = folders::Model::list_for_org(&ctx.db, beta)
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
            .folder;
        let folder_path = format!("/documents/folders/{}", folder.id);
        let chat_path = format!("/chat/{}", general.id);
        for path in [
            "/dashboard",
            "/notifications",
            "/more",
            "/settings/claude",
            "/members",
            "/tasks",
            folder_path.as_str(),
            chat_path.as_str(),
            "/roadmap",
            "/meetings",
            "/orgs",
        ] {
            let res = get(&request, &tab, path).await;
            assert_eq!(res.status_code(), 200, "{path}");
            assert_drawn_for(&res.text(), beta, path);
        }
        let page = get(&request, &tab, &folder_path).await.text();
        assert!(
            body_tag(&page).contains(r#"hx-history="false""#),
            "body_attrs kept"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn acting_pages_carry_the_acting_org() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let acme = organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap();
        let admin = super::admin::super_admin_cookie(&request, &ctx).await;
        let acting = acting_in(&admin, acme.id);
        for path in ["/dashboard", "/tasks", "/members"] {
            let page = get(&request, &acting, path).await.text();
            assert_drawn_for(&page, acme.id, path);
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn auth_pages_carry_no_org() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        for path in ["/login", "/signup"] {
            let page = request.get(path).await.text();
            assert!(!body_tag(&page).contains("data-org-id"), "{path}");
            assert!(!body_tag(&page).contains("X-Org-Id"), "{path}");
        }
        let waiting = join(&request, &ctx, "acme", "bob", "bob@example.com").await;
        let res = get(&request, &waiting, "/dashboard").await;
        assert_eq!(res.status_code(), 403);
        let page = res.text();
        assert!(!body_tag(&page).contains("data-org-id"), "the waiting page");
    })
    .await;
}

fn has_switcher(page: &str) -> bool {
    page.contains("data-org-menu")
}

#[tokio::test]
#[serial]
async fn one_org_shows_the_switcher() {
    request::<App, _, _>(|request, _ctx| async move {
        let alice = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let home = get(&request, &alice, "/dashboard").await.text();
        assert!(has_switcher(&home), "the menu, for New organisation");
        assert!(home.contains("data-org-sheet"), "the phone header");
        assert!(home.contains(r#"id="org-sheet""#));
        let more = get(&request, &alice, "/more").await.text();
        assert!(more.contains(r#"<h2 class="section-label">Organisation</h2>"#));
        assert!(!more.contains(">Switch<"), "nothing to switch to");
        assert!(!more.contains("data-org-sheet"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn two_orgs_show_the_switcher() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, _, _) = alice_in_two_orgs(&request, &ctx).await;
        let home = get(&request, &cookie, "/dashboard").await.text();
        assert!(has_switcher(&home));
        assert!(home.contains(r#"aria-haspopup="menu""#));
        assert!(
            home.contains(r#"<a href="/o/acme/orgs?section=dashboard" class="org-switch""#),
            "a link without JavaScript"
        );
        assert!(home.contains("data-org-sheet"), "the phone header");
        let more = get(&request, &cookie, "/more").await.text();
        assert!(more.contains("data-org-sheet"), "the Organisation row");
        assert!(more.contains(">Switch<"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn one_org_and_a_pending_request_shows_the_switcher() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let id = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap()
            .id;
        add_org(
            &ctx,
            &request,
            "Himalayan Ritual",
            id,
            memberships::status::PENDING,
        )
        .await;
        let home = get(&request, &alice, "/dashboard").await.text();
        assert!(has_switcher(&home));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn an_acting_admin_with_no_membership_sees_a_plain_name() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let acme = organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap();
        let admin = super::admin::super_admin_cookie(&request, &ctx).await;
        // The super admin task gives them an org of their own; take it away.
        let raj = users::Model::find_by_email(&ctx.db, "gurungraj26@gmail.com")
            .await
            .unwrap();
        for m in memberships::Model::for_user(&ctx.db, raj.id).await.unwrap() {
            memberships::Entity::delete_by_id(m.id)
                .exec(&ctx.db)
                .await
                .unwrap();
        }
        let home = get(&request, &acting_in(&admin, acme.id), "/dashboard")
            .await
            .text();
        assert!(!has_switcher(&home));
        assert!(home.contains("as platform admin"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn htmx_requests_skip_the_switcher_count() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, _, _) = alice_in_two_orgs(&request, &ctx).await;
        // The same full template asked for over HTMX: `org_menu` stays false,
        // because the count only runs for full page loads.
        let home = htmx_get(&request, &cookie, "/dashboard").await.text();
        assert!(!has_switcher(&home));
        let home = get(&request, &cookie, "/dashboard").await.text();
        assert!(has_switcher(&home));
    })
    .await;
}

/// The org names in the list, in order.
fn listed(html: &str) -> Vec<String> {
    html.split(r#"<span class="org-name truncate-1">"#)
        .skip(1)
        .map(|rest| rest[..rest.find('<').unwrap()].to_string())
        .collect()
}

#[tokio::test]
#[serial]
async fn the_org_list_is_active_a_to_z_then_pending() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, alice, in_acme, _) = alice_in_two_orgs(&request, &ctx).await;
        add_org(
            &ctx,
            &request,
            "Zen Scale",
            alice,
            memberships::status::ACTIVE,
        )
        .await;
        add_org(
            &ctx,
            &request,
            "Himalayan Ritual",
            alice,
            memberships::status::PENDING,
        )
        .await;
        add_org(
            &ctx,
            &request,
            "Northwind",
            alice,
            memberships::status::ACTIVE,
        )
        .await;
        let list = htmx_get(&request, &cookie, "/orgs").await.text();
        assert_eq!(
            listed(&list),
            ["Acme", "Beta", "Northwind", "Zen Scale", "Himalayan Ritual"]
        );
        // Acme is current: ticked, and not a switch button.
        assert!(list.contains(r#"aria-checked="true" data-org-current"#));
        assert!(!list.contains(&format!(
            r#"formaction="/o/acme/orgs/{}/switch""#,
            in_acme.organisation_id
        )));
        let pending = &list[list.find("Waiting for approval").unwrap() - 600..];
        assert!(pending.contains(r#"role="menuitem" aria-disabled="true""#));
        assert!(pending.contains("Himalayan Ritual"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_org_list_hides_removed_and_declined() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, alice, _, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        set_status(&ctx, in_beta, memberships::status::REMOVED).await;
        add_org(
            &ctx,
            &request,
            "Declined Co",
            alice,
            memberships::status::REJECTED,
        )
        .await;
        let list = get(&request, &cookie, "/orgs").await.text();
        assert_eq!(listed(&list), ["Acme"]);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_org_list_never_shows_other_peoples_orgs() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, _, _) = alice_in_two_orgs(&request, &ctx).await;
        sign_up_cookie(&request, "Globex", "gina", "gina@example.com").await;
        let list = htmx_get(&request, &cookie, "/orgs").await.text();
        assert!(!list.contains("Globex"));
        let page = get(&request, &cookie, "/orgs").await.text();
        assert!(!page.contains("Globex"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_org_list_over_htmx_is_a_fragment() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, _, _) = alice_in_two_orgs(&request, &ctx).await;
        let list = htmx_get(&request, &cookie, "/orgs").await.text();
        assert!(!list.contains("<html"));
        assert!(list.contains("data-org-list"));
        let page = get(&request, &cookie, "/orgs").await.text();
        assert!(page.contains("<html"), "a page without JavaScript");
        assert!(page.contains("data-org-list"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_org_list_is_no_store_and_varies_on_hx_request() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, _, _) = alice_in_two_orgs(&request, &ctx).await;
        for res in [
            htmx_get(&request, &cookie, "/orgs").await,
            get(&request, &cookie, "/orgs").await,
        ] {
            assert_eq!(res.headers()["cache-control"], "no-store");
            assert_eq!(res.headers()["vary"], "HX-Request");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_org_list_while_acting_has_acting_in_and_your_organisations() {
    request::<App, _, _>(|request, ctx| async move {
        let (_, _, in_acme, _) = alice_in_two_orgs(&request, &ctx).await;
        let admin = super::admin::super_admin_cookie(&request, &ctx).await;
        let raj = users::Model::find_by_email(&ctx.db, "gurungraj26@gmail.com")
            .await
            .unwrap();
        // The super admin task made them the owner of an org of their own.
        let own = memberships::Model::default_for_user(&ctx.db, raj.id)
            .await
            .unwrap()
            .unwrap()
            .organisation_id;
        let own_org = organisations::Model::find_by_id(&ctx.db, own)
            .await
            .unwrap();
        let own_name = own_org.name.clone();
        let acting = acting_in(&admin, in_acme.organisation_id);
        let home = get(&request, &acting, "/dashboard").await.text();
        assert!(has_switcher(&home), "they have an org of their own");
        assert!(home.contains("Platform admin"));
        let list = htmx_get(&request, &acting, "/orgs").await.text();
        let acting_at = list.find("Acting in").expect("Acting in");
        let yours = list.find("Your organisations").expect("Your organisations");
        assert!(acting_at < yours);
        assert_eq!(listed(&list), ["Acme".to_string(), own_name]);
        // Their own org is a link there; acting carries on in Acme's tabs.
        assert!(list.contains(&format!(
            r#"<a href="/o/{}/dashboard" class="org-row""#,
            own_org.slug
        )));
        assert!(!list.contains("Switching ends acting"));
        assert!(!list.contains("/switch"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_probe_is_204_and_refused_when_stale() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, in_acme, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let tab = with_org(&ctx, &cookie, in_beta.organisation_id).await;
        let res = request
            .get(&tab.at("/orgs/current"))
            .add_header(tab.0.clone(), tab.1.clone())
            .add_header("HX-Request", "true")
            .add_header("x-org-id", in_beta.organisation_id.to_string())
            .await;
        assert_eq!(res.status_code(), 204);
        assert_eq!(res.headers()["cache-control"], "no-store");
        let res = request
            .get(&tab.at("/orgs/current"))
            .add_header(tab.0.clone(), tab.1.clone())
            .add_header("HX-Request", "true")
            .add_header("x-org-id", in_acme.organisation_id.to_string())
            .await;
        assert_eq!(res.status_code(), 409);
        assert_eq!(
            res.headers()["HX-Redirect"],
            format!("/o/beta/dashboard?from={}", in_acme.organisation_id).as_str()
        );
        assert_eq!(res.headers()["cache-control"], "no-store");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_old_switch_post_lands_under_o() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, _, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let res = switch(&request, &cookie, in_beta.organisation_id).await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(res.headers()["location"], "/o/beta/dashboard");
        let tab = with_org(&ctx, &cookie, in_beta.organisation_id).await;
        let res = get(&request, &tab, "/o/beta/dashboard").await;
        assert_eq!(res.status_code(), 200);
        // The toast is the switcher's, on the client, from sessionStorage.
        assert!(!res.text().contains("now in Beta"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn switching_over_htmx_sends_hx_redirect_to_home() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, _, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let res = request
            .post(&format!("/orgs/{}/switch", in_beta.organisation_id))
            .add_header(cookie.0.clone(), cookie.1.clone())
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(res.headers()["HX-Redirect"], "/o/beta/dashboard");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn switching_to_an_org_you_left_is_a_404() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, _, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let beta = in_beta.organisation_id;
        set_status(&ctx, in_beta, memberships::status::REMOVED).await;
        let res = request
            .post(&format!("/orgs/{beta}/switch"))
            .add_header(cookie.0.clone(), cookie.1.clone())
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 404);
        assert_eq!(set_cookie(&res, "current_org"), None);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn switched_is_ignored() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, in_acme, _) = alice_in_two_orgs(&request, &ctx).await;
        let here = with_org(&ctx, &cookie, in_acme.organisation_id).await;
        let res = get(&request, &here, "/o/acme/dashboard?switched=1").await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(set_cookie(&res, "current_org"), None);
        assert!(!res.text().contains("now in"), "no toast from the server");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_switch_ignores_a_next_parameter() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, _, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let res = request
            .post(&format!(
                "/orgs/{}/switch?next=//evil.example",
                in_beta.organisation_id
            ))
            .add_header(cookie.0.clone(), cookie.1.clone())
            .form(&serde_json::json!({ "next": "https://evil.example" }))
            .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(res.headers()["location"], "/o/beta/dashboard");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn there_is_no_get_that_switches() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, _, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let res = get(
            &request,
            &cookie,
            &format!("/orgs/{}/switch", in_beta.organisation_id),
        )
        .await;
        assert_eq!(res.status_code(), 405);
        assert_eq!(set_cookie(&res, "current_org"), None);
    })
    .await;
}

/// The "New organisation" row: a link after the list, and no switch form.
fn new_org_row_outside_the_form(html: &str) -> bool {
    let row = regex::Regex::new(
        r#"<a href="/o/[a-z0-9-]+/orgs/new" class="org-row org-new" role="menuitem""#,
    )
    .unwrap();
    let Some(row) = row.find(html) else {
        return false;
    };
    html[..row.start()].contains("data-org-list") && !html[..row.start()].contains("/switch")
}

#[tokio::test]
#[serial]
async fn the_org_list_ends_with_new_organisation() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, _, _) = alice_in_two_orgs(&request, &ctx).await;
        let fragment = htmx_get(&request, &cookie, "/orgs").await.text();
        assert!(new_org_row_outside_the_form(&fragment), "{fragment}");
        assert!(fragment.find("Beta").unwrap() < fragment.find("New organisation").unwrap());
        // The /orgs page has the switch form, then the row. Its layout has a
        // form of its own after it (sign out), so look inside the card.
        let page = get(&request, &cookie, "/orgs").await.text();
        let card = &page[page.find("org-page").unwrap()..];
        assert!(new_org_row_outside_the_form(card));
        // One org: the same row.
        let solo = sign_up_member(&request, "Zen", "zed", "zed@example.com").await;
        let list = htmx_get(&request, &solo, "/orgs").await.text();
        assert!(new_org_row_outside_the_form(&list));
        let more = get(&request, &solo, "/more").await.text();
        assert!(more.contains(r#"<a href="/o/zen/orgs/new" class="list-row org-new"#));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn acting_shows_no_new_organisation_row() {
    request::<App, _, _>(|request, ctx| async move {
        let (_, _, in_acme, _) = alice_in_two_orgs(&request, &ctx).await;
        let admin = super::admin::super_admin_cookie(&request, &ctx).await;
        let acting = acting_in(&admin, in_acme.organisation_id);
        let list = htmx_get(&request, &acting, "/orgs").await.text();
        assert!(list.contains("Acting in"));
        assert!(!list.contains("/orgs/new"));
        let page = get(&request, &acting, "/orgs").await.text();
        assert!(!page.contains("/orgs/new"));
        let more = get(&request, &acting, "/more").await.text();
        assert!(more.contains(">Switch<"), "back to their own org");
        assert!(!more.contains("/orgs/new"));
        // Their own org, not acting: the row is back.
        let own = get(&request, &(admin.0.clone(), admin.1.clone()), "/more")
            .await
            .text();
        assert!(own.contains("/orgs/new"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_admin_page_keeps_its_plain_name() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let admin = super::admin::super_admin_cookie(&request, &ctx).await;
        let page = get(&request, &admin, "/admin").await.text();
        assert!(page.contains("Platform admin"));
        assert!(!has_switcher(&page));
        assert!(!page.contains("/orgs/new"));
    })
    .await;
}
