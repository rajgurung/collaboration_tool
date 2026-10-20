use collab::{
    app::App,
    models::{conversation_members, conversations, memberships, users},
};
use loco_rs::{app::AppContext, testing::prelude::*};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serial_test::serial;

use super::prepare_data::{approve_as_owner, join, sign_up_member, AsMember};

async fn membership_of(ctx: &AppContext, email: &str) -> memberships::Model {
    let user = users::Model::find_by_email(&ctx.db, email).await.unwrap();
    memberships::Model::default_for_user(&ctx.db, user.id)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[serial]
async fn owner_sees_join_link_and_pending_requests() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        join(&request, &ctx, "acme", "bob", "bob@example.com").await;

        let res = request
            .get("/members")
            .add_header(owner.0.clone(), owner.1.clone())
            .await;
        assert_eq!(res.status_code(), 200);
        let body = res.text();
        assert!(body.contains("/join/acme"), "{body}");
        assert!(body.contains(
            "People who use it join with a new or existing account, then wait for you or another admin to let them in."
        ));
        assert!(body.contains("bob@example.com"));
        assert!(body.contains("Approve"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn approving_gives_access_and_a_seat_in_general() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let bob = join(&request, &ctx, "acme", "bob", "bob@example.com").await;
        let pending = membership_of(&ctx, "bob@example.com").await;

        let res = request
            .post(&format!("/members/{}/approve", pending.id))
            .as_member(&owner)
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 200);
        assert!(res
            .headers()
            .get("HX-Trigger")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("bob approved."));

        let approved = membership_of(&ctx, "bob@example.com").await;
        assert_eq!(approved.status, "active");
        assert!(approved.approved_by_id.is_some());

        let general = conversations::Model::find_general(&ctx.db, approved.organisation_id)
            .await
            .unwrap();
        let seat = conversation_members::Entity::find()
            .filter(conversation_members::Column::ConversationId.eq(general.id))
            .filter(conversation_members::Column::UserId.eq(approved.user_id))
            .one(&ctx.db)
            .await
            .unwrap();
        assert!(seat.is_some());

        let res = request
            .get("/dashboard")
            .add_header(bob.0.clone(), bob.1.clone())
            .await;
        assert_eq!(res.status_code(), 200);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn approving_twice_reports_an_error_without_changes() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        join(&request, &ctx, "acme", "bob", "bob@example.com").await;
        let id = membership_of(&ctx, "bob@example.com").await.id;
        let path = format!("/members/{id}/approve");
        request.post(&path).as_member(&owner).await;

        let res = request
            .post(&path)
            .as_member(&owner)
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 200);
        let trigger = res
            .headers()
            .get("HX-Trigger")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(trigger.contains(r#""kind":"error""#), "{trigger}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn declining_shows_the_declined_page() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let bob = join(&request, &ctx, "acme", "bob", "bob@example.com").await;
        let id = membership_of(&ctx, "bob@example.com").await.id;

        let res = request
            .post(&format!("/members/{id}/reject"))
            .as_member(&owner)
            .await;
        assert_eq!(
            res.status_code(),
            303,
            "plain form posts go back to the members page"
        );
        assert_eq!(res.headers().get("location").unwrap(), "/members");

        let res = request
            .get("/dashboard")
            .add_header(bob.0.clone(), bob.1.clone())
            .await;
        assert!(res.text().contains("Request declined"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn members_cannot_approve_but_admins_can() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let bob = join(&request, &ctx, "acme", "bob", "bob@example.com").await;
        let bob_id = membership_of(&ctx, "bob@example.com").await.id;
        request
            .post(&format!("/members/{bob_id}/approve"))
            .as_member(&owner)
            .await;
        join(&request, &ctx, "acme", "carol", "carol@example.com").await;
        let carol_id = membership_of(&ctx, "carol@example.com").await.id;

        // Bob is a plain member: he cannot see or act on requests.
        let page = request
            .get("/members")
            .add_header(bob.0.clone(), bob.1.clone())
            .await;
        assert!(!page.text().contains("carol@example.com"));
        let denied = request
            .post(&format!("/members/{carol_id}/approve"))
            .as_member(&bob)
            .await;
        assert_eq!(denied.status_code(), 403);

        // Once Bob is an admin he can approve.
        request
            .post(&format!("/members/{bob_id}/role"))
            .as_member(&owner)
            .form(&serde_json::json!({ "role": "admin" }))
            .await;
        assert_eq!(membership_of(&ctx, "bob@example.com").await.role, "admin");
        let ok = request
            .post(&format!("/members/{carol_id}/approve"))
            .as_member(&bob)
            .await;
        assert_eq!(ok.status_code(), 303);
        assert_eq!(
            membership_of(&ctx, "carol@example.com").await.status,
            "active"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn role_changes_are_owner_only_and_never_touch_the_owner() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let bob = join(&request, &ctx, "acme", "bob", "bob@example.com").await;
        let bob_id = membership_of(&ctx, "bob@example.com").await.id;
        let alice_id = membership_of(&ctx, "alice@example.com").await.id;
        request
            .post(&format!("/members/{bob_id}/approve"))
            .as_member(&owner)
            .await;
        request
            .post(&format!("/members/{bob_id}/role"))
            .as_member(&owner)
            .form(&serde_json::json!({ "role": "admin" }))
            .await;

        // An admin cannot change roles.
        let res = request
            .post(&format!("/members/{alice_id}/role"))
            .as_member(&bob)
            .form(&serde_json::json!({ "role": "member" }))
            .await;
        assert_eq!(res.status_code(), 403);

        // The owner cannot demote themselves or set a made-up role.
        for (id, role) in [(alice_id, "member"), (bob_id, "owner")] {
            let res = request
                .post(&format!("/members/{id}/role"))
                .as_member(&owner)
                .add_header("HX-Request", "true")
                .form(&serde_json::json!({ "role": role }))
                .await;
            let trigger = res
                .headers()
                .get("HX-Trigger")
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            assert!(trigger.contains(r#""kind":"error""#), "{trigger}");
        }
        assert_eq!(membership_of(&ctx, "alice@example.com").await.role, "owner");
        assert_eq!(membership_of(&ctx, "bob@example.com").await.role, "admin");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn owners_cannot_touch_another_organisations_members() {
    request::<App, _, _>(|request, ctx| async move {
        let acme_owner = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        sign_up_member(&request, "Globex", "gina", "gina@example.com").await;
        join(&request, &ctx, "globex", "hank", "hank@example.com").await;
        let hank_id = membership_of(&ctx, "hank@example.com").await.id;

        for action in ["approve", "reject"] {
            let res = request
                .post(&format!("/members/{hank_id}/{action}"))
                .as_member(&acme_owner)
                .await;
            assert_eq!(res.status_code(), 404, "{action}");
        }
        assert_eq!(
            membership_of(&ctx, "hank@example.com").await.status,
            "pending"
        );
    })
    .await;
}

/// Bob joins Acme and is approved. Returns his membership.
async fn approved_bob(request: &loco_rs::TestServer, ctx: &AppContext) -> memberships::Model {
    join(request, ctx, "acme", "bob", "bob@example.com").await;
    let bob = membership_of(ctx, "bob@example.com").await;
    approve_as_owner(ctx, &bob).await
}

#[tokio::test]
#[serial]
async fn the_remove_dialog_says_access_ends_in_this_org() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        approved_bob(&request, &ctx).await;

        let page = request
            .get("/members")
            .add_header(owner.0.clone(), owner.1.clone())
            .await
            .text();
        assert!(page.contains("They lose access to Acme straight away, on every device."));
        assert!(page.contains(
            "If you think their account was taken over, ask them to reset their password."
        ));
        assert!(!page.contains("signed out on every device"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_members_flash_says_removed() {
    request::<App, _, _>(|request, ctx| async move {
        let owner = sign_up_member(&request, "Acme", "alice", "alice@example.com").await;
        let bob = approved_bob(&request, &ctx).await;
        let res = request
            .post(&format!("/members/{}/remove", bob.id))
            .as_member(&owner)
            .add_header("HX-Request", "true")
            .await;
        assert_eq!(res.status_code(), 200);

        let page = request
            .get(&format!("/members?removed={}", bob.id))
            .add_header(owner.0.clone(), owner.1.clone())
            .await
            .text();
        assert!(page.contains("bob was removed."), "{page}");
        assert!(!page.contains("signed out"));
    })
    .await;
}
