use collab::{
    app::App,
    models::{meeting_attendees, meetings, users},
    views::time,
};
use loco_rs::testing::prelude::*;
use sea_orm::{EntityTrait, PaginatorTrait};
use serial_test::serial;

use super::prepare_data::{join, sign_up};

#[tokio::test]
#[serial]
async fn new_meeting_form_defaults_to_today_and_ticks_me() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let alice = users::Model::find_by_email(&ctx.db, "alice@example.com")
            .await
            .unwrap();
        let res = request
            .get("/meetings/new")
            .add_header(owner.0, owner.1)
            .await;
        assert_eq!(res.status_code(), 200);
        let body = res.text();
        // The form uses the organisation's zone (London by default), not the
        // machine's, so CI near midnight UTC still agrees on the date.
        let today = time::today(time::zone("Europe/London"))
            .format("%Y-%m-%d")
            .to_string();
        assert!(body.contains(&format!(r#"value="{today}""#)), "{body}");
        assert!(
            body.contains(&format!(
                r#"value="{}" class="h-5 w-5 accent-[var(--accent)]" checked"#,
                alice.id
            )),
            "{body}"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn logging_a_meeting_saves_attendees_and_shows_it() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let alice = users::Model::find_by_email(&ctx.db, "alice@example.com").await.unwrap();

        let body = format!(
            "title=Weekly&held_on=2026-10-05&summary=Agreed+the+launch+scope&decisions=Ship+in+November&attendee_ids={0}&attendee_ids={0}",
            alice.id
        );
        let res = request
            .post("/meetings")
            .add_header(owner.0.clone(), owner.1.clone())
            .add_header("HX-Request", "true")
            .bytes(body.into())
            .content_type("application/x-www-form-urlencoded")
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        assert!(res.text().contains("Agreed the launch scope"));
        assert!(res.text().contains("05 Oct 2026"));

        let meeting = meetings::Entity::find().one(&ctx.db).await.unwrap().unwrap();
        assert_eq!(meeting.created_by_id, Some(alice.id));
        assert_eq!(meeting.decisions, "Ship in November");
        assert_eq!(meeting_attendees::Entity::find().count(&ctx.db).await.unwrap(), 1, "duplicates are dropped");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn meetings_need_minutes_a_date_and_team_attendees() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        join(&request, "acme", "bob", "bob@example.com").await;
        let pending_bob = users::Model::find_by_email(&ctx.db, "bob@example.com")
            .await
            .unwrap();

        let res = request
            .post("/meetings")
            .add_header(owner.0.clone(), owner.1.clone())
            .add_header("HX-Request", "true")
            .form(&serde_json::json!({ "title": "Weekly", "held_on": "", "summary": "" }))
            .await;
        assert_eq!(res.status_code(), 422);
        assert_eq!(res.headers().get("HX-Retarget").unwrap(), "#meeting-form");
        assert!(res.text().contains("Write the minutes"));

        let res = request
            .post("/meetings")
            .add_header(owner.0, owner.1)
            .add_header("HX-Request", "true")
            .bytes(
                format!(
                    "title=Weekly&held_on=2026-10-05&summary=Notes&attendee_ids={}",
                    pending_bob.id
                )
                .into(),
            )
            .content_type("application/x-www-form-urlencoded")
            .await;
        assert_eq!(res.status_code(), 422);
        assert!(res.text().contains("Attendees must be on the team."));
        assert_eq!(meetings::Entity::find().count(&ctx.db).await.unwrap(), 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn meetings_stay_inside_their_organisation() {
    request::<App, _, _>(|request, _ctx| async move {
        let acme = sign_up(&request, "Acme", "alice", "alice@example.com").await;
        let globex = sign_up(&request, "Globex", "gina", "gina@example.com").await;
        request
            .post("/meetings")
            .add_header(globex.0, globex.1)
            .form(&serde_json::json!({ "title": "Board meeting", "held_on": "2026-10-01", "summary": "Confidential plans" }))
            .await;
        let page = request.get("/meetings").add_header(acme.0, acme.1).await;
        assert!(!page.text().contains("Confidential plans"));
    })
    .await;
}
