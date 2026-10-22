//! The Claude connector for people in more than one organisation: every
//! answer names the connected org, and writes must name it too.
use collab::{
    app::App,
    models::{
        access_tokens::{self, PersonalParams},
        memberships, organisations, users,
    },
};
use loco_rs::{app::AppContext, prelude::*, TestServer};
use sea_orm::{ActiveModelTrait, ActiveValue};
use serde_json::{json, Value};
use serial_test::serial;

use super::{
    mcp::{rpc, RESOURCE},
    prepare_data::{create_user, sign_up_member, TestMember},
};

const NEW_PROTOCOL: &str = "2026-07-28";

/// Raj owns Himalayan Ritual and is an active member of Softly (owned by
/// Sam). Kreepa is only in Himalayan Ritual. Returns Raj's cookie.
async fn setup(request: &TestServer, ctx: &AppContext) -> TestMember {
    let raj = sign_up_member(request, "Himalayan Ritual", "raj", "raj@example.com").await;
    sign_up_member(request, "Softly", "sam", "sam@example.com").await;
    let raj_id = users::Model::find_by_email(&ctx.db, "raj@example.com")
        .await
        .unwrap()
        .id;
    add_member(ctx, "softly", raj_id, "raj").await;
    let kreepa = create_user(ctx, "kreepa@example.com", "Kreepa").await;
    add_member(ctx, "himalayan-ritual", kreepa.id, "kreepa").await;
    raj
}

/// An active member row, inserted directly.
async fn add_member(ctx: &AppContext, slug: &str, user_id: i64, username: &str) {
    let org = organisations::Model::find_by_slug(&ctx.db, slug)
        .await
        .unwrap();
    memberships::ActiveModel {
        user_id: ActiveValue::Set(user_id),
        username: ActiveValue::Set(username.to_string()),
        role: ActiveValue::Set(memberships::role::MEMBER.to_string()),
        status: ActiveValue::Set(memberships::status::ACTIVE.to_string()),
        approved_at: ActiveValue::Set(Some(chrono::Utc::now().into())),
        ..Default::default()
    }
    .set_tenant(org.id)
    .unwrap()
    .insert(&ctx.db)
    .await
    .unwrap();
}

/// A personal token for this person in the org with this slug.
async fn token_in(ctx: &AppContext, email: &str, slug: &str) -> String {
    let user = users::Model::find_by_email(&ctx.db, email).await.unwrap();
    let org = organisations::Model::find_by_slug(&ctx.db, slug)
        .await
        .unwrap();
    access_tokens::Model::create_personal(
        &ctx.db,
        org.id,
        user.id,
        &PersonalParams {
            name: format!("Laptop {slug}"),
        },
        RESOURCE,
    )
    .await
    .unwrap()
    .1
}

/// Posts one JSON-RPC request in the 2026-07-28 style: the version in the
/// header and in `_meta`, and the method in a header.
async fn rpc_new(request: &TestServer, token: &str, method: &str, mut params: Value) -> Value {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": NEW_PROTOCOL,
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": { "name": "test", "version": "1" },
    });
    let res = request
        .post("/mcp")
        .add_header("authorization", format!("Bearer {token}"))
        .add_header("accept", "application/json, text/event-stream")
        .add_header("mcp-protocol-version", NEW_PROTOCOL)
        .add_header("mcp-method", method)
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
        .await;
    assert_eq!(res.status_code(), 200, "{}", res.text());
    serde_json::from_str(&res.text()).unwrap()
}

#[tokio::test]
#[serial]
async fn initialize_names_the_org() {
    request::<App, _, _>(|request, ctx| async move {
        setup(&request, &ctx).await;
        for (slug, name) in [
            ("himalayan-ritual", "Himalayan Ritual"),
            ("softly", "Softly"),
        ] {
            let token = token_in(&ctx, "raj@example.com", slug).await;
            let body = rpc(
                &request,
                &token,
                "initialize",
                json!({
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": { "name": "test", "version": "1" }
                }),
            )
            .await
            .json();
            let instructions = body["result"]["instructions"].as_str().unwrap();
            assert!(
                instructions.starts_with(&format!("Connected to {name} ({slug}) as raj.")),
                "{instructions}"
            );
            assert_eq!(body["result"]["protocolVersion"], "2025-11-25");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn discover_names_the_org() {
    request::<App, _, _>(|request, ctx| async move {
        setup(&request, &ctx).await;
        let token = token_in(&ctx, "raj@example.com", "softly").await;
        let body = rpc_new(&request, &token, "server/discover", json!({})).await;
        let result = &body["result"];
        let instructions = result["instructions"]
            .as_str()
            .unwrap_or_else(|| panic!("{body}"));
        assert!(
            instructions.starts_with("Connected to Softly (softly) as raj."),
            "{instructions}"
        );
        assert_eq!(result["cacheScope"], "private");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn tools_list_is_private_and_names_the_slug() {
    request::<App, _, _>(|request, ctx| async move {
        setup(&request, &ctx).await;
        let token = token_in(&ctx, "raj@example.com", "himalayan-ritual").await;
        let body = rpc_new(&request, &token, "tools/list", json!({})).await;
        let result = &body["result"];
        assert_eq!(result["cacheScope"], "private", "{body}");
        let tools = result["tools"].as_array().unwrap();
        for tool in tools {
            let name = tool["name"].as_str().unwrap();
            let description = tool["description"].as_str().unwrap();
            let names_slug = description.contains("pass organisation \"himalayan-ritual\"");
            let writes = !name.starts_with("list_") && name != "get_task";
            assert_eq!(names_slug, writes, "{name}: {description}");
        }

        // The older protocol gets the same descriptions.
        let body = rpc(&request, &token, "tools/list", json!({})).await.json();
        let all = body["result"]["tools"].to_string();
        assert!(all.contains("Connected to Himalayan Ritual (himalayan-ritual)"));
    })
    .await;
}
