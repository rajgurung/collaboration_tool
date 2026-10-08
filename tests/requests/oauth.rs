//! OAuth for Claude: discovery, registration, the Allow page and tokens.
use collab::{
    app::App,
    models::{access_tokens, oauth_codes, organisations},
};
use loco_rs::{testing::prelude::*, TestServer};
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{json, Value};
use serial_test::serial;

use super::{
    admin::super_admin_cookie,
    mcp::{rpc, RESOURCE},
    prepare_data::{sign_up, USER_PASSWORD},
};

type Cookie = (axum::http::HeaderName, axum::http::HeaderValue);

const CLAUDE: &str = "https://claude.ai/api/mcp/auth_callback";
const VERIFIER: &str = "a-fairly-long-pkce-verifier-that-is-at-least-43-characters-long";

fn form_body(fields: &[(&str, &str)]) -> axum::body::Bytes {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish()
        .into()
}

fn location(headers: &axum::http::HeaderMap) -> String {
    headers
        .get("location")
        .map(|l| l.to_str().unwrap().to_string())
        .unwrap_or_default()
}

fn query_of(location: &str) -> std::collections::HashMap<String, String> {
    url::Url::parse(location)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

async fn register(request: &TestServer, redirect: &str) -> String {
    let res = request
        .post("/oauth/register")
        .json(&json!({ "client_name": "Claude", "redirect_uris": [redirect] }))
        .await;
    assert_eq!(res.status_code(), 201, "{}", res.text());
    res.json::<Value>()["client_id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The authorize parameters Claude sends.
fn authorize_fields<'a>(client_id: &'a str, redirect: &'a str) -> Vec<(&'a str, String)> {
    vec![
        ("response_type", "code".to_string()),
        ("client_id", client_id.to_string()),
        ("redirect_uri", redirect.to_string()),
        ("code_challenge", oauth_codes::challenge_for(VERIFIER)),
        ("code_challenge_method", "S256".to_string()),
        ("state", "xyz".to_string()),
        ("scope", "tasks".to_string()),
        ("resource", RESOURCE.to_string()),
    ]
}

fn authorize_url(client_id: &str, redirect: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(authorize_fields(client_id, redirect))
        .finish();
    format!("/oauth/authorize?{query}")
}

/// Presses a button on the Allow page and returns where it sends the browser.
async fn decide(
    request: &TestServer,
    cookie: &Cookie,
    client_id: &str,
    redirect: &str,
    decision: &str,
) -> String {
    let mut fields = authorize_fields(client_id, redirect);
    fields.push(("decision", decision.to_string()));
    let pairs: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let res = request
        .post("/oauth/authorize")
        .add_header(cookie.0.clone(), cookie.1.clone())
        .content_type("application/x-www-form-urlencoded")
        .bytes(form_body(&pairs))
        .await;
    assert_eq!(res.status_code(), 302, "{}", res.text());
    location(res.headers())
}

async fn code_for(request: &TestServer, cookie: &Cookie, client_id: &str) -> String {
    let back = decide(request, cookie, client_id, CLAUDE, "allow").await;
    query_of(&back)["code"].clone()
}

/// Posts to the token endpoint. Returns the status and JSON body.
async fn token(request: &TestServer, fields: &[(&str, &str)]) -> (u16, Value) {
    let res = request
        .post("/oauth/token")
        .content_type("application/x-www-form-urlencoded")
        .bytes(form_body(fields))
        .await;
    assert_eq!(res.headers()["cache-control"].to_str().unwrap(), "no-store");
    (res.status_code().as_u16(), res.json())
}

async fn exchange(request: &TestServer, client_id: &str, code: &str) -> (u16, Value) {
    token(
        request,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("code", code),
            ("code_verifier", VERIFIER),
            ("redirect_uri", CLAUDE),
            ("resource", RESOURCE),
        ],
    )
    .await
}

async fn refresh(request: &TestServer, client_id: &str, refresh_token: &str) -> (u16, Value) {
    token(
        request,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh_token),
        ],
    )
    .await
}

async fn works(request: &TestServer, access_token: &str) -> bool {
    rpc(request, access_token, "tools/list", json!({}))
        .await
        .status
        == 200
}

#[tokio::test]
#[serial]
async fn discovery_documents_describe_the_server() {
    request::<App, _, _>(|request, _ctx| async move {
        for path in [
            "/.well-known/oauth-protected-resource",
            "/.well-known/oauth-protected-resource/mcp",
        ] {
            let doc: Value = request.get(path).await.json();
            assert_eq!(doc["resource"], RESOURCE);
            assert_eq!(
                doc["authorization_servers"],
                json!(["http://localhost:5150"])
            );
            assert_eq!(doc["scopes_supported"], json!(["tasks"]));
            assert_eq!(doc["bearer_methods_supported"], json!(["header"]));
        }
        let doc: Value = request
            .get("/.well-known/oauth-authorization-server")
            .await
            .json();
        assert_eq!(doc["issuer"], "http://localhost:5150");
        assert_eq!(
            doc["authorization_endpoint"],
            "http://localhost:5150/oauth/authorize"
        );
        assert_eq!(doc["token_endpoint"], "http://localhost:5150/oauth/token");
        assert_eq!(
            doc["registration_endpoint"],
            "http://localhost:5150/oauth/register"
        );
        assert_eq!(doc["code_challenge_methods_supported"], json!(["S256"]));
        assert_eq!(
            doc["token_endpoint_auth_methods_supported"],
            json!(["none"])
        );
        assert_eq!(doc["authorization_response_iss_parameter_supported"], true);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn registration_is_public_and_allowlisted() {
    request::<App, _, _>(|request, _ctx| async move {
        let res = request
            .post("/oauth/register")
            .json(&json!({
                "client_name": "Claude",
                "redirect_uris": [CLAUDE],
                "token_endpoint_auth_method": "client_secret_post",
            }))
            .await;
        assert_eq!(res.status_code(), 201);
        let body: Value = res.json();
        assert_eq!(body["token_endpoint_auth_method"], "none");
        assert!(body.get("client_secret").is_none());

        register(&request, "http://localhost:49152/callback").await;
        register(&request, "http://127.0.0.1:3000/callback").await;

        let res = request
            .post("/oauth/register")
            .json(&json!({ "redirect_uris": ["https://evil.example/callback"] }))
            .await;
        assert_eq!(res.status_code(), 400);
        assert_eq!(res.json::<Value>()["error"], "invalid_redirect_uri");

        let res = request
            .post("/oauth/register")
            .json(&json!({ "client_name": "x".repeat(9000), "redirect_uris": [CLAUDE] }))
            .await;
        assert_eq!(res.status_code(), 413, "bodies over 8 KB are refused");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_whole_flow_from_sign_in_to_a_tool_call() {
    request::<App, _, _>(|request, _ctx| async move {
        sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let client_id = register(&request, CLAUDE).await;
        let url = authorize_url(&client_id, CLAUDE);

        // Signed out: off to log in, with a way back.
        let res = request.get(&url).await;
        assert_eq!(res.status_code(), 303);
        let to_login = location(res.headers());
        assert!(
            to_login.starts_with("/login?next=%2Foauth%2Fauthorize%3F"),
            "{to_login}"
        );
        let login_page = request.get(&to_login).await.text();
        assert!(login_page.contains(r#"name="next""#));

        let res = request
            .post("/login")
            .content_type("application/x-www-form-urlencoded")
            .bytes(form_body(&[
                ("email", "alice@example.com"),
                ("password", USER_PASSWORD),
                ("next", &url),
            ]))
            .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(location(res.headers()), url);
        let set_cookie = res.headers()["set-cookie"].to_str().unwrap();
        let alice: Cookie = (
            axum::http::HeaderName::from_static("cookie"),
            set_cookie.split(';').next().unwrap().parse().unwrap(),
        );

        let page = request
            .get(&url)
            .add_header(alice.0.clone(), alice.1.clone())
            .await;
        assert_eq!(page.status_code(), 200);
        assert_eq!(page.headers()["x-frame-options"], "DENY");
        assert_eq!(page.headers()["cache-control"], "no-store");
        assert!(page.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'"));
        let body = page.text();
        assert!(body.contains("Allow Claude to use Collab Tool?"));
        assert!(body.contains("in Acme, as alice"));
        assert!(body.contains("claude.ai"));

        let back = decide(&request, &alice, &client_id, CLAUDE, "allow").await;
        assert!(back.starts_with(CLAUDE), "{back}");
        let query = query_of(&back);
        assert_eq!(query["state"], "xyz");
        assert_eq!(query["iss"], "http://localhost:5150");

        let (status, tokens) = exchange(&request, &client_id, &query["code"]).await;
        assert_eq!(status, 200, "{tokens}");
        assert_eq!(tokens["token_type"], "Bearer");
        assert_eq!(tokens["expires_in"], 3600);
        assert_eq!(tokens["scope"], "tasks");
        let access = tokens["access_token"].as_str().unwrap();
        assert!(access.starts_with("collab_at_"));
        assert!(tokens["refresh_token"]
            .as_str()
            .unwrap()
            .starts_with("collab_rt_"));

        let reply = rpc(
            &request,
            access,
            "tools/call",
            json!({ "name": "list_members", "arguments": {} }),
        )
        .await;
        assert_eq!(reply.status, 200);
        assert_eq!(reply.json()["result"]["isError"], false, "{}", reply.text);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn deny_goes_back_with_access_denied() {
    request::<App, _, _>(|request, _ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let client_id = register(&request, CLAUDE).await;
        let back = decide(&request, &alice, &client_id, CLAUDE, "deny").await;
        let query = query_of(&back);
        assert_eq!(query["error"], "access_denied");
        assert_eq!(query["state"], "xyz");
        assert!(!query.contains_key("code"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn bad_clients_and_redirects_get_a_page_not_a_redirect() {
    request::<App, _, _>(|request, _ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let client_id = register(&request, CLAUDE).await;

        let res = request
            .get(&authorize_url("collab_client_unknown", CLAUDE))
            .add_header(alice.0.clone(), alice.1.clone())
            .await;
        assert_eq!(res.status_code(), 400);
        assert!(res.headers().get("location").is_none());

        let res = request
            .get(&authorize_url(
                &client_id,
                "https://claude.com/api/mcp/auth_callback",
            ))
            .add_header(alice.0.clone(), alice.1.clone())
            .await;
        assert_eq!(res.status_code(), 400, "not the redirect URI it registered");

        let wrong_resource = authorize_url(&client_id, CLAUDE)
            .replace("localhost%3A5150%2Fmcp", "elsewhere.example%2Fmcp");
        let res = request
            .get(&wrong_resource)
            .add_header(alice.0.clone(), alice.1.clone())
            .await;
        assert_eq!(res.status_code(), 302);
        assert_eq!(
            query_of(&location(res.headers()))["error"],
            "invalid_target"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn codes_must_match_and_work_once() {
    request::<App, _, _>(|request, ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let client_id = register(&request, CLAUDE).await;
        let base = |code: &str| {
            vec![
                ("grant_type", "authorization_code".to_string()),
                ("client_id", client_id.clone()),
                ("code", code.to_string()),
                ("code_verifier", VERIFIER.to_string()),
                ("redirect_uri", CLAUDE.to_string()),
                ("resource", RESOURCE.to_string()),
            ]
        };
        for (field, value) in [
            (
                "code_verifier",
                "a-different-verifier-that-is-also-43-characters-long",
            ),
            ("code_verifier", "short"),
            ("redirect_uri", "https://claude.com/api/mcp/auth_callback"),
            ("resource", "https://elsewhere.example/mcp"),
        ] {
            let code = code_for(&request, &alice, &client_id).await;
            let mut fields = base(&code);
            fields.retain(|(k, _)| *k != field);
            fields.push((field, value.to_string()));
            let pairs: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
            let (status, body) = token(&request, &pairs).await;
            assert_eq!(
                (status, body["error"].clone()),
                (400, json!("invalid_grant")),
                "{field}={value}"
            );
        }

        // Reusing a code fails and revokes what it was swapped for.
        let code = code_for(&request, &alice, &client_id).await;
        let (_, first) = exchange(&request, &client_id, &code).await;
        let access = first["access_token"].as_str().unwrap();
        assert!(works(&request, access).await);
        let (status, body) = exchange(&request, &client_id, &code).await;
        assert_eq!(
            (status, body["error"].clone()),
            (400, json!("invalid_grant"))
        );
        assert!(!works(&request, access).await, "the grant is revoked");

        // Expired codes fail.
        let code = code_for(&request, &alice, &client_id).await;
        let row = oauth_codes::Entity::find()
            .filter(oauth_codes::Column::CodeHash.eq(access_tokens::hash(&code)))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        let mut row: oauth_codes::ActiveModel = row.into();
        row.expires_at =
            ActiveValue::Set((chrono::Utc::now() - chrono::Duration::seconds(1)).into());
        row.update(&ctx.db).await.unwrap();
        let (status, body) = exchange(&request, &client_id, &code).await;
        assert_eq!(
            (status, body["error"].clone()),
            (400, json!("invalid_grant"))
        );

        // The plain code is never stored.
        let stored = oauth_codes::Entity::find().all(&ctx.db).await.unwrap();
        assert!(stored.iter().all(|c| c.code_hash != code));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn racing_for_one_code_lets_one_win_then_revokes_it() {
    request::<App, _, _>(|request, _ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let client_id = register(&request, CLAUDE).await;
        let code = code_for(&request, &alice, &client_id).await;
        let (a, b) = tokio::join!(
            exchange(&request, &client_id, &code),
            exchange(&request, &client_id, &code)
        );
        let mut statuses = [a.0, b.0];
        statuses.sort_unstable();
        assert_eq!(statuses, [200, 400]);
        let winner = if a.0 == 200 { a.1 } else { b.1 };
        assert!(!works(&request, winner["access_token"].as_str().unwrap()).await);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn refresh_rotates_and_reuse_revokes_the_grant() {
    request::<App, _, _>(|request, _ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let client_id = register(&request, CLAUDE).await;
        let other_client = register(&request, CLAUDE).await;
        let code = code_for(&request, &alice, &client_id).await;
        let (_, first) = exchange(&request, &client_id, &code).await;
        let old_refresh = first["refresh_token"].as_str().unwrap();

        let (status, body) = refresh(&request, &other_client, old_refresh).await;
        assert_eq!(
            (status, body["error"].clone()),
            (400, json!("invalid_grant"))
        );
        let (status, body) = token(
            &request,
            &[
                ("grant_type", "refresh_token"),
                ("client_id", &client_id),
                ("refresh_token", old_refresh),
                ("resource", "https://elsewhere.example/mcp"),
            ],
        )
        .await;
        assert_eq!(
            (status, body["error"].clone()),
            (400, json!("invalid_grant"))
        );

        let (status, second) = refresh(&request, &client_id, old_refresh).await;
        assert_eq!(status, 200, "{second}");
        let new_access = second["access_token"].as_str().unwrap();
        assert_ne!(second["refresh_token"], first["refresh_token"]);
        assert!(works(&request, new_access).await);
        assert!(
            !works(&request, first["access_token"].as_str().unwrap()).await,
            "the rotated access token stops"
        );

        let (status, body) = refresh(&request, &client_id, old_refresh).await;
        assert_eq!(
            (status, body["error"].clone()),
            (400, json!("invalid_grant"))
        );
        assert!(
            !works(&request, new_access).await,
            "reuse revokes the grant"
        );
        let (status, _) = refresh(
            &request,
            &client_id,
            second["refresh_token"].as_str().unwrap(),
        )
        .await;
        assert_eq!(status, 400);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn racing_refreshes_let_one_win_then_revoke_it() {
    request::<App, _, _>(|request, _ctx| async move {
        let alice = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let client_id = register(&request, CLAUDE).await;
        let code = code_for(&request, &alice, &client_id).await;
        let (_, first) = exchange(&request, &client_id, &code).await;
        let old_refresh = first["refresh_token"].as_str().unwrap();
        let (a, b) = tokio::join!(
            refresh(&request, &client_id, old_refresh),
            refresh(&request, &client_id, old_refresh)
        );
        let mut statuses = [a.0, b.0];
        statuses.sort_unstable();
        assert_eq!(statuses, [200, 400]);
        let winner = if a.0 == 200 { a.1 } else { b.1 };
        assert!(!works(&request, winner["access_token"].as_str().unwrap()).await);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_super_admin_acting_elsewhere_is_asked_to_leave_first() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let acme = organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap();
        let admin = super_admin_cookie(&request, &ctx).await;
        let acting = format!("{}; acting_org={}", admin.1.to_str().unwrap(), acme.id);
        let client_id = register(&request, CLAUDE).await;

        let page = request
            .get(&authorize_url(&client_id, CLAUDE))
            .add_header("cookie", acting.clone())
            .await;
        assert_eq!(page.status_code(), 403);
        let body = page.text();
        assert!(body.contains("Leave Acme first"));
        assert!(body.contains(r#"action="/admin/leave""#));

        let mut fields = authorize_fields(&client_id, CLAUDE);
        fields.push(("decision", "allow".to_string()));
        let pairs: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let res = request
            .post("/oauth/authorize")
            .add_header("cookie", acting)
            .content_type("application/x-www-form-urlencoded")
            .bytes(form_body(&pairs))
            .await;
        assert_eq!(res.status_code(), 403);
        assert!(oauth_codes::Entity::find()
            .all(&ctx.db)
            .await
            .unwrap()
            .is_empty());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn login_next_survives_a_failed_login_and_refuses_other_places() {
    request::<App, _, _>(|request, _ctx| async move {
        sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let next = "/oauth/authorize?client_id=abc&state=1";
        let res = request
            .post("/login")
            .content_type("application/x-www-form-urlencoded")
            .bytes(form_body(&[
                ("email", "alice@example.com"),
                ("password", "wrong-password"),
                ("next", next),
            ]))
            .await;
        assert_eq!(res.status_code(), 422);
        assert!(res
            .text()
            .contains(r#"name="next" value="/oauth/authorize?client_id=abc&amp;state=1""#));

        for bad in [
            "//evil.example",
            "/\\evil.example",
            "/%5Cevil.example",
            "https://evil.example",
            "/dashboard",
        ] {
            let res = request
                .post("/login")
                .content_type("application/x-www-form-urlencoded")
                .bytes(form_body(&[
                    ("email", "alice@example.com"),
                    ("password", USER_PASSWORD),
                    ("next", bad),
                ]))
                .await;
            assert_eq!(location(res.headers()), "/", "{bad}");
            let page = request
                .get(&format!(
                    "/login?next={}",
                    url::form_urlencoded::byte_serialize(bad.as_bytes()).collect::<String>()
                ))
                .await
                .text();
            assert!(!page.contains(r#"name="next""#), "{bad}");
        }
    })
    .await;
}
