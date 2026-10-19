//! The Connect Claude page: personal tokens and revoking connections.
use collab::{
    app::App,
    models::{access_tokens, organisations},
};
use loco_rs::{testing::prelude::*, TestServer};
use sea_orm::EntityTrait;
use serde_json::json;
use serial_test::serial;

use super::{
    admin::super_admin_cookie,
    mcp::{rpc, token_for},
    multi_org::{alice_in_two_orgs, with_org},
    prepare_data::sign_up,
};

type Cookie = (axum::http::HeaderName, axum::http::HeaderValue);

/// The hidden `org_id` on the token form, as this browser sees it.
async fn shown_org_id(request: &TestServer, cookie: &Cookie) -> String {
    let page = request
        .get("/settings/claude")
        .add_header(cookie.0.clone(), cookie.1.clone())
        .await
        .text();
    let marker = r#"name="org_id" value=""#;
    let start = page.find(marker).expect("a hidden org_id") + marker.len();
    page[start..start + page[start..].find('"').unwrap()].to_string()
}

/// Creates a token through the page; returns the status and the page.
async fn create(request: &TestServer, cookie: &Cookie, name: &str) -> (u16, String) {
    let org_id = shown_org_id(request, cookie).await;
    let res = request
        .post("/settings/claude/tokens")
        .add_header(cookie.0.clone(), cookie.1.clone())
        .form(&json!({ "name": name, "org_id": org_id }))
        .await;
    (res.status_code().as_u16(), res.text())
}

fn shown_token(page: &str) -> String {
    let start = page.find("collab_pat_").expect("the token is shown");
    page[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect()
}

#[tokio::test]
#[serial]
async fn the_page_shows_the_connector_url_and_more_links_to_it() {
    request::<App, _, _>(|request, _ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let more = request
            .get("/more")
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(more.contains(r#"href="/settings/claude""#));

        let page = request
            .get("/settings/claude")
            .add_header(alice.0.clone(), alice.1.clone())
            .await;
        assert_eq!(page.status_code(), 200);
        let body = page.text();
        assert!(body.contains("http://localhost:5150/mcp"));
        assert!(body.contains("claude mcp add --transport http collab http://localhost:5150/mcp"));
        assert!(body.contains("Nothing is connected yet."));

        assert_eq!(request.get("/settings/claude").await.status_code(), 303);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_personal_token_is_shown_once_and_stored_hashed() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let (status, page) = create(&request, &alice, "Work laptop").await;
        assert_eq!(status, 200);
        let token = shown_token(&page);
        assert!(page.contains(&format!(r#"--header "Authorization: Bearer {token}""#)));

        let rows = access_tokens::Entity::find().all(&ctx.db).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].token_hash, access_tokens::hash(&token));
        assert!(
            !format!("{:?}", rows[0]).contains(&token),
            "no plain value stored"
        );
        assert_eq!(rows[0].kind, "personal");
        assert_eq!(rows[0].expires_at, None);

        assert_eq!(
            rpc(&request, &token, "tools/list", json!({})).await.status,
            200
        );
        let page = request
            .get("/settings/claude")
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(page.contains("Work laptop"));
        assert!(!page.contains(&token), "never shown again");

        let (status, page) = create(&request, &alice, "").await;
        assert_eq!(status, 422);
        assert!(page.contains("Name the token"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn ten_tokens_at_most() {
    request::<App, _, _>(|request, _ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        for i in 0..10 {
            assert_eq!(create(&request, &alice, &format!("Token {i}")).await.0, 200);
        }
        let (status, page) = create(&request, &alice, "One more").await;
        assert_eq!(status, 422);
        assert!(page.contains("up to 10 personal tokens"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn revoking_kills_the_grant_and_only_your_own() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let bob = sign_up(&request, "Globex", "bob", "bob@example.com").await;
        let alice_token = token_for(&ctx, "alice@example.com").await;
        let grant = access_tokens::Entity::find()
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap()
            .grant_id;

        let res = request
            .post(&format!("/settings/claude/grants/{grant}/revoke"))
            .add_header(bob.0.clone(), bob.1.clone())
            .await;
        assert_eq!(res.status_code(), 404, "someone else's connection");
        assert_eq!(
            rpc(&request, &alice_token, "tools/list", json!({}))
                .await
                .status,
            200
        );

        let res = request
            .post(&format!("/settings/claude/grants/{grant}/revoke"))
            .add_header(alice.0.clone(), alice.1.clone())
            .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(
            rpc(&request, &alice_token, "tools/list", json!({}))
                .await
                .status,
            401
        );
        let page = request
            .get("/settings/claude")
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(page.contains("Nothing is connected yet."));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_super_admin_acting_elsewhere_cannot_make_tokens() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let acme = organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap();
        let admin = super_admin_cookie(&request, &ctx).await;
        let acting = format!("{}; acting_org={}", admin.1.to_str().unwrap(), acme.id);

        let page = request
            .get("/settings/claude")
            .add_header("cookie", acting.clone())
            .await
            .text();
        assert!(page.contains("Tokens can only be made in your own organisation"));
        assert!(page.contains(r#"action="/admin/leave""#));

        let res = request
            .post("/settings/claude/tokens")
            .add_header("cookie", acting)
            .form(&json!({ "name": "Sneaky", "org_id": acme.id.to_string() }))
            .await;
        assert_eq!(res.status_code(), 403);
        assert!(!res.text().contains("collab_pat_"));
        assert!(access_tokens::Entity::find()
            .all(&ctx.db)
            .await
            .unwrap()
            .is_empty());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_token_form_pins_the_org() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, in_acme, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        assert_eq!(
            shown_org_id(&request, &cookie).await,
            in_acme.organisation_id.to_string()
        );
        let in_beta_cookie = with_org(&cookie, in_beta.organisation_id);
        assert_eq!(
            shown_org_id(&request, &in_beta_cookie).await,
            in_beta.organisation_id.to_string()
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_token_for_another_org_is_refused() {
    request::<App, _, _>(|request, ctx| async move {
        let (cookie, _, in_acme, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let in_beta_cookie = with_org(&cookie, in_beta.organisation_id);
        for org_id in [in_acme.organisation_id.to_string(), String::new()] {
            let res = request
                .post("/settings/claude/tokens")
                .add_header(in_beta_cookie.0.clone(), in_beta_cookie.1.clone())
                .form(&json!({ "name": "Laptop", "org_id": org_id }))
                .await;
            assert_eq!(res.status_code(), 409, "{org_id:?}");
            let page = res.text();
            assert!(page.contains(
                "You switched organisation while this page was open. Check the organisation, then create the token again."
            ));
            assert!(!page.contains("Start again from Claude"));
            assert!(page.contains(r#"value="Laptop""#), "the name is kept");
            assert!(!page.contains("collab_pat_"));
        }
        let res = request
            .post("/settings/claude/tokens")
            .add_header(in_beta_cookie.0.clone(), in_beta_cookie.1.clone())
            .form(&json!({ "name": "Laptop" }))
            .await;
        assert_eq!(res.status_code(), 409, "no org_id");
        assert!(access_tokens::Entity::find()
            .all(&ctx.db)
            .await
            .unwrap()
            .is_empty());
    })
    .await;
}
