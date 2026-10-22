use collab::{
    app::App,
    models::{
        memberships, note_reactions, notifications, organisations, projects, task_assignees,
        task_notes::{self, NoteParams},
        tasks, users,
    },
};
use loco_rs::testing::prelude::*;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter,
};
use serial_test::serial;

use super::{
    admin::super_admin_cookie,
    prepare_data::{join, sign_up_member, AsMember, TestMember},
    tasks::{approved_member, project_in, task_form},
};

fn project_form(name: &str, lane: &str, owner_id: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name, "lane": lane, "status": "In progress",
        "accent": "#72e5b4", "owner_id": owner_id, "summary": "Ship the first version.",
    })
}

#[tokio::test]
#[serial]
async fn creating_a_project_shows_it_in_its_lane() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();

        let res = request
            .post(&owner.at("/roadmap/projects"))
            .as_member(&owner)
            .add_header("HX-Request", "true")
            .form(&project_form(
                "Design system",
                "next",
                &alice.id.to_string(),
            ))
            .await;
        assert_eq!(res.status_code(), 200);
        let lane = request
            .get(&owner.at("/roadmap?lane=next"))
            .add_header(owner.0.clone(), owner.1.clone())
            .await
            .text();
        assert!(lane.contains("Design system"));
        assert!(res
            .headers()
            .get("HX-Trigger")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("Project added"));

        let saved = projects::Entity::find()
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.lane, "next");
        assert_eq!(saved.owner_id, Some(alice.id));

        // Every lane is on the page (side by side on desktop). Phones see the
        // current lane, "Now" by default, and the switcher counts it under "Next".
        let page = request
            .get(&owner.at("/roadmap"))
            .add_header(owner.0.clone(), owner.1.clone())
            .await
            .text();
        let next = page.split(r#"data-lane="next""#).nth(1).unwrap();
        assert!(next
            .split("data-lane=")
            .next()
            .unwrap()
            .contains("Design system"));
        assert!(page.contains(r#"data-lane="now" data-current"#));
        assert!(page.contains("Next 1"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn invalid_projects_rerender_the_form_with_errors() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let mut form = project_form("", "someday", "");
        form["accent"] = serde_json::json!("red");

        let res = request
            .post(&owner.at("/roadmap/projects"))
            .as_member(&owner)
            .add_header("HX-Request", "true")
            .form(&form)
            .await;
        assert_eq!(res.status_code(), 422);
        assert_eq!(res.headers().get("HX-Retarget").unwrap(), "#project-form");
        let body = res.text();
        for message in [
            "Give the project a name",
            "Choose now, next or later.",
            "Choose a colour.",
        ] {
            assert!(body.contains(message), "{message}: {body}");
        }
        assert_eq!(projects::Entity::find().count(&ctx.db).await.unwrap(), 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn owner_must_be_an_approved_member_of_the_same_org() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        sign_up_member(&request, "Globex", "gina", "gina@example.com").await;
        join(&request, &ctx, "acme", "bob", "bob@example.com").await;
        let gina = users::Model::find_by_email(&ctx.db, "gina@example.com")
            .await
            .unwrap();
        let bob = users::Model::find_by_email(&ctx.db, "bob@example.com")
            .await
            .unwrap();

        for outsider in [gina.id, bob.id] {
            let res = request
                .post(&owner.at("/roadmap/projects"))
                .as_member(&owner)
                .add_header("HX-Request", "true")
                .form(&project_form("Launch", "now", &outsider.to_string()))
                .await;
            assert_eq!(res.status_code(), 422);
            assert!(res.text().contains("Choose someone from the team."));
        }
        assert_eq!(projects::Entity::find().count(&ctx.db).await.unwrap(), 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn editing_updates_the_project_and_the_form_is_prefilled() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        request
            .post(&owner.at("/roadmap/projects"))
            .as_member(&owner)
            .form(&project_form("Pricing", "now", ""))
            .await;
        let project = projects::Entity::find()
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();

        let edit = request
            .get(&owner.at(&format!("/roadmap/projects/{}/edit", project.id)))
            .add_header(owner.0.clone(), owner.1.clone())
            .await;
        assert_eq!(edit.status_code(), 200);
        assert!(edit.text().contains(r#"value="Pricing""#));

        let res = request
            .post(&owner.at(&format!("/roadmap/projects/{}", project.id)))
            .as_member(&owner)
            .form(&project_form("Pricing model", "later", ""))
            .await;
        assert_eq!(res.status_code(), 303);
        let saved = projects::Entity::find_by_id(project.id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.name, "Pricing model");
        assert_eq!(saved.lane, "later");
        assert_eq!(saved.owner_id, None);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn projects_from_other_orgs_are_invisible() {
    request::<App, _, _>(|request, ctx| async move {
        let acme = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let globex = sign_up_member(&request, "Globex", "gina", "gina@example.com").await;
        request
            .post(&globex.at("/roadmap/projects"))
            .as_member(&globex)
            .form(&project_form("Secret plan", "now", ""))
            .await;
        let secret = projects::Entity::find()
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();

        let page = request
            .get(&acme.at("/roadmap"))
            .add_header(acme.0.clone(), acme.1.clone())
            .await;
        assert!(!page.text().contains("Secret plan"));
        let edit = request
            .get(&acme.at(&format!("/roadmap/projects/{}/edit", secret.id)))
            .add_header(acme.0.clone(), acme.1.clone())
            .await;
        assert_eq!(edit.status_code(), 404);
        let update = request
            .post(&acme.at(&format!("/roadmap/projects/{}", secret.id)))
            .as_member(&acme)
            .form(&project_form("Hijacked", "now", ""))
            .await;
        assert_eq!(update.status_code(), 404);
        let unchanged = projects::Entity::find_by_id(secret.id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unchanged.name, "Secret plan");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn progress_is_the_share_of_tasks_done() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        for name in ["Launch", "Someday"] {
            request
                .post(&owner.at("/roadmap/projects"))
                .as_member(&owner)
                .form(&project_form(name, "now", ""))
                .await;
        }
        let launch = projects::Entity::find()
            .filter(projects::Column::Name.eq("Launch"))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        for (title, status) in [
            ("A", "done"),
            ("B", "progress"),
            ("C", "todo"),
            ("D", "blocked"),
        ] {
            request
                .post(&owner.at("/tasks"))
                .as_member(&owner)
                .form(&serde_json::json!({
                    "title": title, "project_id": launch.id.to_string(),
                    "priority": "medium", "status": status,
                }))
                .await;
        }

        let page = request
            .get(&owner.at("/roadmap"))
            .add_header(owner.0.clone(), owner.1.clone())
            .await
            .text();
        let compact: String = page.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            compact.contains(r#"aria-valuenow="25""#),
            "1 of 4 done is 25%"
        );
        assert!(compact.contains("1 of 4 tasks done"));
        assert!(compact.contains("No tasks yet"), "a project without tasks");
    })
    .await;
}

/// A project in Acme with two tasks, one assigned to `assignee` (who gets a
/// notification), a note and a reaction. Returns the project and task ids.
async fn project_with_work(
    request: &loco_rs::TestServer,
    ctx: &loco_rs::app::AppContext,
    owner: &TestMember,
    assignee: i64,
) -> (i64, Vec<i64>) {
    let project = project_in(ctx, "acme", "Spring range").await;
    for (title, who) in [
        ("Order incense", assignee.to_string()),
        ("Shoot photos", String::new()),
    ] {
        let res = request
            .post(&owner.at("/tasks"))
            .as_member(owner)
            .add_header("HX-Request", "true")
            .form(&task_form(title, project.id, &who))
            .await;
        assert!(res.status_code().is_success(), "{}", res.text());
    }
    let task_ids: Vec<i64> = tasks::Entity::find()
        .filter(tasks::Column::ProjectId.eq(project.id))
        .all(&ctx.db)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    let task = tasks::Entity::find_by_id(task_ids[0])
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    let note = task_notes::Model::create(
        &ctx.db,
        &task,
        assignee,
        &NoteParams {
            body: "On it".to_string(),
        },
    )
    .await
    .unwrap();
    note_reactions::Model::set(&ctx.db, &note, assignee, "👍", true)
        .await
        .unwrap();
    (project.id, task_ids)
}

async fn left_of(ctx: &loco_rs::app::AppContext, project: i64, task_ids: &[i64]) -> [u64; 5] {
    let links: Vec<String> = task_ids
        .iter()
        .map(|id| format!("/tasks?open={id}"))
        .collect();
    [
        projects::Entity::find_by_id(project)
            .count(&ctx.db)
            .await
            .unwrap(),
        tasks::Entity::find()
            .filter(tasks::Column::Id.is_in(task_ids.iter().copied()))
            .count(&ctx.db)
            .await
            .unwrap(),
        task_notes::Entity::find()
            .filter(task_notes::Column::TaskId.is_in(task_ids.iter().copied()))
            .count(&ctx.db)
            .await
            .unwrap(),
        task_assignees::Entity::find()
            .filter(task_assignees::Column::TaskId.is_in(task_ids.iter().copied()))
            .count(&ctx.db)
            .await
            .unwrap(),
        notifications::Entity::find()
            .filter(notifications::Column::Link.is_in(links))
            .count(&ctx.db)
            .await
            .unwrap(),
    ]
}

#[tokio::test]
#[serial]
async fn owners_and_admins_delete_a_project_with_its_tasks() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let (admin, admin_id) =
            approved_member(&request, &ctx, &owner, "bob", "bob@example.com").await;
        let (_, carol) =
            approved_member(&request, &ctx, &owner, "carol", "carol@example.com").await;
        let row = memberships::Model::active_in(&ctx.db, owner.org_id, admin_id)
            .await
            .unwrap()
            .unwrap();
        let mut row: memberships::ActiveModel = row.into();
        row.role = ActiveValue::Set(memberships::role::ADMIN.to_string());
        row.update(&ctx.db).await.unwrap();
        let admin = TestMember::new(admin.cookie, owner.org_id, "acme");
        let other = project_in(&ctx, "acme", "Keep me").await;

        // The edit form offers it, with the count in the confirm.
        let (project, task_ids) = project_with_work(&request, &ctx, &owner, carol).await;
        assert_eq!(left_of(&ctx, project, &task_ids).await, [1, 2, 1, 1, 1]);
        let form = request
            .get(&owner.at(&format!("/roadmap/projects/{project}/edit")))
            .as_member(&owner)
            .add_header("HX-Request", "true")
            .await
            .text();
        assert!(form.contains("Delete project"), "{form}");
        assert!(form.contains(
            "hx-confirm=\"Delete “Spring range” and its 2 tasks? Their notes go with them. \
This can&#39;t be undone.\""
        ));

        let res = request
            .post(&owner.at(&format!("/roadmap/projects/{project}/delete")))
            .as_member(&owner)
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let trigger = res.headers()["hx-trigger"].to_str().unwrap();
        assert!(trigger.contains("Project deleted") && trigger.contains("roadmap-changed"));
        assert!(trigger.contains("close-dialogs"));
        assert_eq!(left_of(&ctx, project, &task_ids).await, [0, 0, 0, 0, 0]);
        assert_eq!(
            note_reactions::Entity::find().count(&ctx.db).await.unwrap(),
            0
        );

        // An admin, without HTMX: a redirect to the roadmap.
        let (project, task_ids) = project_with_work(&request, &ctx, &owner, carol).await;
        let res = request
            .post(&admin.at(&format!("/roadmap/projects/{project}/delete")))
            .as_member(&admin)
            .await;
        assert_eq!(res.status_code(), 303, "{}", res.text());
        assert_eq!(res.headers()["location"], "/o/acme/roadmap");
        assert_eq!(left_of(&ctx, project, &task_ids).await, [0, 0, 0, 0, 0]);
        assert!(projects::Entity::find_by_id(other.id)
            .one(&ctx.db)
            .await
            .unwrap()
            .is_some());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn members_cannot_delete_projects() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let (member, carol) =
            approved_member(&request, &ctx, &owner, "carol", "carol@example.com").await;
        let member = TestMember::new(member.cookie, owner.org_id, "acme");
        let (project, task_ids) = project_with_work(&request, &ctx, &owner, carol).await;
        let form = request
            .get(&member.at(&format!("/roadmap/projects/{project}/edit")))
            .as_member(&member)
            .add_header("HX-Request", "true")
            .await
            .text();
        assert!(!form.contains("Delete project"));

        let res = request
            .post(&member.at(&format!("/roadmap/projects/{project}/delete")))
            .as_member(&member)
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 403);
        assert_eq!(left_of(&ctx, project, &task_ids).await, [1, 2, 1, 1, 1]);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_super_admin_acting_cannot_delete_a_project() {
    request::<App, _, _>(|request, ctx| async move {
        sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let acme = organisations::Model::find_by_slug(&ctx.db, "acme")
            .await
            .unwrap();
        let project = project_in(&ctx, "acme", "Launch").await;
        let admin = super_admin_cookie(&request, &ctx).await;
        let res = request
            .post(&format!("/o/acme/roadmap/projects/{}/delete", project.id))
            .add_header(
                "cookie",
                format!("{}; acting_org={}", admin.1.to_str().unwrap(), acme.id),
            )
            .add_header("x-org-id", acme.id.to_string())
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 403, "{}", res.text());
        assert!(res.text().contains("Leave admin mode"), "{}", res.text());
        assert_eq!(projects::Entity::find().count(&ctx.db).await.unwrap(), 1);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn another_orgs_project_is_not_found() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        sign_up_member(&request, "Globex", "gina", "gina@example.com").await;
        let theirs = project_in(&ctx, "globex", "Secret plan").await;
        let res = request
            .post(&owner.at(&format!("/roadmap/projects/{}/delete", theirs.id)))
            .as_member(&owner)
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 404);
        assert_eq!(projects::Entity::find().count(&ctx.db).await.unwrap(), 1);
    })
    .await;
}
