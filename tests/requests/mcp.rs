//! `/mcp`: bearer auth and the tools Claude calls, over JSON-RPC.
use collab::{
    app::App,
    models::{
        access_tokens::{self, PersonalParams},
        memberships, notifications, organisations, tasks, users,
    },
};
use loco_rs::{app::AppContext, testing::prelude::*, TestServer};
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{json, Value};
use serial_test::serial;

use super::{
    prepare_data::sign_up,
    tasks::{approved_member, project_in},
};

pub const RESOURCE: &str = "http://localhost:5150/mcp";

/// A personal token for the user with this email, made through the model.
pub async fn token_for(ctx: &AppContext, email: &str) -> String {
    let user = users::Model::find_by_email(&ctx.db, email).await.unwrap();
    let membership = memberships::Model::find_for_user(&ctx.db, user.id)
        .await
        .unwrap()
        .unwrap();
    access_tokens::Model::create_personal(
        &ctx.db,
        membership.organisation_id,
        user.id,
        &PersonalParams {
            name: "Laptop".to_string(),
        },
        RESOURCE,
    )
    .await
    .unwrap()
    .1
}

/// What came back from `/mcp`.
pub struct Reply {
    pub status: u16,
    pub www_authenticate: String,
    pub text: String,
}

impl Reply {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or_else(|_| panic!("not JSON: {}", self.text))
    }
}

/// Posts one JSON-RPC message to `/mcp`.
pub async fn rpc(request: &TestServer, token: &str, method: &str, params: Value) -> Reply {
    let res = request
        .post("/mcp")
        .add_header("authorization", format!("Bearer {token}"))
        .add_header("accept", "application/json, text/event-stream")
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
        .await;
    Reply {
        status: res.status_code().as_u16(),
        www_authenticate: res
            .headers()
            .get("www-authenticate")
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default(),
        text: res.text(),
    }
}

/// Calls a tool. Returns whether it was a tool error, and its text (parsed as
/// JSON when it is JSON).
pub async fn tool(request: &TestServer, token: &str, name: &str, args: Value) -> (bool, Value) {
    let res = rpc(
        request,
        token,
        "tools/call",
        json!({ "name": name, "arguments": args }),
    )
    .await;
    assert_eq!(res.status, 200, "{}", res.text);
    let body = res.json();
    let result = &body["result"];
    assert!(result.is_object(), "expected a result: {body}");
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    (
        result["isError"] == json!(true),
        serde_json::from_str(text).unwrap_or_else(|_| json!(text)),
    )
}

async fn user_id(ctx: &AppContext, email: &str) -> i64 {
    users::Model::find_by_email(&ctx.db, email)
        .await
        .unwrap()
        .id
}

async fn notices(ctx: &AppContext, user_id: i64, kind: &str) -> Vec<notifications::Model> {
    notifications::Entity::find()
        .filter(notifications::Column::UserId.eq(user_id))
        .filter(notifications::Column::Kind.eq(kind))
        .all(&ctx.db)
        .await
        .unwrap()
}

/// Edits the only token row for a test.
async fn change_token(ctx: &AppContext, edit: impl FnOnce(&mut access_tokens::ActiveModel)) {
    let row = access_tokens::Entity::find()
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    let mut row: access_tokens::ActiveModel = row.into();
    edit(&mut row);
    row.update(&ctx.db).await.unwrap();
}

#[tokio::test]
#[serial]
async fn no_token_gets_a_401_that_points_at_the_metadata() {
    request::<App, _, _>(|request, _ctx| async move {
        let res = request
            .post("/mcp")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
            .await;
        assert_eq!(res.status_code(), 401);
        assert_eq!(
            res.headers()["www-authenticate"].to_str().unwrap(),
            r#"Bearer resource_metadata="http://localhost:5150/.well-known/oauth-protected-resource", scope="tasks""#
        );

        let res = rpc(&request, "collab_pat_made-up", "tools/list", json!({})).await;
        assert_eq!(res.status, 401);
        assert!(res.www_authenticate.starts_with(r#"Bearer error="invalid_token""#));

        assert_eq!(request.get("/mcp").await.status_code(), 405);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn expired_revoked_and_wrong_resource_tokens_are_refused() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let token = token_for(&ctx, "alice@example.com").await;
        assert_eq!(
            rpc(&request, &token, "tools/list", json!({})).await.status,
            200
        );

        let past = chrono::Utc::now() - chrono::Duration::minutes(1);
        change_token(&ctx, |t| t.expires_at = ActiveValue::Set(Some(past.into()))).await;
        assert_eq!(
            rpc(&request, &token, "tools/list", json!({})).await.status,
            401
        );

        change_token(&ctx, |t| {
            t.expires_at = ActiveValue::Set(None);
            t.resource = ActiveValue::Set("https://elsewhere.example/mcp".to_string());
        })
        .await;
        assert_eq!(
            rpc(&request, &token, "tools/list", json!({})).await.status,
            401
        );

        change_token(&ctx, |t| {
            t.resource = ActiveValue::Set(RESOURCE.to_string());
            t.revoked_at = ActiveValue::Set(Some(chrono::Utc::now().into()));
        })
        .await;
        assert_eq!(
            rpc(&request, &token, "tools/list", json!({})).await.status,
            401
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn tokens_stop_working_without_an_active_membership() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let token = token_for(&ctx, "alice@example.com").await;
        let alice = user_id(&ctx, "alice@example.com").await;
        let membership = memberships::Model::find_for_user(&ctx.db, alice)
            .await
            .unwrap()
            .unwrap();

        // The app has no way to make a member pending again; flip it directly.
        let mut pending: memberships::ActiveModel = membership.clone().into();
        pending.status = ActiveValue::Set("pending".to_string());
        pending.update(&ctx.db).await.unwrap();
        assert_eq!(
            rpc(&request, &token, "tools/list", json!({})).await.status,
            401
        );

        // Nor to remove a member; delete the row directly.
        memberships::Entity::delete_by_id(membership.id)
            .exec(&ctx.db)
            .await
            .unwrap();
        assert_eq!(
            rpc(&request, &token, "tools/list", json!({})).await.status,
            401
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn browsers_from_other_sites_are_refused_and_no_origin_is_fine() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let token = token_for(&ctx, "alice@example.com").await;
        let res = request
            .post("/mcp")
            .add_header("authorization", format!("Bearer {token}"))
            .add_header("accept", "application/json, text/event-stream")
            .add_header("origin", "https://evil.example")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
            .await;
        assert_eq!(res.status_code(), 403);

        let res = rpc(&request, &token, "tools/list", json!({})).await;
        assert_eq!(res.status, 200);
        let row = access_tokens::Entity::find()
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert!(row.last_used_at.is_some(), "use is recorded");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn initialize_and_list_the_eight_tools() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let token = token_for(&ctx, "alice@example.com").await;
        let res = rpc(
            &request,
            &token,
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" }
            }),
        )
        .await;
        let body = res.json();
        assert_eq!(body["result"]["serverInfo"]["name"], "Collab Tool");
        assert_eq!(body["result"]["protocolVersion"], "2025-11-25");

        let body = rpc(&request, &token, "tools/list", json!({})).await.json();
        let mut names: Vec<&str> = body["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "add_task_note",
                "create_project",
                "create_task",
                "get_task",
                "list_members",
                "list_projects",
                "list_tasks",
                "update_task"
            ]
        );
        let list = body["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "list_tasks")
            .unwrap();
        assert_eq!(list["annotations"]["readOnlyHint"], true);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn create_project_fills_in_a_colour_and_tells_the_owner() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let (_, bob) = approved_member(&request, &ctx, &owner, "bob", "bob@example.com").await;
        let token = token_for(&ctx, "alice@example.com").await;

        let (error, project) = tool(
            &request,
            &token,
            "create_project",
            json!({ "name": "Collab Tool website", "lane": "now", "status": "Active", "owner": "Bob" }),
        )
        .await;
        assert!(!error, "{project}");
        assert_eq!(project["name"], "Collab Tool website");
        assert_eq!(project["owner"], "bob");
        let saved = collab::models::projects::Entity::find_by_id(project["id"].as_i64().unwrap())
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.accent, "#ffb454");
        assert_eq!(saved.owner_id, Some(bob));
        assert_eq!(notices(&ctx, bob, "owner").await.len(), 1);

        let (error, message) = tool(
            &request,
            &token,
            "create_project",
            json!({ "name": "Later", "lane": "someday", "status": "Planned" }),
        )
        .await;
        assert!(error);
        assert!(message.as_str().unwrap().contains("lane"), "{message}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn create_task_assigns_by_username_and_notifies() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let (_, bob) = approved_member(&request, &ctx, &owner, "bob", "bob@example.com").await;
        let (_, carol) =
            approved_member(&request, &ctx, &owner, "carol", "carol@example.com").await;
        let project = project_in(&ctx, "acme", "Launch").await;
        let mut owned: collab::models::projects::ActiveModel = project.clone().into();
        owned.owner_id = ActiveValue::Set(Some(carol));
        owned.update(&ctx.db).await.unwrap();
        let token = token_for(&ctx, "alice@example.com").await;

        let (error, task) = tool(
            &request,
            &token,
            "create_task",
            json!({ "title": "Write the launch email", "project_id": project.id, "assignees": ["bob", "me"] }),
        )
        .await;
        assert!(!error, "{task}");
        assert_eq!(task["status"], "todo");
        assert_eq!(task["priority"], "medium");
        assert_eq!(task["project"], "Launch");
        assert_eq!(task["assignees"], json!(["bob", "alice"]));
        let acme = organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap();
        let saved = tasks::Model::find_in_org(&ctx.db, acme.id, task["id"].as_i64().unwrap())
            .await
            .unwrap();
        assert_eq!(saved.organisation_id, acme.id);
        assert_eq!(notices(&ctx, bob, "assigned").await.len(), 1);
        assert_eq!(notices(&ctx, carol, "project").await.len(), 1);
        let alice = user_id(&ctx, "alice@example.com").await;
        assert!(
            notices(&ctx, alice, "assigned").await.is_empty(),
            "you are never notified about your own change"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn update_task_changes_only_what_it_is_given() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let (_, bob) = approved_member(&request, &ctx, &owner, "bob", "bob@example.com").await;
        let project = project_in(&ctx, "acme", "Launch").await;
        let mut owned: collab::models::projects::ActiveModel = project.clone().into();
        owned.owner_id = ActiveValue::Set(Some(bob));
        owned.update(&ctx.db).await.unwrap();
        let token = token_for(&ctx, "alice@example.com").await;
        let (_, task) = tool(
            &request,
            &token,
            "create_task",
            json!({
                "title": "Ship it", "project_id": project.id, "priority": "high",
                "due_on": "2026-11-03", "assignees": ["me"]
            }),
        )
        .await;
        let id = task["id"].as_i64().unwrap();
        let before = notices(&ctx, bob, "project").await.len();

        let (error, task) = tool(
            &request,
            &token,
            "update_task",
            json!({ "task_id": id, "status": "blocked" }),
        )
        .await;
        assert!(!error, "{task}");
        assert_eq!(task["status"], "blocked");
        assert_eq!(task["title"], "Ship it");
        assert_eq!(task["priority"], "high");
        assert_eq!(task["due_on"], "2026-11-03");
        assert_eq!(task["project_id"], project.id);
        assert_eq!(task["assignees"], json!(["alice"]));
        assert_eq!(notices(&ctx, bob, "project").await.len(), before + 1);

        let (_, task) = tool(
            &request,
            &token,
            "update_task",
            json!({ "task_id": id, "project_id": null, "due_on": null, "assignees": [] }),
        )
        .await;
        assert_eq!(task["project_id"], Value::Null);
        assert_eq!(task["due_on"], Value::Null);
        assert_eq!(task["assignees"], json!([]));
        assert_eq!(task["status"], "blocked");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn other_organisations_are_out_of_reach() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up(&request, "Acme", "alice", "alice@example.com").await;
        sign_up(&request, "Globex", "gina", "gina@example.com").await;
        let theirs = project_in(&ctx, "globex", "Secret plan").await;
        let gina = token_for(&ctx, "gina@example.com").await;
        let (_, secret) = tool(
            &request,
            &gina,
            "create_task",
            json!({ "title": "Secret task", "project_id": theirs.id }),
        )
        .await;
        let alice = token_for(&ctx, "alice@example.com").await;

        let (_, list) = tool(&request, &alice, "list_tasks", json!({})).await;
        assert_eq!(list["tasks"], json!([]));
        let (_, projects) = tool(&request, &alice, "list_projects", json!({})).await;
        assert_eq!(projects["projects"], json!([]));

        let (error, message) = tool(
            &request,
            &alice,
            "get_task",
            json!({ "task_id": secret["id"] }),
        )
        .await;
        assert!(error, "{message}");
        let (error, _) = tool(
            &request,
            &alice,
            "update_task",
            json!({ "task_id": secret["id"], "title": "Mine now" }),
        )
        .await;
        assert!(error);
        let (error, _) = tool(
            &request,
            &alice,
            "create_task",
            json!({ "title": "Sneaky", "project_id": theirs.id }),
        )
        .await;
        assert!(error, "a project from another org is refused");
        let (error, _) = tool(
            &request,
            &alice,
            "create_task",
            json!({ "title": "Sneaky", "assignees": ["gina"] }),
        )
        .await;
        assert!(error, "people from another org are refused");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn list_tasks_filters_and_list_members_has_no_emails() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        approved_member(&request, &ctx, &owner, "bob", "bob@example.com").await;
        let project = project_in(&ctx, "acme", "Launch").await;
        let token = token_for(&ctx, "alice@example.com").await;
        for (title, assignees, status) in [
            ("Write copy", json!(["bob"]), "todo"),
            ("Fix login", json!(["me"]), "blocked"),
            ("Tidy up", json!([]), "done"),
        ] {
            tool(
                &request,
                &token,
                "create_task",
                json!({ "title": title, "project_id": project.id, "assignees": assignees, "status": status }),
            )
            .await;
        }
        let titles = |list: &Value| -> Vec<String> {
            list["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["title"].as_str().unwrap().to_string())
                .collect()
        };
        let (_, list) = tool(&request, &token, "list_tasks", json!({ "assignee": "me" })).await;
        assert_eq!(titles(&list), ["Fix login"]);
        let (_, list) = tool(&request, &token, "list_tasks", json!({ "assignee": "bob" })).await;
        assert_eq!(titles(&list), ["Write copy"]);
        let (_, list) = tool(&request, &token, "list_tasks", json!({ "status": "done" })).await;
        assert_eq!(titles(&list), ["Tidy up"]);
        let (_, list) = tool(&request, &token, "list_tasks", json!({ "query": "FIX" })).await;
        assert_eq!(titles(&list), ["Fix login"]);
        let (_, list) = tool(&request, &token, "list_tasks", json!({ "limit": 2 })).await;
        assert_eq!(titles(&list).len(), 2);
        assert_eq!(list["more"], true);
        let (error, _) = tool(&request, &token, "list_tasks", json!({ "status": "nope" })).await;
        assert!(error);

        let (_, members) = tool(&request, &token, "list_members", json!({})).await;
        assert_eq!(
            members["members"],
            json!([
                { "username": "alice", "role": "owner", "you": true },
                { "username": "bob", "role": "member", "you": false },
            ])
        );
        assert!(!members.to_string().contains('@'));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn validation_problems_come_back_as_tool_errors() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let token = token_for(&ctx, "alice@example.com").await;
        let (error, message) = tool(
            &request,
            &token,
            "create_task",
            json!({ "title": "x".repeat(141) }),
        )
        .await;
        assert!(error);
        assert!(message.as_str().unwrap().starts_with("title:"), "{message}");

        let (error, message) = tool(
            &request,
            &token,
            "create_task",
            json!({ "title": "Fine", "assignees": ["nobody"] }),
        )
        .await;
        assert!(error);
        assert!(message.as_str().unwrap().contains("nobody"), "{message}");

        let (error, _) = tool(
            &request,
            &token,
            "create_task",
            json!({ "title": "Fine", "due_on": "next tuesday" }),
        )
        .await;
        assert!(error);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn notes_notify_mentioned_people() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let (_, bob) = approved_member(&request, &ctx, &owner, "bob", "bob@example.com").await;
        let token = token_for(&ctx, "alice@example.com").await;
        let (_, task) = tool(&request, &token, "create_task", json!({ "title": "Chore" })).await;

        let (error, note) = tool(
            &request,
            &token,
            "add_task_note",
            json!({ "task_id": task["id"], "body": "Can you look, @bob?" }),
        )
        .await;
        assert!(!error, "{note}");
        assert_eq!(note["author"], "alice");
        assert_eq!(notices(&ctx, bob, "mention").await.len(), 1);

        let (_, shown) = tool(
            &request,
            &token,
            "get_task",
            json!({ "task_id": task["id"] }),
        )
        .await;
        assert_eq!(shown["notes"][0]["body"], "Can you look, @bob?");
        assert_eq!(shown["notes"][0]["author"], "alice");

        let (error, _) = tool(
            &request,
            &token,
            "add_task_note",
            json!({ "task_id": task["id"], "body": "  " }),
        )
        .await;
        assert!(error, "an empty note is refused");
    })
    .await;
}
