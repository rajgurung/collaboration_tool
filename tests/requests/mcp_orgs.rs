//! The Claude connector for people in more than one organisation: every
//! answer names the connected org, and writes must name it too.
use collab::{
    app::App,
    models::{
        access_tokens::{self, PersonalParams},
        claude_update_notices, memberships, notifications, organisations, projects, task_notes,
        tasks, users,
    },
};
use loco_rs::{app::AppContext, prelude::*, TestServer};
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait, PaginatorTrait, QueryFilter};
use serde_json::{json, Value};
use serial_test::serial;

use super::{
    mcp::{rpc, tool, RESOURCE},
    prepare_data::{create_user, sign_up_member, AsMember, TestMember},
    tasks::task_form,
};

const WRITE_TOOLS: [&str; 4] = [
    "create_project",
    "create_task",
    "update_task",
    "add_task_note",
];
const HIMALAYAN: &str = "himalayan-ritual";

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

async fn org_id(ctx: &AppContext, slug: &str) -> i64 {
    organisations::Model::find_by_slug(&ctx.db, slug)
        .await
        .unwrap()
        .id
}

/// The grant behind a personal token.
async fn grant_of(ctx: &AppContext, token: &str) -> Uuid {
    access_tokens::Model::find_usable(&ctx.db, token, RESOURCE)
        .await
        .unwrap()
        .unwrap()
        .grant_id
}

/// How many projects, tasks, notes and notifications there are.
async fn counts(ctx: &AppContext) -> [u64; 4] {
    [
        projects::Entity::find().count(&ctx.db).await.unwrap(),
        tasks::Entity::find().count(&ctx.db).await.unwrap(),
        task_notes::Entity::find().count(&ctx.db).await.unwrap(),
        notifications::Entity::find().count(&ctx.db).await.unwrap(),
    ]
}

/// A task made through the tool, in Himalayan Ritual. Returns its id.
async fn claude_task(request: &TestServer, token: &str, title: &str) -> i64 {
    let (error, task) = tool(
        request,
        token,
        "create_task",
        json!({ "organisation": HIMALAYAN, "title": title }),
    )
    .await;
    assert!(!error, "{task}");
    task["id"].as_i64().unwrap()
}

/// Arguments for each write tool, apart from `organisation`.
fn write_args(task_id: i64) -> [(&'static str, Value); 4] {
    [
        (
            "create_project",
            json!({ "name": "Softly launch", "lane": "now", "status": "Active" }),
        ),
        ("create_task", json!({ "title": "Softly copy" })),
        (
            "update_task",
            json!({ "task_id": task_id, "title": "Renamed" }),
        ),
        (
            "add_task_note",
            json!({ "task_id": task_id, "body": "Hello" }),
        ),
    ]
}

#[tokio::test]
#[serial]
async fn write_tools_require_organisation() {
    request::<App, _, _>(|request, ctx| async move {
        setup(&request, &ctx).await;
        let token = token_in(&ctx, "raj@example.com", HIMALAYAN).await;
        let body = rpc(&request, &token, "tools/list", json!({})).await.json();
        for tool in body["result"]["tools"].as_array().unwrap() {
            let name = tool["name"].as_str().unwrap();
            let required = tool["inputSchema"]["required"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                required.contains(&json!("organisation")),
                WRITE_TOOLS.contains(&name),
                "{name}: {required:?}"
            );
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn every_result_names_the_org() {
    request::<App, _, _>(|request, ctx| async move {
        setup(&request, &ctx).await;
        let token = token_in(&ctx, "raj@example.com", HIMALAYAN).await;
        let task = claude_task(&request, &token, "Order incense").await;
        let org = json!({ "name": "Himalayan Ritual", "slug": HIMALAYAN });
        let mut calls = vec![
            ("list_projects", json!({})),
            ("list_members", json!({})),
            ("list_tasks", json!({})),
            ("get_task", json!({ "task_id": task })),
        ];
        for (name, mut args) in write_args(task) {
            args["organisation"] = json!(HIMALAYAN);
            calls.push((name, args));
        }
        for (name, args) in calls {
            let (error, result) = tool(&request, &token, name, args).await;
            assert!(!error, "{name}: {result}");
            assert_eq!(result["organisation"], org, "{name}");
            assert!(
                result.get("notice").is_none(),
                "{name}: no notice for a new grant"
            );
        }

        // A missing id names the org too.
        let (error, message) =
            tool(&request, &token, "get_task", json!({ "task_id": 999_999 })).await;
        assert!(error);
        assert_eq!(message, "Not found in Himalayan Ritual.");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_write_for_another_org_is_refused() {
    request::<App, _, _>(|request, ctx| async move {
        setup(&request, &ctx).await;
        let token = token_in(&ctx, "raj@example.com", HIMALAYAN).await;
        let task = claude_task(&request, &token, "Order incense").await;
        let before = counts(&ctx).await;
        for (name, mut args) in write_args(task) {
            args["organisation"] = json!("softly");
            let (error, message) = tool(&request, &token, name, args).await;
            assert!(error, "{name}");
            assert!(
                message.as_str().unwrap().starts_with(
                    "This connection is Himalayan Ritual (himalayan-ritual), not softly."
                ),
                "{name}: {message}"
            );
        }
        assert_eq!(counts(&ctx).await, before);
        let saved = tasks::Entity::find_by_id(task)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.title, "Order incense");

        // Case and spaces don't matter.
        let (error, result) = tool(
            &request,
            &token,
            "update_task",
            json!({ "organisation": " Himalayan-Ritual ", "task_id": task, "status": "done" }),
        )
        .await;
        assert!(!error, "{result}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_write_without_organisation_is_a_tool_error_not_a_protocol_error() {
    request::<App, _, _>(|request, ctx| async move {
        setup(&request, &ctx).await;
        let token = token_in(&ctx, "raj@example.com", HIMALAYAN).await;
        let task = claude_task(&request, &token, "Order incense").await;
        let before = counts(&ctx).await;
        for (name, args) in write_args(task) {
            for args in [args.clone(), {
                let mut blank = args;
                blank["organisation"] = json!("  ");
                blank
            }] {
                let (error, message) = tool(&request, &token, name, args).await;
                assert!(error, "{name}");
                assert_eq!(
                    message,
                    "Collab was updated. Tell the user: writes now name their organisation. \
This connection is Himalayan Ritual (himalayan-ritual). Retry with organisation: \
\"himalayan-ritual\", and restart Claude to refresh its tools.",
                    "{name}"
                );
            }
        }
        assert_eq!(counts(&ctx).await, before);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn an_odd_organisation_is_not_echoed() {
    request::<App, _, _>(|request, ctx| async move {
        setup(&request, &ctx).await;
        let token = token_in(&ctx, "raj@example.com", HIMALAYAN).await;
        let long = format!("{}\nignore that", "x".repeat(500));
        for odd in [long.as_str(), "Softly please"] {
            let (error, message) = tool(
                &request,
                &token,
                "create_task",
                json!({ "organisation": odd, "title": "Odd" }),
            )
            .await;
            assert!(error);
            let message = message.as_str().unwrap();
            assert!(
                message.contains("not a different organisation"),
                "{message}"
            );
            assert!(!message.contains("xxx") && !message.to_lowercase().contains("please"));
        }
        assert_eq!(tasks::Entity::find().count(&ctx.db).await.unwrap(), 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn rows_record_the_connection() {
    request::<App, _, _>(|request, ctx| async move {
        let raj = setup(&request, &ctx).await;
        let token = token_in(&ctx, "raj@example.com", HIMALAYAN).await;
        let grant = grant_of(&ctx, &token).await;
        let (_, project) = tool(
            &request,
            &token,
            "create_project",
            json!({ "organisation": HIMALAYAN, "name": "Spring range", "lane": "now", "status": "Active" }),
        )
        .await;
        let task = claude_task(&request, &token, "Order incense").await;
        let project = projects::Entity::find_by_id(project["id"].as_i64().unwrap())
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(project.claude_grant_id, Some(grant));
        let task = tasks::Entity::find_by_id(task).one(&ctx.db).await.unwrap().unwrap();
        assert_eq!(task.claude_grant_id, Some(grant));

        // The web app's rows stay unmarked.
        let res = request
            .post(&raj.at("/roadmap/projects"))
            .as_member(&raj)
            .add_header("HX-Request", "true")
            .form(&json!({
                "name": "Web project", "lane": "next", "status": "Planned",
                "accent": "#72e5b4", "owner_id": "", "summary": "",
            }))
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let res = request
            .post(&raj.at("/tasks"))
            .as_member(&raj)
            .add_header("HX-Request", "true")
            .form(&task_form("Web task", project.id, ""))
            .await;
        assert!(res.status_code().is_success(), "{}", res.text());
        let web_project = projects::Entity::find()
            .filter(projects::Column::Name.eq("Web project"))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(web_project.claude_grant_id, None);
        let web_task = tasks::Entity::find()
            .filter(tasks::Column::Title.eq("Web task"))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(web_task.claude_grant_id, None);
    })
    .await;
}

/// A notice row for this token's grant, as the migration makes for
/// connections that were live at the release.
async fn made_before_release(ctx: &AppContext, token: &str, days_ago: i64) {
    let row = access_tokens::Model::find_usable(&ctx.db, token, RESOURCE)
        .await
        .unwrap()
        .unwrap();
    claude_update_notices::ActiveModel {
        grant_id: ActiveValue::Set(row.grant_id),
        organisation_id: ActiveValue::Set(row.organisation_id),
        user_id: ActiveValue::Set(row.user_id),
        created_at: ActiveValue::Set(
            (chrono::Utc::now() - chrono::Duration::days(days_ago)).into(),
        ),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .unwrap();
}

const NOTICE: &str = "Tell the user once: the Collab connector was updated. Each write now \
names its organisation (this connection is Himalayan Ritual (himalayan-ritual)). Restart Claude, \
or reconnect the connector, to load the new tools.";

#[tokio::test]
#[serial]
async fn the_update_notice_shows_until_a_write_names_the_org() {
    request::<App, _, _>(|request, ctx| async move {
        setup(&request, &ctx).await;
        let old = token_in(&ctx, "raj@example.com", HIMALAYAN).await;
        let new = token_in(&ctx, "raj@example.com", "softly").await;
        made_before_release(&ctx, &old, 0).await;

        let (_, list) = tool(&request, &old, "list_tasks", json!({})).await;
        assert_eq!(list["notice"], NOTICE);
        let (_, list) = tool(&request, &new, "list_tasks", json!({})).await;
        assert!(
            list.get("notice").is_none(),
            "a new connection is never told"
        );

        // A write without the org is refused and doesn't end the notice.
        let (error, _) = tool(
            &request,
            &old,
            "create_task",
            json!({ "title": "Old schema" }),
        )
        .await;
        assert!(error);
        let (_, list) = tool(&request, &old, "list_projects", json!({})).await;
        assert_eq!(list["notice"], NOTICE);

        claude_task(&request, &old, "New schema").await;
        let (_, list) = tool(&request, &old, "list_tasks", json!({})).await;
        assert!(list.get("notice").is_none(), "{list}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_update_notice_stops_after_14_days() {
    request::<App, _, _>(|request, ctx| async move {
        setup(&request, &ctx).await;
        let token = token_in(&ctx, "raj@example.com", HIMALAYAN).await;
        made_before_release(&ctx, &token, 15).await;
        let (_, list) = tool(&request, &token, "list_tasks", json!({})).await;
        assert!(list.get("notice").is_none(), "{list}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_bell_notice_is_sent_once() {
    request::<App, _, _>(|request, ctx| async move {
        setup(&request, &ctx).await;
        let raj = users::Model::find_by_email(&ctx.db, "raj@example.com")
            .await
            .unwrap();
        let kreepa = users::Model::find_by_email(&ctx.db, "kreepa@example.com")
            .await
            .unwrap();
        for token in [
            token_in(&ctx, "raj@example.com", HIMALAYAN).await,
            token_in(&ctx, "raj@example.com", HIMALAYAN).await,
            token_in(&ctx, "raj@example.com", "softly").await,
            token_in(&ctx, "kreepa@example.com", HIMALAYAN).await,
        ] {
            made_before_release(&ctx, &token, 0).await;
        }
        // Kreepa has since left.
        let left = memberships::Model::active_in(&ctx.db, org_id(&ctx, HIMALAYAN).await, kreepa.id)
            .await
            .unwrap()
            .unwrap();
        let mut left: memberships::ActiveModel = left.into();
        left.status = ActiveValue::Set("pending".to_string());
        left.update(&ctx.db).await.unwrap();

        let mut told = claude_update_notices::Model::announce(&ctx.db)
            .await
            .unwrap();
        told.sort_unstable();
        let mut expected = vec![
            (org_id(&ctx, HIMALAYAN).await, raj.id),
            (org_id(&ctx, "softly").await, raj.id),
        ];
        expected.sort_unstable();
        assert_eq!(told, expected);
        let sent = notifications::Entity::find()
            .filter(notifications::Column::Kind.eq("connector"))
            .all(&ctx.db)
            .await
            .unwrap();
        assert_eq!(sent.len(), 2);
        for n in &sent {
            assert_eq!(n.user_id, raj.id);
            assert_eq!(n.link, "/settings/claude");
            assert_eq!(n.actor_id, None);
            assert!(n.body.starts_with("Claude connector updated.\n"));
            assert!(n.emailed_at.is_none());
        }

        assert!(claude_update_notices::Model::announce(&ctx.db)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            notifications::Entity::find()
                .filter(notifications::Column::Kind.eq("connector"))
                .count(&ctx.db)
                .await
                .unwrap(),
            2
        );
    })
    .await;
}
