use collab::{
    app::App,
    models::{projects, users},
};
use loco_rs::testing::prelude::*;
use sea_orm::{EntityTrait, PaginatorTrait};
use serial_test::serial;

use super::prepare_data::{join, sign_up};

fn project_form(name: &str, lane: &str, owner_id: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name, "lane": lane, "status": "In progress", "progress": "40",
        "accent": "#72e5b4", "owner_id": owner_id, "summary": "Ship the first version.",
    })
}

#[tokio::test]
#[serial]
async fn creating_a_project_shows_it_in_its_lane() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();

        let res = request
            .post("/roadmap/projects")
            .add_header(owner.0.clone(), owner.1.clone())
            .add_header("HX-Request", "true")
            .form(&project_form(
                "Design system",
                "next",
                &alice.id.to_string(),
            ))
            .await;
        assert_eq!(res.status_code(), 200);
        assert!(res.text().contains(r#"id="roadmap-lanes""#));
        assert!(res.text().contains("Design system"));
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
        assert_eq!(saved.progress, 40);

        let page = request.get("/roadmap").add_header(owner.0, owner.1).await;
        assert!(page.text().contains("Design system"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn invalid_projects_rerender_the_form_with_errors() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let mut form = project_form("", "someday", "");
        form["progress"] = serde_json::json!("150");
        form["accent"] = serde_json::json!("red");

        let res = request
            .post("/roadmap/projects")
            .add_header(owner.0, owner.1)
            .add_header("HX-Request", "true")
            .form(&form)
            .await;
        assert_eq!(res.status_code(), 422);
        assert_eq!(res.headers().get("HX-Retarget").unwrap(), "#project-form");
        let body = res.text();
        for message in [
            "Give the project a name",
            "Choose now, next or later.",
            "0 to 100",
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
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        sign_up(&request, "Globex", "gina", "gina@example.com").await;
        join(&request, "acme", "bob", "bob@example.com").await;
        let gina = users::Model::find_by_email(&ctx.db, "gina@example.com")
            .await
            .unwrap();
        let bob = users::Model::find_by_email(&ctx.db, "bob@example.com")
            .await
            .unwrap();

        for outsider in [gina.id, bob.id] {
            let res = request
                .post("/roadmap/projects")
                .add_header(owner.0.clone(), owner.1.clone())
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
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        request
            .post("/roadmap/projects")
            .add_header(owner.0.clone(), owner.1.clone())
            .form(&project_form("Pricing", "now", ""))
            .await;
        let project = projects::Entity::find()
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();

        let edit = request
            .get(&format!("/roadmap/projects/{}/edit", project.id))
            .add_header(owner.0.clone(), owner.1.clone())
            .await;
        assert_eq!(edit.status_code(), 200);
        assert!(edit.text().contains(r#"value="Pricing""#));

        let res = request
            .post(&format!("/roadmap/projects/{}", project.id))
            .add_header(owner.0, owner.1)
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
        let acme = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let globex = sign_up(&request, "Globex", "gina", "gina@example.com").await;
        request
            .post("/roadmap/projects")
            .add_header(globex.0.clone(), globex.1.clone())
            .form(&project_form("Secret plan", "now", ""))
            .await;
        let secret = projects::Entity::find()
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();

        let page = request
            .get("/roadmap")
            .add_header(acme.0.clone(), acme.1.clone())
            .await;
        assert!(!page.text().contains("Secret plan"));
        let edit = request
            .get(&format!("/roadmap/projects/{}/edit", secret.id))
            .add_header(acme.0.clone(), acme.1.clone())
            .await;
        assert_eq!(edit.status_code(), 404);
        let update = request
            .post(&format!("/roadmap/projects/{}", secret.id))
            .add_header(acme.0, acme.1)
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
