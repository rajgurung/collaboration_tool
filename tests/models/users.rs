use chrono::{offset::Local, Duration};
use collab::{
    app::App,
    models::{
        access_tokens::{self, PersonalParams},
        oauth_clients,
        organisations::{self, SignupParams},
        users::{self, Model, RegisterParams},
    },
};
use insta::assert_debug_snapshot;
use loco_rs::testing::prelude::*;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, IntoActiveModel, PaginatorTrait,
    QueryFilter, QuerySelect, TransactionTrait,
};
use serial_test::serial;

use super::memberships::{
    alice_in_two_orgs, claude_client, code_for, exchange, oauth_tokens, personal_token,
    second_connection, usable, HOLD, RESOURCE,
};

macro_rules! configure_insta {
    ($($expr:expr),*) => {
        let mut settings = insta::Settings::clone_current();
        settings.set_prepend_module_to_snapshot(false);
        settings.set_snapshot_suffix("users");
        let _guard = settings.bind_to_scope();
    };
}

#[tokio::test]
#[serial]
async fn test_can_validate_model() {
    configure_insta!();

    let boot = boot_test::<App>()
        .await
        .expect("Failed to boot test application");

    let invalid_user = users::ActiveModel {
        name: ActiveValue::set("1".to_string()),
        email: ActiveValue::set("invalid-email".to_string()),
        ..Default::default()
    };

    let res = invalid_user.insert(&boot.app_context.db).await;

    assert_debug_snapshot!(res);
}

#[tokio::test]
#[serial]
async fn everyone_is_a_reserved_name() {
    let boot = boot_test::<App>()
        .await
        .expect("Failed to boot test application");

    let user = users::ActiveModel {
        name: ActiveValue::set("EveryOne".to_string()),
        email: ActiveValue::set("e@example.com".to_string()),
        ..Default::default()
    };

    let err = user.insert(&boot.app_context.db).await.unwrap_err();
    assert!(err.to_string().contains("That name is reserved."), "{err}");
}

#[tokio::test]
#[serial]
async fn can_create_with_password() {
    configure_insta!();

    let boot = boot_test::<App>()
        .await
        .expect("Failed to boot test application");

    let params = RegisterParams {
        email: "test@framework.com".to_string(),
        password: "12341234".to_string(),
        name: "framework".to_string(),
    };

    let user = Model::create_with_password(&boot.app_context.db, &params)
        .await
        .expect("a user should be created");

    // Snapshot only the fields this test is about, never the whole `Model`.
    // A whole-model snapshot encodes every column, so adding one field to
    // `users` — the first thing most apps do — fails every such test at once
    // and buries the one real change in a pile of mechanical re-blessing.
    // Anything the snapshot does not cover, assert directly:
    assert_ne!(
        user.password, params.password,
        "the password must be stored hashed, never in the clear"
    );
    assert_debug_snapshot!((user.email, user.name));
}
#[tokio::test]
#[serial]
async fn handle_create_with_password_with_duplicate() {
    configure_insta!();

    let boot = boot_test::<App>()
        .await
        .expect("Failed to boot test application");
    seed::<App>(&boot.app_context)
        .await
        .expect("Failed to seed database");

    let new_user = Model::create_with_password(
        &boot.app_context.db,
        &RegisterParams {
            email: "user1@example.com".to_string(),
            password: "12341234".to_string(),
            name: "framework".to_string(),
        },
    )
    .await;

    assert_debug_snapshot!(new_user);
}

#[tokio::test]
#[serial]
async fn can_find_by_email() {
    configure_insta!();

    let boot = boot_test::<App>()
        .await
        .expect("Failed to boot test application");
    seed::<App>(&boot.app_context)
        .await
        .expect("Failed to seed database");

    let existing_user = Model::find_by_email(&boot.app_context.db, "user1@example.com").await;
    let non_existing_user_results =
        Model::find_by_email(&boot.app_context.db, "un@existing-email.com").await;

    // Narrowed on purpose — see `can_create_with_password` above.
    assert_debug_snapshot!(existing_user.map(|user| (user.email, user.name)));
    assert_debug_snapshot!(non_existing_user_results.map(|user| (user.email, user.name)));
}

#[tokio::test]
#[serial]
async fn can_find_by_pid() {
    configure_insta!();

    let boot = boot_test::<App>()
        .await
        .expect("Failed to boot test application");
    seed::<App>(&boot.app_context)
        .await
        .expect("Failed to seed database");

    let existing_user =
        Model::find_by_pid(&boot.app_context.db, "11111111-1111-1111-1111-111111111111").await;
    let non_existing_user_results =
        Model::find_by_pid(&boot.app_context.db, "23232323-2323-2323-2323-232323232323").await;

    // Narrowed on purpose — see `can_create_with_password` above.
    assert_debug_snapshot!(existing_user.map(|user| (user.pid, user.email)));
    assert_debug_snapshot!(non_existing_user_results.map(|user| (user.pid, user.email)));
}

#[tokio::test]
#[serial]
async fn can_verification_token() {
    configure_insta!();

    let boot = boot_test::<App>()
        .await
        .expect("Failed to boot test application");
    seed::<App>(&boot.app_context)
        .await
        .expect("Failed to seed database");

    let user = Model::find_by_pid(&boot.app_context.db, "11111111-1111-1111-1111-111111111111")
        .await
        .expect("Failed to find user by PID");

    assert!(
        user.email_verification_sent_at.is_none(),
        "Expected no email verification sent timestamp"
    );
    assert!(
        user.email_verification_token.is_none(),
        "Expected no email verification token"
    );

    let result = user
        .into_active_model()
        .set_email_verification_sent(&boot.app_context.db)
        .await;

    assert!(result.is_ok(), "Failed to set email verification sent");

    let user = Model::find_by_pid(&boot.app_context.db, "11111111-1111-1111-1111-111111111111")
        .await
        .expect("Failed to find user by PID after setting verification sent");

    assert!(
        user.email_verification_sent_at.is_some(),
        "Expected email verification sent timestamp to be present"
    );
    assert!(
        user.email_verification_token.is_some(),
        "Expected email verification token to be present"
    );
}

#[tokio::test]
#[serial]
async fn can_set_forgot_password_sent() {
    configure_insta!();

    let boot = boot_test::<App>()
        .await
        .expect("Failed to boot test application");
    seed::<App>(&boot.app_context)
        .await
        .expect("Failed to seed database");

    let user = Model::find_by_pid(&boot.app_context.db, "11111111-1111-1111-1111-111111111111")
        .await
        .expect("Failed to find user by PID");

    assert!(
        user.reset_sent_at.is_none(),
        "Expected no reset sent timestamp"
    );
    assert!(user.reset_token.is_none(), "Expected no reset token");

    let result = user
        .into_active_model()
        .set_forgot_password_sent(&boot.app_context.db)
        .await;

    assert!(result.is_ok(), "Failed to set forgot password sent");

    let user = Model::find_by_pid(&boot.app_context.db, "11111111-1111-1111-1111-111111111111")
        .await
        .expect("Failed to find user by PID after setting forgot password sent");

    assert!(
        user.reset_sent_at.is_some(),
        "Expected reset sent timestamp to be present"
    );
    assert!(
        user.reset_token.is_some(),
        "Expected reset token to be present"
    );
}

#[tokio::test]
#[serial]
async fn can_verified() {
    configure_insta!();

    let boot = boot_test::<App>()
        .await
        .expect("Failed to boot test application");
    seed::<App>(&boot.app_context)
        .await
        .expect("Failed to seed database");

    let user = Model::find_by_pid(&boot.app_context.db, "11111111-1111-1111-1111-111111111111")
        .await
        .expect("Failed to find user by PID");

    assert!(
        user.email_verified_at.is_none(),
        "Expected email to be unverified"
    );

    let result = user
        .into_active_model()
        .verified(&boot.app_context.db)
        .await;

    assert!(result.is_ok(), "Failed to mark email as verified");

    let user = Model::find_by_pid(&boot.app_context.db, "11111111-1111-1111-1111-111111111111")
        .await
        .expect("Failed to find user by PID after verification");

    assert!(
        user.email_verified_at.is_some(),
        "Expected email to be verified"
    );
}

#[tokio::test]
#[serial]
async fn can_reset_password() {
    configure_insta!();

    let boot = boot_test::<App>()
        .await
        .expect("Failed to boot test application");
    seed::<App>(&boot.app_context)
        .await
        .expect("Failed to seed database");

    let user = Model::find_by_pid(&boot.app_context.db, "11111111-1111-1111-1111-111111111111")
        .await
        .expect("Failed to find user by PID");

    assert!(
        user.verify_password("12341234"),
        "Password verification failed for original password"
    );

    let result = user
        .clone()
        .into_active_model()
        .reset_password(&boot.app_context.db, "new-password")
        .await;

    assert!(result.is_ok(), "Failed to reset password");

    let user = Model::find_by_pid(&boot.app_context.db, "11111111-1111-1111-1111-111111111111")
        .await
        .expect("Failed to find user by PID after password reset");

    assert!(
        user.verify_password("new-password"),
        "Password verification failed for new password"
    );
}

#[tokio::test]
#[serial]
async fn magic_link() {
    let boot = boot_test::<App>().await.unwrap();
    seed::<App>(&boot.app_context).await.unwrap();

    let user = Model::find_by_pid(&boot.app_context.db, "11111111-1111-1111-1111-111111111111")
        .await
        .unwrap();

    assert!(
        user.magic_link_token.is_none(),
        "Magic link token should be initially unset"
    );
    assert!(
        user.magic_link_expiration.is_none(),
        "Magic link expiration should be initially unset"
    );

    let create_result = user
        .into_active_model()
        .create_magic_link(&boot.app_context.db)
        .await;

    assert!(
        create_result.is_ok(),
        "Failed to create magic link: {:?}",
        create_result.unwrap_err()
    );

    let updated_user =
        Model::find_by_pid(&boot.app_context.db, "11111111-1111-1111-1111-111111111111")
            .await
            .expect("Failed to refetch user after magic link creation");

    assert!(
        updated_user.magic_link_token.is_some(),
        "Magic link token should be set after creation"
    );

    let magic_link_token = updated_user.magic_link_token.unwrap();
    assert_eq!(
        magic_link_token.len(),
        users::MAGIC_LINK_LENGTH as usize,
        "Magic link token length does not match expected length"
    );

    assert!(
        updated_user.magic_link_expiration.is_some(),
        "Magic link expiration should be set after creation"
    );

    let now = Local::now();
    let should_expired_at = now + Duration::minutes(users::MAGIC_LINK_EXPIRATION_MIN.into());
    let actual_expiration = updated_user.magic_link_expiration.unwrap();

    assert!(
        actual_expiration >= now,
        "Magic link expiration should be in the future or now"
    );

    assert!(
        actual_expiration <= should_expired_at,
        "Magic link expiration exceeds expected maximum expiration time"
    );
}

/// Signs up an organisation and returns its owner and the org id.
async fn owner_of(ctx: &loco_rs::app::AppContext, org: &str, owner: &str) -> (Model, i64) {
    let (user, org) = organisations::Model::sign_up(
        &ctx.db,
        &SignupParams {
            organisation_name: org.to_string(),
            name: owner.to_string(),
            email: format!("{owner}@example.com"),
            password: "correct-horse".to_string(),
        },
    )
    .await
    .unwrap();
    (user, org.id)
}

#[tokio::test]
#[serial]
async fn an_unlock_counts_only_in_its_own_org() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, acme) = owner_of(ctx, "Acme", "alice").await;
    let (_, beta) = owner_of(ctx, "Beta", "bob").await;

    Model::unlock_secrets(&ctx.db, alice.id, "sid-1", acme)
        .await
        .unwrap();
    let alice = Model::find_by_id(&ctx.db, alice.id).await.unwrap();
    assert!(alice.secrets_unlocked(Some("sid-1"), acme));
    assert!(!alice.secrets_unlocked(Some("sid-1"), beta));
    assert!(!alice.secrets_unlocked(Some("sid-2"), acme));

    Model::end_unlock(&ctx.db, alice.id).await.unwrap();
    let alice = Model::find_by_id(&ctx.db, alice.id).await.unwrap();
    assert!(!alice.secrets_unlocked(Some("sid-1"), acme));
    assert_eq!(alice.secrets_unlocked_org_id, None);
}

#[tokio::test]
#[serial]
async fn a_session_bump_clears_the_unlock_org() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, acme) = owner_of(ctx, "Acme", "alice").await;

    Model::unlock_secrets(&ctx.db, alice.id, "sid-1", acme)
        .await
        .unwrap();
    Model::increment_session_version(&ctx.db, alice.id)
        .await
        .unwrap();
    let alice = Model::find_by_id(&ctx.db, alice.id).await.unwrap();
    assert_eq!(alice.secrets_unlocked_org_id, None);
    assert_eq!(alice.secrets_unlocked_sid, None);
    assert!(!alice.secrets_unlocked(Some("sid-1"), acme));
}

#[tokio::test]
#[serial]
async fn end_unlock_in_clears_only_that_org() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    assert!(
        Model::unlock_secrets(&ctx.db, orgs.alice, "sid-1", orgs.acme)
            .await
            .unwrap()
    );

    Model::end_unlock_in(&ctx.db, orgs.alice, orgs.beta)
        .await
        .unwrap();
    let alice = Model::find_by_id(&ctx.db, orgs.alice).await.unwrap();
    assert!(
        alice.secrets_unlocked(Some("sid-1"), orgs.acme),
        "another org"
    );

    Model::end_unlock_in(&ctx.db, orgs.alice, orgs.acme)
        .await
        .unwrap();
    let alice = Model::find_by_id(&ctx.db, orgs.alice).await.unwrap();
    assert!(!alice.secrets_unlocked(Some("sid-1"), orgs.acme));
    assert_eq!(alice.secrets_unlocked_org_id, None);
}

#[tokio::test]
#[serial]
async fn end_unlock_for_session_clears_only_that_sign_in() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    assert!(
        Model::unlock_secrets(&ctx.db, orgs.alice, "sid-1", orgs.acme)
            .await
            .unwrap()
    );

    Model::end_unlock_for_session(&ctx.db, orgs.alice, "sid-2")
        .await
        .unwrap();
    let alice = Model::find_by_id(&ctx.db, orgs.alice).await.unwrap();
    assert!(
        alice.secrets_unlocked(Some("sid-1"), orgs.acme),
        "another sign-in"
    );

    Model::end_unlock_for_session(&ctx.db, orgs.alice, "sid-1")
        .await
        .unwrap();
    let alice = Model::find_by_id(&ctx.db, orgs.alice).await.unwrap();
    assert!(!alice.secrets_unlocked(Some("sid-1"), orgs.acme));
    assert_eq!(alice.secrets_unlocked_sid, None);
}

#[tokio::test]
#[serial]
async fn a_password_reset_revokes_tokens_in_every_org() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    let in_acme = personal_token(ctx, orgs.acme, orgs.alice).await;
    let in_beta = oauth_tokens(ctx, orgs.beta, orgs.alice).await;
    let bobs = personal_token(ctx, orgs.beta, orgs.bob).await;

    let alice = Model::find_by_id(&ctx.db, orgs.alice).await.unwrap();
    let (alice, revoked) = alice
        .reset_password_everywhere(ctx, "another-horse")
        .await
        .unwrap();
    assert_eq!(revoked, 2);
    assert!(!usable(ctx, &in_acme).await);
    assert!(!usable(ctx, &in_beta.access_token).await);
    assert!(usable(ctx, &bobs).await, "someone else's token");

    let (_, revoked) = alice
        .reset_password_everywhere(ctx, "a-third-horse")
        .await
        .unwrap();
    assert_eq!(revoked, 0, "none left to revoke");
}

/// A password reset holds the user row on another connection while a
/// refresh runs. The refresh waits, then sees its token revoked.
#[tokio::test]
#[serial]
async fn a_refresh_waits_for_a_password_reset_in_progress() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    let claude = claude_client(ctx).await;
    let code = code_for(ctx, &claude, orgs.acme, orgs.alice).await;
    let issued = exchange(&ctx.db, &claude, &code).await.unwrap();
    let grant = access_tokens::Entity::find()
        .filter(access_tokens::Column::RefreshHash.eq(access_tokens::hash(&issued.refresh_token)))
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    let other = second_connection(ctx).await;

    let reset = async {
        let txn = other.begin().await.unwrap();
        users::Entity::find_by_id(orgs.alice)
            .lock_exclusive()
            .one(&txn)
            .await
            .unwrap();
        tokio::time::sleep(HOLD).await;
        access_tokens::Model::revoke_all_for_user(&txn, orgs.alice)
            .await
            .unwrap();
        txn.commit().await.unwrap();
    };
    let refresh = async {
        tokio::time::sleep(HOLD / 5).await;
        access_tokens::Model::refresh(&ctx.db, &issued.refresh_token, &claude, RESOURCE)
            .await
            .unwrap()
    };
    let ((), refreshed) = tokio::join!(reset, refresh);
    assert!(refreshed.is_none(), "refused after the reset");
    let live = access_tokens::Entity::find()
        .filter(access_tokens::Column::GrantId.eq(grant.grant_id))
        .filter(access_tokens::Column::RevokedAt.is_null())
        .count(&ctx.db)
        .await
        .unwrap();
    assert_eq!(live, 0);
}

/// A password reset of Alice on its own connection: the user row update,
/// then revoking her tokens, then commit.
async fn reset_alice(db: &sea_orm::DatabaseConnection, alice: i64) {
    let txn = db.begin().await.unwrap();
    Model::increment_session_version(&txn, alice).await.unwrap();
    access_tokens::Model::revoke_all_for_user(&txn, alice)
        .await
        .unwrap();
    txn.commit().await.unwrap();
}

/// Holds a row `FOR UPDATE` for [`HOLD`] on its own connection, so a token
/// insert that points at it (a key share for the foreign key) waits, with
/// its transaction still open.
async fn hold_row<E>(db: &sea_orm::DatabaseConnection, id: i64)
where
    E: EntityTrait,
    <E::PrimaryKey as sea_orm::PrimaryKeyTrait>::ValueType: From<i64>,
{
    let txn = db.begin().await.unwrap();
    E::find_by_id(id).lock_exclusive().one(&txn).await.unwrap();
    tokio::time::sleep(HOLD).await;
    txn.commit().await.unwrap();
}

async fn after(fraction: u32) {
    tokio::time::sleep(HOLD * fraction / 5).await;
}

// A token minted while a reset runs must not survive it. The mint is held
// mid-transaction (its insert waits on `hold_row`) while the reset runs.
// Without the mint's `FOR SHARE` on the user row, the reset would revoke
// before the new token was committed, and miss it.

#[tokio::test]
#[serial]
async fn a_code_exchange_racing_a_reset_ends_revoked() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    let claude = claude_client(ctx).await;
    let code = code_for(ctx, &claude, orgs.acme, orgs.alice).await;
    let blocker = second_connection(ctx).await;
    let resetter = second_connection(ctx).await;

    let (_, issued, ()) = tokio::join!(
        hold_row::<oauth_clients::Entity>(&blocker, claude.id),
        async {
            after(1).await;
            exchange(&ctx.db, &claude, &code).await
        },
        async {
            after(2).await;
            reset_alice(&resetter, orgs.alice).await;
        }
    );
    if let Some(issued) = issued {
        assert!(
            !usable(ctx, &issued.access_token).await,
            "revoked by the reset"
        );
    }
    let live = access_tokens::Entity::find()
        .filter(access_tokens::Column::UserId.eq(orgs.alice))
        .filter(access_tokens::Column::RevokedAt.is_null())
        .count(&ctx.db)
        .await
        .unwrap();
    assert_eq!(live, 0);
}

#[tokio::test]
#[serial]
async fn a_personal_token_racing_a_reset_ends_revoked() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let orgs = alice_in_two_orgs(ctx).await;
    let blocker = second_connection(ctx).await;
    let resetter = second_connection(ctx).await;
    let params = PersonalParams {
        name: "Laptop".to_string(),
    };

    let (_, minted, ()) = tokio::join!(
        hold_row::<organisations::Entity>(&blocker, orgs.acme),
        async {
            after(1).await;
            access_tokens::Model::create_personal(&ctx.db, orgs.acme, orgs.alice, &params, RESOURCE)
                .await
        },
        async {
            after(2).await;
            reset_alice(&resetter, orgs.alice).await;
        }
    );
    let (_, token) = minted.unwrap();
    assert!(!usable(ctx, &token).await, "revoked by the reset");
}
