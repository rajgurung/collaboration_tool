//! Several owners per project: who can change them, who hears about it, and
//! what an owner can do. Juniper Tea Co: Ana (owner), Maya (admin), Tom and
//! Leo (members).
use collab::{
    app::App,
    models::{notifications, project_owners, projects, tasks},
};
use loco_rs::{app::AppContext, testing::prelude::*, TestServer};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serial_test::serial;

use super::{
    admin::super_admin_cookie,
    juniper::juniper,
    prepare_data::{AsMember, TestMember},
    roadmap_done::task_in,
    tasks::{own, project_in},
};

/// A project form as the sheet posts it, with these owners ticked.
fn project_body(name: &str, owners: &[i64]) -> axum::body::Bytes {
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    form.extend_pairs([
        ("name", name),
        ("lane", "now"),
        ("status", "Active"),
        ("accent", "#72e5b4"),
        ("summary", ""),
    ]);
    for id in owners {
        form.append_pair("owner_ids", &id.to_string());
    }
    form.finish().into()
}

async fn save(
    request: &TestServer,
    who: &TestMember,
    project: &projects::Model,
    owners: &[i64],
) -> u16 {
    request
        .post(&who.at(&format!("/roadmap/projects/{}", project.id)))
        .as_member(who)
        .add_header("HX-Request", "true")
        .content_type("application/x-www-form-urlencoded")
        .bytes(project_body(&project.name, owners))
        .await
        .status_code()
        .as_u16()
}

async fn owners(ctx: &AppContext, project: &projects::Model) -> Vec<i64> {
    project_owners::Model::for_project(&ctx.db, project.organisation_id, project.id)
        .await
        .unwrap()
}

/// `(kind, body)` of everything in someone's bell.
async fn inbox(ctx: &AppContext, user_id: i64) -> Vec<(String, String)> {
    notifications::Entity::find()
        .filter(notifications::Column::UserId.eq(user_id))
        .order_by_asc(notifications::Column::Id)
        .all(&ctx.db)
        .await
        .unwrap()
        .into_iter()
        .map(|n| (n.kind, n.body))
        .collect()
}

fn owner_notice(name: &str) -> (String, String) {
    ("owner".to_string(), format!("made you owner of {name}"))
}

async fn juniper_project(ctx: &AppContext, name: &str) -> projects::Model {
    project_in(ctx, "juniper-tea-co", name).await
}

#[tokio::test]
#[serial]
async fn an_admin_makes_two_owners_and_each_hears_once() {
    request::<App, _, _>(|request, ctx| async move {
        let t = juniper(&request, &ctx).await;
        let website = juniper_project(&ctx, "Website").await;

        assert_eq!(
            save(&request, &t.maya, &website, &[t.ana_id, t.leo_id]).await,
            200
        );
        assert_eq!(owners(&ctx, &website).await, vec![t.ana_id, t.leo_id]);
        assert_eq!(inbox(&ctx, t.ana_id).await, vec![owner_notice("Website")]);
        assert_eq!(inbox(&ctx, t.leo_id).await, vec![owner_notice("Website")]);
        assert!(inbox(&ctx, t.maya_id).await.is_empty());

        // The same owners again: nothing new. Taking Ana off: nothing.
        save(&request, &t.maya, &website, &[t.ana_id, t.leo_id]).await;
        save(&request, &t.maya, &website, &[t.leo_id]).await;
        assert_eq!(inbox(&ctx, t.ana_id).await.len(), 1);
        assert_eq!(inbox(&ctx, t.leo_id).await.len(), 1);
        // Adding Tom tells only Tom.
        save(&request, &t.maya, &website, &[t.leo_id, t.tom_id]).await;
        assert_eq!(inbox(&ctx, t.tom_id).await, vec![owner_notice("Website")]);
        assert_eq!(inbox(&ctx, t.leo_id).await.len(), 1);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn repeated_ids_save_once_and_one_outsider_refuses_the_list() {
    request::<App, _, _>(|request, ctx| async move {
        let t = juniper(&request, &ctx).await;
        let website = juniper_project(&ctx, "Website").await;

        save(&request, &t.ana, &website, &[t.leo_id, t.leo_id, t.tom_id]).await;
        assert_eq!(owners(&ctx, &website).await, vec![t.leo_id, t.tom_id]);
        assert_eq!(inbox(&ctx, t.leo_id).await, vec![owner_notice("Website")]);

        let res = request
            .post(&t.ana.at(&format!("/roadmap/projects/{}", website.id)))
            .as_member(&t.ana)
            .add_header("HX-Request", "true")
            .content_type("application/x-www-form-urlencoded")
            .bytes(project_body("Website", &[t.maya_id, 999_999]))
            .await;
        assert_eq!(res.status_code(), 422);
        assert!(res.text().contains("Choose people from the team."));
        assert_eq!(owners(&ctx, &website).await, vec![t.leo_id, t.tom_id]);
        assert!(inbox(&ctx, t.maya_id).await.is_empty());
    })
    .await;
}

/// S1: only admins and the project's owners change its owners. Anyone else's
/// edit saves, with the owners left as they were.
#[tokio::test]
#[serial]
async fn only_admins_and_owners_change_owners() {
    request::<App, _, _>(|request, ctx| async move {
        let t = juniper(&request, &ctx).await;
        let website = juniper_project(&ctx, "Website").await;
        own(&ctx, &website, &[t.ana_id, t.leo_id]).await;

        // Tom, not an owner, ticks himself: the edit saves, the owners don't.
        assert_eq!(save(&request, &t.tom, &website, &[t.tom_id]).await, 200);
        assert_eq!(owners(&ctx, &website).await, vec![t.ana_id, t.leo_id]);
        assert!(inbox(&ctx, t.tom_id).await.is_empty());
        let close = request
            .post(&t.tom.at(&format!("/roadmap/projects/{}/close", website.id)))
            .as_member(&t.tom)
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(close.status_code(), 403);
        assert!(close
            .text()
            .contains("Only this project's owners and admins can do that."));
        // His sheet shows the owners, but can't change them.
        let sheet = request
            .get(&t.tom.at(&format!("/roadmap/projects/{}/edit", website.id)))
            .as_member(&t.tom)
            .await
            .text();
        assert!(sheet.contains("Only this project's owners and admins can change owners."));
        assert!(sheet.contains("disabled"));
        assert!(!sheet.contains("Mark project done"));

        // Leo, an owner, can.
        assert_eq!(
            save(&request, &t.leo, &website, &[t.leo_id, t.tom_id]).await,
            200
        );
        assert_eq!(owners(&ctx, &website).await, vec![t.leo_id, t.tom_id]);
        let sheet = request
            .get(&t.leo.at(&format!("/roadmap/projects/{}/edit", website.id)))
            .as_member(&t.leo)
            .await
            .text();
        assert!(!sheet.contains("Only this project's owners and admins can change owners."));

        // A project with no owners: a member can't set them, an admin can.
        let shop = juniper_project(&ctx, "Instagram shop").await;
        save(&request, &t.tom, &shop, &[t.tom_id]).await;
        assert!(owners(&ctx, &shop).await.is_empty());
        save(&request, &t.maya, &shop, &[t.maya_id]).await;
        assert_eq!(owners(&ctx, &shop).await, vec![t.maya_id]);

        // A new project's maker picks its owners.
        let res = request
            .post(&t.tom.at("/roadmap/projects"))
            .as_member(&t.tom)
            .add_header("HX-Request", "true")
            .content_type("application/x-www-form-urlencoded")
            .bytes(project_body("Tea tasting evenings", &[t.tom_id, t.leo_id]))
            .await;
        assert_eq!(res.status_code(), 200);
        let tasting = projects::Entity::find()
            .filter(projects::Column::Name.eq("Tea tasting evenings"))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(owners(&ctx, &tasting).await, vec![t.tom_id, t.leo_id]);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_super_admin_acting_cant_change_owners() {
    request::<App, _, _>(|request, ctx| async move {
        let t = juniper(&request, &ctx).await;
        let website = juniper_project(&ctx, "Website").await;
        own(&ctx, &website, &[t.ana_id]).await;
        let admin = super_admin_cookie(&request, &ctx).await;
        let res = request
            .post(&t.ana.at(&format!("/roadmap/projects/{}", website.id)))
            .add_header(
                "cookie",
                format!("{}; acting_org={}", admin.1.to_str().unwrap(), t.org_id),
            )
            .add_header("x-org-id", t.org_id.to_string())
            .add_header("HX-Request", "true")
            .content_type("application/x-www-form-urlencoded")
            .bytes(project_body("Website", &[t.tom_id]))
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        assert_eq!(owners(&ctx, &website).await, vec![t.ana_id]);
        assert!(inbox(&ctx, t.tom_id).await.is_empty());

        // Nor on a new project: it is made with no owners.
        let res = request
            .post(&t.ana.at("/roadmap/projects"))
            .add_header(
                "cookie",
                format!("{}; acting_org={}", admin.1.to_str().unwrap(), t.org_id),
            )
            .add_header("x-org-id", t.org_id.to_string())
            .add_header("HX-Request", "true")
            .content_type("application/x-www-form-urlencoded")
            .bytes(project_body("Acting launch", &[t.tom_id]))
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let made = projects::Entity::find()
            .filter(projects::Column::Name.eq("Acting launch"))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert!(owners(&ctx, &made).await.is_empty());
        assert!(inbox(&ctx, t.tom_id).await.is_empty());
    })
    .await;
}

/// A non-owner's edit that fails shows the saved owners again, not the
/// ticks the server ignored.
#[tokio::test]
#[serial]
async fn a_failed_edit_shows_the_saved_owners_to_a_non_owner() {
    request::<App, _, _>(|request, ctx| async move {
        let t = juniper(&request, &ctx).await;
        let website = juniper_project(&ctx, "Website").await;
        own(&ctx, &website, &[t.leo_id]).await;
        let res = request
            .post(&t.tom.at(&format!("/roadmap/projects/{}", website.id)))
            .as_member(&t.tom)
            .add_header("HX-Request", "true")
            .content_type("application/x-www-form-urlencoded")
            .bytes(project_body("", &[]))
            .await;
        assert_eq!(res.status_code(), 422);
        let html = res.text();
        assert!(html.contains("Give the project a name"));
        assert!(
            html.contains(&format!(r#"name="owner_ids" value="{}" checked"#, t.leo_id)),
            "{html}"
        );
        assert_eq!(owners(&ctx, &website).await, vec![t.leo_id]);
    })
    .await;
}

/// More than 50 owners is refused before anything is looked up.
#[tokio::test]
#[serial]
async fn more_than_fifty_owners_are_refused() {
    request::<App, _, _>(|request, ctx| async move {
        let t = juniper(&request, &ctx).await;
        let website = juniper_project(&ctx, "Website").await;
        let many: Vec<i64> = (1..=51).collect();
        let res = request
            .post(&t.ana.at(&format!("/roadmap/projects/{}", website.id)))
            .as_member(&t.ana)
            .add_header("HX-Request", "true")
            .content_type("application/x-www-form-urlencoded")
            .bytes(project_body("Website", &many))
            .await;
        assert_eq!(res.status_code(), 422);
        assert!(res.text().contains("Choose up to 50 owners."));
        assert!(owners(&ctx, &website).await.is_empty());
    })
    .await;
}

/// Every owner can close and reopen, and hears about it once.
#[tokio::test]
#[serial]
async fn every_owner_can_close_and_each_hears_once() {
    request::<App, _, _>(|request, ctx| async move {
        let t = juniper(&request, &ctx).await;
        let website = juniper_project(&ctx, "Website").await;
        own(&ctx, &website, &[t.ana_id, t.leo_id]).await;
        task_in(
            &ctx,
            t.org_id,
            Some(website.id),
            "Order tins",
            "todo",
            &[t.ana_id],
        )
        .await;
        task_in(
            &ctx,
            t.org_id,
            Some(website.id),
            "Book the courier",
            "progress",
            &[t.ana_id],
        )
        .await;

        let lane = request
            .get(&t.tom.at("/roadmap"))
            .as_member(&t.tom)
            .await
            .text();
        assert!(
            lane.contains(r#"aria-label="Owners: ana and leo""#),
            "{lane}"
        );

        let close = request
            .post(&t.leo.at(&format!("/roadmap/projects/{}/close", website.id)))
            .as_member(&t.leo)
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(close.status_code(), 200, "{}", close.text());
        let ana = inbox(&ctx, t.ana_id).await;
        assert_eq!(
            ana.iter().filter(|(kind, _)| kind == "closed").count(),
            1,
            "{ana:?}"
        );
        assert!(ana.contains(&(
            "closed".to_string(),
            "marked Website done\n2 of your tasks closed".to_string()
        )));
        assert!(inbox(&ctx, t.tom_id).await.is_empty());

        let reopen = request
            .post(
                &t.leo
                    .at(&format!("/roadmap/projects/{}/reopen", website.id)),
            )
            .as_member(&t.leo)
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(reopen.status_code(), 200);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn every_owner_hears_about_new_and_blocked_tasks() {
    request::<App, _, _>(|request, ctx| async move {
        let t = juniper(&request, &ctx).await;
        let website = juniper_project(&ctx, "Website").await;
        own(&ctx, &website, &[t.ana_id, t.leo_id, t.tom_id]).await;
        let res = request
            .post(&t.tom.at("/tasks"))
            .as_member(&t.tom)
            .add_header("HX-Request", "true")
            .form(&super::tasks::task_form("Order tins", website.id, ""))
            .await;
        assert_eq!(res.status_code(), 200);
        let task = tasks::Entity::find().one(&ctx.db).await.unwrap().unwrap();
        request
            .post(&t.tom.at(&format!("/tasks/{}/status", task.id)))
            .as_member(&t.tom)
            .add_header("HX-Request", "true")
            .form(&serde_json::json!({ "status": "blocked" }))
            .await;
        for id in [t.ana_id, t.leo_id] {
            let kinds: Vec<String> = inbox(&ctx, id)
                .await
                .into_iter()
                .filter(|(kind, _)| kind == "project")
                .map(|(_, body)| body)
                .collect();
            assert_eq!(
                kinds,
                vec![
                    "added “Order tins” to Website".to_string(),
                    "marked “Order tins” as blocked in Website".to_string(),
                ]
            );
        }
        assert!(inbox(&ctx, t.tom_id).await.is_empty(), "never the actor");
    })
    .await;
}

/// S4: a removed member left as an owner hears nothing, and a removal takes
/// them off every project.
#[tokio::test]
#[serial]
async fn a_removed_owner_hears_nothing() {
    request::<App, _, _>(|request, ctx| async move {
        let t = juniper(&request, &ctx).await;
        let website = juniper_project(&ctx, "Website").await;
        own(&ctx, &website, &[t.leo_id, t.tom_id]).await;
        let tom = collab::models::memberships::Model::active_in(&ctx.db, t.org_id, t.tom_id)
            .await
            .unwrap()
            .unwrap();
        let res = request
            .post(&t.ana.at(&format!("/members/{}/remove", tom.id)))
            .as_member(&t.ana)
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        assert_eq!(owners(&ctx, &website).await, vec![t.leo_id]);

        // A row left behind (one removed before this change) tells no one.
        own(&ctx, &website, &[t.leo_id, t.tom_id]).await;
        let before = inbox(&ctx, t.tom_id).await.len();
        task_in(&ctx, t.org_id, Some(website.id), "Order tins", "todo", &[]).await;
        request
            .post(&t.ana.at("/tasks"))
            .as_member(&t.ana)
            .add_header("HX-Request", "true")
            .form(&super::tasks::task_form("Book the courier", website.id, ""))
            .await;
        request
            .post(&t.ana.at(&format!("/roadmap/projects/{}/close", website.id)))
            .as_member(&t.ana)
            .add_header("HX-Request", "true")
            .await;
        let shop = juniper_project(&ctx, "Instagram shop").await;
        save(&request, &t.ana, &shop, &[t.tom_id]).await;
        assert_eq!(inbox(&ctx, t.tom_id).await.len(), before);
        assert!(inbox(&ctx, t.leo_id).await.len() >= 2, "Leo still hears");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn cards_show_three_owners_then_a_count() {
    request::<App, _, _>(|request, ctx| async move {
        let t = juniper(&request, &ctx).await;
        let catalogue = juniper_project(&ctx, "Wholesale catalogue").await;
        own(&ctx, &catalogue, &[t.tom_id, t.ana_id, t.leo_id, t.maya_id]).await;
        let lane = request
            .get(&t.ana.at("/roadmap"))
            .as_member(&t.ana)
            .await
            .text();
        assert!(
            lane.contains(r#"aria-label="Owners: tom, ana, leo and maya""#),
            "{lane}"
        );
        assert!(lane.contains(">+1</span>"));
        let start = lane.find("owner-stack").unwrap();
        let stack = &lane[start..start + lane[start..].find("</span></span>").unwrap()];
        assert_eq!(stack.matches("avatar avatar-sm").count(), 4, "three and +1");
    })
    .await;
}

/// `projects.owner_id` is gone: owners come from `project_owners`. No code
/// or template may name it, and no template may read a single owner.
#[test]
fn nothing_names_the_dropped_owner_id() {
    let rust = regex::Regex::new(r"\bowner_id\b").unwrap();
    let template = regex::Regex::new(r"\bowner_id\b|\.owner\b").unwrap();
    let mut offenders = Vec::new();
    let mut dirs = vec![
        std::path::PathBuf::from("src"),
        std::path::PathBuf::from("assets/views"),
    ];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            let name = path.to_string_lossy().replace('\\', "/");
            let pattern = if name.starts_with("src/") {
                &rust
            } else {
                &template
            };
            let text = std::fs::read_to_string(&path).unwrap();
            for (n, line) in text.lines().enumerate() {
                if pattern.is_match(line) {
                    offenders.push(format!("{name}:{}: {}", n + 1, line.trim()));
                }
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:#?}");
}
