//! Every app link carries its organisation. The walk renders the pages,
//! fragments and write responses and checks every link in them; the grep
//! catches templates and writes the walk doesn't reach.
use std::collections::BTreeSet;

use axum_test::multipart::{MultipartForm, Part};
use collab::{
    app::App,
    models::{
        conversations, documents, folders, memberships, messages, notifications, organisations,
        projects, saved_views, task_notes, tasks, users,
    },
};
use loco_rs::{app::AppContext, prelude::*, TestServer};
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;
use serial_test::serial;

use super::{
    multi_org::alice_in_two_orgs,
    prepare_data::{approve_as_owner, join, login, AsMember, TestMember, USER_PASSWORD},
    tasks::{project_in, task_form},
};

/// Bare paths a page may link to: they are not inside an org.
const ALLOWED: [&str; 8] = [
    "/static/",
    "/login",
    "/logout",
    "/account/sign-out-everywhere",
    "/admin",
    "/join/",
    "/oauth/",
    "/mcp",
];

/// Every `href`, `action`, `formaction`, `hx-*`, `ws-connect`, `sse-connect`
/// and `data-*url` value that starts with `/`.
fn links(html: &str) -> Vec<String> {
    let re = regex::Regex::new(
        r#"(?:href|action|formaction|hx-(?:get|post|put|patch|delete)|ws-connect|sse-connect|data-[a-z-]*url)="(/[^"]*)""#,
    )
    .unwrap();
    re.captures_iter(html).map(|c| c[1].to_string()).collect()
}

/// What the walk saw: each response's links, and every redirect.
#[derive(Default)]
struct Seen {
    bad: BTreeSet<String>,
    checked: usize,
}

impl Seen {
    /// A write's response: its links, and the status it must have.
    fn write(&mut self, what: &str, res: &axum_test::TestResponse, status: u16) {
        if res.status_code().as_u16() != status {
            self.bad.insert(format!(
                "{what}: status {}, expected {status}",
                res.status_code()
            ));
        }
        self.check(what, res, None);
    }

    /// `also` is another org whose address may appear (the switcher's rows).
    /// A 5xx is always bad: a template that failed to render, say.
    fn check(&mut self, what: &str, res: &axum_test::TestResponse, also: Option<&str>) {
        if res.status_code().is_server_error() {
            self.bad
                .insert(format!("{what}: status {}", res.status_code()));
        }
        let mut values = links(&res.text());
        for name in ["location", "hx-redirect"] {
            if let Some(v) = res.headers().get(name) {
                values.push(v.to_str().unwrap().to_string());
            }
        }
        for value in values {
            self.checked += 1;
            let inside = value.starts_with("/o/acme/")
                || also.is_some_and(|slug| value.starts_with(&format!("/o/{slug}/")));
            let allowed = value == "/" || ALLOWED.iter().any(|a| value.starts_with(a));
            if !inside && !allowed {
                self.bad.insert(format!("{what}: {value}"));
            }
        }
    }
}

struct World {
    alice: TestMember,
    bob: TestMember,
    task: tasks::Model,
    project: projects::Model,
    general: conversations::Model,
    hello: messages::Model,
    folder: folders::Model,
    logins: folders::Model,
    pending: memberships::Model,
}

async fn hx(req: axum_test::TestRequest) -> axum_test::TestResponse {
    req.add_header("hx-request", "true").await
}

/// One of everything in Acme, made through the app where a write's own
/// response is part of the walk.
async fn world(request: &TestServer, ctx: &AppContext, seen: &mut Seen) -> World {
    let (alice, _, _, _) = alice_in_two_orgs(request, ctx).await;
    let joined = join(request, ctx, "acme", "bobby", "bobby@example.com").await;
    let bob_user = users::Model::find_by_email(&ctx.db, "bobby@example.com")
        .await
        .unwrap();
    let pending = memberships::Model::default_for_user(&ctx.db, bob_user.id)
        .await
        .unwrap()
        .unwrap();
    approve_as_owner(ctx, &pending).await;
    let bob = TestMember::new(joined.cookie, alice.org_id, "acme");
    join(request, ctx, "acme", "carol", "carol@example.com").await;
    let carol = users::Model::find_by_email(&ctx.db, "carol@example.com")
        .await
        .unwrap();
    let pending = memberships::Model::default_for_user(&ctx.db, carol.id)
        .await
        .unwrap()
        .unwrap();

    let project = project_in(ctx, "acme", "Launch").await;
    let res = hx(request
        .post(&alice.at("/tasks"))
        .as_member(&alice)
        .form(&task_form(
            "Write the brief",
            project.id,
            &bob_user.id.to_string(),
        )))
    .await;
    seen.write("create a task", &res, 200);
    let task = tasks::Entity::find().one(&ctx.db).await.unwrap().unwrap();
    let res = hx(request
        .post(&alice.at(&format!("/tasks/{}", task.id)))
        .as_member(&alice)
        .form(&task_form("Write the brief again", project.id, "")))
    .await;
    seen.write("edit a task", &res, 200);
    let res = hx(request
        .post(&alice.at(&format!("/tasks/{}/status", task.id)))
        .as_member(&alice)
        .form(&json!({ "status": "progress" })))
    .await;
    seen.write("change a status", &res, 200);
    let res = hx(request
        .post(&bob.at(&format!("/tasks/{}/notes", task.id)))
        .as_member(&bob)
        .form(&json!({ "body": "Started @alice" })))
    .await;
    seen.write("add a note", &res, 200);
    let note = task_notes::Entity::find()
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    let res = hx(request
        .post(&alice.at(&format!("/tasks/{}/notes/{}/reactions", task.id, note.id)))
        .as_member(&alice)
        .form(&json!({ "emoji": "👍", "on": "1" })))
    .await;
    seen.write("react to a note", &res, 200);
    let res = hx(request
        .post(&alice.at("/tasks/views"))
        .as_member(&alice)
        .form(&json!({ "name": "Mine", "scope": "mine" })))
    .await;
    seen.write("save a view", &res, 200);

    let org = organisations::Model::find_by_slug(&ctx.db, "acme")
        .await
        .unwrap();
    let general = conversations::Model::find_general(&ctx.db, org.id)
        .await
        .unwrap();
    let res = hx(request
        .post(&alice.at(&format!("/chat/{}/messages", general.id)))
        .as_member(&alice)
        .form(&json!({ "body": "Hello @bobby" })))
    .await;
    seen.write("send a message", &res, 200);
    let hello = messages::Entity::find()
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    let res = hx(request
        .post(&bob.at(&format!(
            "/chat/{}/messages/{}/reactions",
            general.id, hello.id
        )))
        .as_member(&bob)
        .form(&json!({ "emoji": "🎉", "on": "1" })))
    .await;
    seen.write("react to a message", &res, 200);
    let res = hx(request
        .post(&bob.at(&format!(
            "/chat/{}/threads/{}/replies",
            general.id, hello.id
        )))
        .as_member(&bob)
        .form(&json!({ "body": "In the thread @alice" })))
    .await;
    seen.write("reply in a thread", &res, 200);
    let res = request
        .post(&alice.at("/chat/groups"))
        .as_member(&alice)
        .form(&json!({ "name": "leadership" }))
        .await;
    seen.write("create a group", &res, 303);
    let res = request
        .post(&alice.at("/chat/dms"))
        .as_member(&alice)
        .form(&json!({ "user_id": bob_user.id.to_string() }))
        .await;
    seen.write("start a DM", &res, 303);

    let res = hx(request
        .post(&alice.at("/documents/folders"))
        .as_member(&alice)
        .form(&json!({ "name": "Insurance", "note": "" })))
    .await;
    seen.write("create a folder", &res, 200);
    let all = folders::Model::list_for_org(&ctx.db, org.id).await.unwrap();
    let folder = all
        .iter()
        .find(|s| s.folder.name == "Insurance")
        .unwrap()
        .folder
        .clone();
    let logins = all
        .iter()
        .find(|s| s.folder.is_logins())
        .unwrap()
        .folder
        .clone();
    let mut pdf = b"%PDF-1.4\n".to_vec();
    pdf.resize(1024, b' ');
    let res = request
        .post(&alice.at(&format!("/documents/folders/{}/files", folder.id)))
        .as_member(&alice)
        .add_header("hx-request", "true")
        .multipart(MultipartForm::new().add_part("file", Part::bytes(pdf).file_name("policy.pdf")))
        .await;
    seen.write("upload a file", &res, 200);
    let doc = documents::Entity::find()
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    let res = hx(request
        .post(&alice.at(&format!("/documents/files/{}/rename", doc.id)))
        .as_member(&alice)
        .form(&json!({ "name": "renewal" })))
    .await;
    seen.write("rename a file", &res, 200);
    let res = hx(request
        .post(&alice.at(&format!("/documents/folders/{}/logins", logins.id)))
        .as_member(&alice)
        .form(&[
            ("title", "Bank"),
            ("username", "office@example.com"),
            ("url", "bank.example.com"),
            ("notes", ""),
            ("password", "pa55word-long"),
        ]))
    .await;
    seen.write("add a login", &res, 200);

    let res = hx(request
        .post(&alice.at("/roadmap/projects"))
        .as_member(&alice)
        .form(&json!({
            "name": "Website", "lane": "next", "status": "Planned",
            "accent": "#72e5b4", "owner_id": "", "summary": "",
        })))
    .await;
    seen.write("create a project", &res, 200);
    let res = hx(request
        .post(&alice.at("/meetings"))
        .as_member(&alice)
        .form(&json!({ "title": "Weekly", "held_on": "2026-10-05", "summary": "Agreed the launch scope" })))
    .await;
    seen.write("create a meeting", &res, 200);
    let bob_row = memberships::Model::active_in(&ctx.db, org.id, bob_user.id)
        .await
        .unwrap()
        .unwrap();
    let res = hx(request
        .post(&alice.at(&format!("/members/{}/role", bob_row.id)))
        .as_member(&alice)
        .form(&json!({ "role": "admin" })))
    .await;
    seen.write("change a role", &res, 200);
    // Approving is a write too; leave carol waiting for the Members page.
    let dana = join(request, ctx, "acme", "dana", "dana@example.com").await;
    let dana_row = memberships::Entity::find()
        .filter(memberships::Column::Username.eq("dana"))
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    let res = hx(request
        .post(&alice.at(&format!("/members/{}/approve", dana_row.id)))
        .as_member(&alice))
    .await;
    seen.write("approve a member", &res, 200);
    drop(dana);

    World {
        alice,
        bob,
        task,
        project,
        general,
        hello,
        folder,
        logins,
        pending,
    }
}

#[tokio::test]
#[serial]
async fn every_app_page_links_only_inside_its_org() {
    request::<App, _, _>(|request, ctx| async move {
        let mut seen = Seen::default();
        let w = world(&request, &ctx, &mut seen).await;
        let view = saved_views::Entity::find()
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        let login = collab::models::logins::Entity::find()
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        let removed = w.pending.id;
        let t = w.task.id;
        let c = w.general.id;
        let m = w.hello.id;
        let pages = [
            "/dashboard".to_string(),
            "/notifications".to_string(),
            "/tasks".to_string(),
            "/tasks?view=list".to_string(),
            "/tasks?filter=done".to_string(),
            format!("/tasks/{t}"),
            format!("/tasks?open={t}"),
            format!("/tasks/{t}/edit"),
            "/tasks/new".to_string(),
            format!("/tasks/views/{}/edit", view.id),
            "/tasks/views/new?scope=mine".to_string(),
            "/chat".to_string(),
            format!("/chat/{c}"),
            format!("/chat/{c}/threads/{m}"),
            format!("/chat/{c}/feed"),
            format!("/chat/{c}/messages/{m}/actions"),
            "/chat/groups/new".to_string(),
            "/chat/dms/new".to_string(),
            "/documents".to_string(),
            format!("/documents/folders/{}", w.folder.id),
            format!("/documents/folders/{}", w.logins.id),
            format!("/documents/logins/{}", login.id),
            "/documents/activity".to_string(),
            "/roadmap".to_string(),
            "/roadmap/list".to_string(),
            format!("/roadmap/projects/{}/edit", w.project.id),
            "/roadmap/projects/new".to_string(),
            "/meetings".to_string(),
            "/meetings/new".to_string(),
            "/members".to_string(),
            format!("/members/{removed}/rotate"),
            "/more".to_string(),
            "/settings/claude".to_string(),
            "/orgs?section=tasks".to_string(),
            "/orgs/new".to_string(),
            "/tasks/999999".to_string(),
        ];
        for path in &pages {
            let also = path.starts_with("/orgs?").then_some("beta");
            let page = request
                .get(&w.alice.at(path))
                .add_header(w.alice.0.clone(), w.alice.1.clone())
                .add_header("accept", "text/html")
                .await;
            assert!(
                page.status_code().is_success() || page.status_code() == 404,
                "{path}: {}",
                page.status_code()
            );
            seen.check(path, &page, also);
            let fragment = hx(request.get(&w.alice.at(path)).as_member(&w.alice)).await;
            seen.check(&format!("{path} (htmx)"), &fragment, also);
        }
        // The bob side of the mentions and the reply.
        let res = request
            .get(&w.bob.at("/notifications"))
            .add_header(w.bob.0.clone(), w.bob.1.clone())
            .await;
        seen.check("bob's notifications", &res, None);
        assert!(seen.bad.is_empty(), "{:#?}", seen.bad);
        assert!(seen.checked > 300, "only {} links checked", seen.checked);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn bell_links_carry_the_org() {
    request::<App, _, _>(|request, ctx| async move {
        let mut seen = Seen::default();
        let w = world(&request, &ctx, &mut seen).await;
        let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();
        let mine = notifications::Entity::find()
            .filter(notifications::Column::UserId.eq(alice.id))
            .all(&ctx.db)
            .await
            .unwrap();
        assert!(!mine.is_empty(), "the walk's mentions notify alice");
        for n in mine {
            assert!(
                n.link.starts_with('/') && !n.link.starts_with("/o/"),
                "{}",
                n.link
            );
            let res = request
                .post(&w.alice.at(&format!("/notifications/{}/open", n.id)))
                .as_member(&w.alice)
                .form(&json!({}))
                .await;
            assert_eq!(res.status_code(), 303);
            assert_eq!(
                res.headers()["location"].to_str().unwrap(),
                w.alice.at(&n.link),
                "the stored path stays org-relative and gets the prefix when shown"
            );
        }
        // A link that isn't a path inside the org opens nothing outside it.
        let odd = notifications::ActiveModel {
            organisation_id: ActiveValue::Set(w.alice.org_id),
            user_id: ActiveValue::Set(alice.id),
            kind: ActiveValue::Set("mention".to_string()),
            body: ActiveValue::Set("odd".to_string()),
            link: ActiveValue::Set("//evil.example.com".to_string()),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .unwrap();
        let res = request
            .post(&w.alice.at(&format!("/notifications/{}/open", odd.id)))
            .as_member(&w.alice)
            .form(&json!({}))
            .await;
        assert_eq!(res.headers()["location"], "/dashboard");
    })
    .await;
}

// ---------- The source grep ----------

/// Files under `dir` with `ext`, as `(path, text)`.
fn sources(dir: &str, ext: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut dirs = vec![std::path::PathBuf::from(dir)];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == ext) {
                let name = path.to_string_lossy().replace('\\', "/");
                out.push((name, std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    out
}

const SECTION: &str =
    "(?:dashboard|tasks|chat|documents|members|more|notifications|roadmap|meetings|settings|guide|orgs)";

/// Templates drawn for no org, whose bare links open the last org.
const BARE_TEMPLATES: [&str; 4] = [
    "assets/views/join/",
    "assets/views/tenancy/",
    "assets/views/errors/not_found_signed_in.html",
    "assets/views/errors/not_found_public.html",
];

/// Rust lines that may hold a bare tenant path, each for a reason.
const BARE_RUST: [(&str, &str); 22] = [
    // PR 2: mailers keep their links until email links name their org.
    ("src/mailers/", ""),
    // Org-relative paths stored or built by models, prefixed when shown.
    ("src/models/guide.rs", ""),
    ("src/models/saved_views.rs", ""),
    // Notification links: the ones a delete removes, and the bell notice.
    ("src/models/tasks.rs", "format!(\"/tasks?open={id}\")"),
    (
        "src/models/claude_update_notices.rs",
        "\"/settings/claude\"",
    ),
    ("src/controllers/notifications.rs", "let link = format!(\"/"),
    (
        "src/controllers/notifications.rs",
        "link: format!(\"/roadmap?lane=",
    ),
    (
        "src/controllers/chat.rs",
        "format!(\"/chat/{conversation_id}/threads/{parent_id}\")",
    ),
    // Bare on purpose: the address then picks the last org.
    ("src/controllers/page.rs", "redirect(\"/dashboard\")"),
    ("src/controllers/signup.rs", "redirect(\"/dashboard\")"),
    ("src/controllers/join.rs", "redirect(\"/dashboard\")"),
    ("src/controllers/join.rs", "\"/dashboard?requested={}\""),
    ("src/controllers/auth.rs", "\"/more?signed_out=1\""),
    ("src/controllers/auth.rs", "\"/dashboard\","),
    // Refusals and fallbacks with no org to name.
    // Bare Home when the claim check runs without an address (`?from=`
    // for the bare refusal), for not-a-member over HTMX, and when no org is
    // left. Then the bare address picks the org.
    (
        "src/extractors/current_member.rs",
        "\"/dashboard\".to_string()",
    ),
    (
        "src/extractors/current_member.rs",
        "map_or(\"/dashboard\", |pq|",
    ),
    (
        "src/extractors/current_member.rs",
        "format!(\"/dashboard?from={id}\")",
    ),
    (
        "src/extractors/current_member.rs",
        "\"/dashboard?from=\".to_string()",
    ),
    (
        "src/extractors/current_member.rs",
        "(\"HX-Redirect\", \"/dashboard\")",
    ),
    ("src/initializers/view_engine.rs", "\"/dashboard\""),
    ("src/views/links.rs", ""),
    // `enter_org` builds the address with `org_path` itself.
    ("src/controllers/orgs.rs", "enter_org("),
];

#[test]
fn no_hand_built_tenant_paths() {
    let mut bad = Vec::new();

    // A path in quotes, or right after a `{% ... %}` tag inside an attribute.
    let literal = regex::Regex::new(&format!(r#"(?:["']|%\}}|\}}\}})/{SECTION}\b"#)).unwrap();
    for caught in [
        r#"href="/tasks""#,
        "{% endif %}/tasks",
        "{{ base }}/tasks/1",
        "'/chat'",
    ] {
        assert!(literal.is_match(caught), "the grep misses {caught}");
    }
    for (path, text) in sources("assets/views", "html") {
        if BARE_TEMPLATES.iter().any(|b| path.starts_with(b)) {
            continue;
        }
        for m in literal.find_iter(&text) {
            let before = &text[..m.start()];
            let open = before.rfind("{{").max(before.rfind("{%"));
            let close = before.rfind("}}").max(before.rfind("%}"));
            let in_org_url = match (open, close) {
                (Some(open), close) if close.is_none_or(|c| open > c) => {
                    before[open..].contains("org_url(path=")
                }
                _ => false,
            };
            if !in_org_url {
                let line = text[..m.start()].lines().count();
                bad.push(format!("{path}:{line}"));
            }
        }
    }

    let rust = regex::Regex::new(&format!(r#""/{SECTION}\b"#)).unwrap();
    for (path, text) in sources("src", "rs") {
        // Unit tests may use any address.
        let code = text.split("#[cfg(test)]").next().unwrap_or_default();
        for (n, line) in code.lines().enumerate() {
            if !rust.is_match(line) {
                continue;
            }
            let routed = line.contains(".add(\"") || line.contains(".prefix(\"");
            let built = line.contains("org_path(") || line.contains(".url(");
            let allowed = BARE_RUST
                .iter()
                .any(|(file, part)| path.starts_with(file) && line.contains(part));
            if !routed && !built && !allowed && !line.trim_start().starts_with("//") {
                bad.push(format!("{path}:{}: {}", n + 1, line.trim()));
            }
        }
    }

    let ts = std::fs::read_to_string("frontend/app.ts").unwrap();
    let script = regex::Regex::new(&format!(r#"["`]/{SECTION}\b"#)).unwrap();
    for (n, line) in ts.lines().enumerate() {
        for m in script.find_iter(line) {
            let before = &line[..m.start()];
            let wrapped = before.ends_with("orgUrl(");
            // A CSS selector, not an address.
            let selector = before.ends_with("*=");
            let comment = line.trim_start().starts_with("//");
            if !wrapped && !selector && !comment {
                bad.push(format!("frontend/app.ts:{}: {}", n + 1, line.trim()));
            }
        }
    }

    // Tenant renders draw with `member.page`, `member.fragment` or (on the
    // socket) `session.fragment`, so templates get `org_base`.
    let bare_data = regex::Regex::new(
        r"(?s)\.(?:view|render)\(\s*&?v(?:iew)?,\s*[^,]+,\s*(?:data!|serde_json::json!|json!)\(",
    )
    .unwrap();
    let untenanted = ["auth", "signup", "join", "oauth", "admin", "page", "mcp"];
    for (path, text) in sources("src/controllers", "rs") {
        let name = path.rsplit('/').next().unwrap().trim_end_matches(".rs");
        if untenanted.contains(&name) {
            continue;
        }
        for m in bare_data.find_iter(&text) {
            let rest = &text[m.end()..text.len().min(m.end() + 600)];
            // `render_reactions` passes its org_base in the data itself.
            if !rest.contains("\"org_base\"") {
                let line = text[..m.start()].lines().count() + 1;
                bad.push(format!("{path}:{line}: render without org_base"));
            }
        }
    }

    assert!(bad.is_empty(), "{bad:#?}");
}

#[tokio::test]
#[serial]
async fn the_switcher_has_no_switch_form() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _, _, _) = alice_in_two_orgs(&request, &ctx).await;
        let list = hx(request.get(&alice.at("/orgs")).as_member(&alice))
            .await
            .text();
        assert!(!list.contains("/switch"), "{list}");
        assert!(!list.contains("<form"), "{list}");
        let _ = login(&request, "alice@example.com", USER_PASSWORD).await;
    })
    .await;
}

// ---------- The switcher, the org lines and the notice ----------

#[tokio::test]
#[serial]
async fn switcher_rows_link_to_the_same_section() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _, _, _) = alice_in_two_orgs(&request, &ctx).await;
        let project = project_in(&ctx, "acme", "Launch").await;
        hx(request
            .post(&alice.at("/tasks"))
            .as_member(&alice)
            .form(&task_form("Proofs", project.id, "")))
        .await;
        let task = tasks::Entity::find().one(&ctx.db).await.unwrap().unwrap();
        let page = request
            .get(&alice.at(&format!("/tasks?open={}", task.id)))
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(page.contains(r#"href="/o/acme/orgs?section=tasks""#), "the trigger");
        for (section, to) in [
            ("tasks", "/o/beta/tasks"),
            ("more", "/o/beta/more"),
            ("orgs", "/o/beta/dashboard"),
            ("claude", "/o/beta/settings/claude"),
            ("../../evil", "/o/beta/dashboard"),
        ] {
            let list = hx(request
                .get(&alice.at(&format!("/orgs?section={section}")))
                .as_member(&alice))
            .await
            .text();
            assert!(
                list.contains(&format!(r#"<a href="{to}" class="org-row" role="menuitemradio""#)),
                "{section}: {list}"
            );
            // The org you're in is a button with a tick.
            assert!(list.contains(r#"<button type="button" class="org-row" role="menuitemradio" aria-checked="true" data-org-current>"#));
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn multi_org_pages_carry_the_flag() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _, _, _) = alice_in_two_orgs(&request, &ctx).await;
        let page = request
            .get(&alice.at("/tasks"))
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(page.contains("data-multi-org"));
        assert!(page.contains(r#"<span class="org-line mobile-only truncate-1">Acme</span>"#));
        // Home names the org already.
        let home = request
            .get(&alice.at("/dashboard"))
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(!home.contains("org-line"));

        let solo =
            super::prepare_data::sign_up_member(&request, "Zen", "zed", "zed@example.com").await;
        let page = request
            .get(&solo.at("/tasks"))
            .add_header(solo.0.clone(), solo.1.clone())
            .await
            .text();
        assert!(!page.contains("data-multi-org"));
        assert!(!page.contains("org-line"));

        // The task sheet always draws its line; CSS shows it for two orgs.
        let project = project_in(&ctx, "zen", "Launch").await;
        hx(request
            .post(&solo.at("/tasks"))
            .as_member(&solo)
            .form(&task_form("Proofs", project.id, "")))
        .await;
        let task = tasks::Entity::find().one(&ctx.db).await.unwrap().unwrap();
        let sheet = hx(request
            .get(&solo.at(&format!("/tasks/{}", task.id)))
            .as_member(&solo))
        .await
        .text();
        assert!(sheet.contains(r#"class="meta sheet-org""#), "{sheet}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn there_is_still_no_get_that_switches() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _, _, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let res = request
            .get(&format!("/orgs/{}/switch", in_beta.organisation_id))
            .add_header(alice.0.clone(), alice.1.clone())
            .await;
        assert_eq!(res.status_code(), 405);
        assert!(res.headers().get("set-cookie").is_none());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn join_open_is_a_link() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _, _, _) = alice_in_two_orgs(&request, &ctx).await;
        let page = request
            .get("/join/beta")
            .add_header(alice.0.clone(), alice.1.clone())
            .await
            .text();
        assert!(page.contains(
            r#"<a href="/o/beta/dashboard" class="btn btn-primary btn-lg">Open Beta</a>"#
        ));
        assert!(!page.contains("/switch"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn admin_enter_lands_under_o() {
    request::<App, _, _>(|request, ctx| async move {
        super::prepare_data::sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let acme = organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap();
        let admin = super::admin::super_admin_cookie(&request, &ctx).await;
        let res = request
            .post(&format!("/admin/orgs/{}/enter", acme.id))
            .add_header(admin.0.clone(), admin.1.clone())
            .await;
        assert_eq!(res.status_code(), 303);
        assert_eq!(res.headers()["location"], "/o/acme/dashboard");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn home_notice_copy() {
    request::<App, _, _>(|request, ctx| async move {
        let (alice, _, _, in_beta) = alice_in_two_orgs(&request, &ctx).await;
        let home = |from: String| {
            let request = &request;
            let alice = alice.clone();
            async move {
                request
                    .get(&alice.at(&format!("/dashboard?from={from}")))
                    .add_header(alice.0.clone(), alice.1.clone())
                    .await
                    .text()
            }
        };
        let saved = "Your last change wasn't saved.";
        for from in [
            String::new(),
            in_beta.organisation_id.to_string(),
            "987654".to_string(),
        ] {
            let page = home(from.clone()).await;
            let start = page.find("data-stale-notice").expect("a notice");
            let notice = &page[start..start + page[start..].find("data-dismiss-notice").unwrap()];
            assert!(notice.contains(saved), "from={from}");
            assert!(notice.contains("Please make it again."), "from={from}");
            assert!(!notice.contains("Back to"), "no Back link: from={from}");
        }
        super::multi_org::set_status(&ctx, in_beta.clone(), memberships::status::REMOVED).await;
        let page = home(in_beta.organisation_id.to_string()).await;
        assert!(page.contains("You no longer have access to Beta."));
        assert!(!page.contains(saved));
    })
    .await;
}
