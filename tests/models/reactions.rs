use collab::{
    app::App,
    models::{
        conversations, messages,
        organisations::{self, SignupParams},
        reactions, users,
    },
};
use loco_rs::{app::AppContext, testing::prelude::*};
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait, PaginatorTrait};
use serial_test::serial;

/// An organisation with its owner and one message in General.
async fn org_with_message(
    ctx: &AppContext,
    name: &str,
    owner: &str,
) -> (users::Model, messages::Model) {
    let (user, org) = organisations::Model::sign_up(
        &ctx.db,
        &SignupParams {
            organisation_name: name.to_string(),
            name: owner.to_string(),
            email: format!("{owner}@example.com"),
            password: "correct-horse".to_string(),
        },
    )
    .await
    .unwrap();
    let general = conversations::Model::find_general(&ctx.db, org.id)
        .await
        .unwrap();
    let message = messages::Model::create(
        &ctx.db,
        &general,
        user.id,
        &messages::MessageParams {
            body: format!("{name} news"),
        },
    )
    .await
    .unwrap();
    (user, message)
}

async fn count(ctx: &AppContext) -> u64 {
    reactions::Entity::find().count(&ctx.db).await.unwrap()
}

#[tokio::test]
#[serial]
async fn setting_on_or_off_twice_is_the_same_as_once() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, message) = org_with_message(ctx, "Acme", "alice").await;

    // The row is already there, as if another tab added it first: no error.
    reactions::ActiveModel {
        emoji: ActiveValue::Set("👍".to_string()),
        organisation_id: ActiveValue::Set(message.organisation_id),
        message_id: ActiveValue::Set(message.id),
        user_id: ActiveValue::Set(alice.id),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .unwrap();
    for _ in 0..2 {
        reactions::Model::set(&ctx.db, &message, alice.id, "👍", true)
            .await
            .unwrap();
        assert_eq!(count(ctx).await, 1);
    }
    for _ in 0..2 {
        reactions::Model::set(&ctx.db, &message, alice.id, "👍", false)
            .await
            .unwrap();
        assert_eq!(count(ctx).await, 0);
    }
    assert!(
        reactions::Model::set(&ctx.db, &message, alice.id, "❤", true)
            .await
            .is_err()
    );
    assert_eq!(count(ctx).await, 0);
}

#[tokio::test]
#[serial]
async fn for_messages_groups_by_message_and_stays_in_the_tenant() {
    let boot = boot_test::<App>().await.unwrap();
    let ctx = &boot.app_context;
    let (alice, acme_message) = org_with_message(ctx, "Acme", "alice").await;
    let (gina, globex_message) = org_with_message(ctx, "Globex", "gina").await;
    for emoji in ["✅", "👍"] {
        reactions::Model::set(&ctx.db, &acme_message, alice.id, emoji, true)
            .await
            .unwrap();
    }
    reactions::Model::set(&ctx.db, &globex_message, gina.id, "🎉", true)
        .await
        .unwrap();

    let ids = [acme_message.id, globex_message.id];
    let acme = reactions::Model::for_messages(&ctx.db, acme_message.organisation_id, &ids)
        .await
        .unwrap();
    assert_eq!(acme.len(), 1, "Globex's message is not Acme's");
    let emoji: Vec<&str> = acme[&acme_message.id]
        .iter()
        .map(|r| r.emoji.as_str())
        .collect();
    assert_eq!(emoji, vec!["✅", "👍"], "oldest first");
    let globex = reactions::Model::for_messages(&ctx.db, globex_message.organisation_id, &ids)
        .await
        .unwrap();
    assert_eq!(globex[&globex_message.id].len(), 1);
    assert!(!globex.contains_key(&acme_message.id));
    assert!(
        reactions::Model::for_messages(&ctx.db, acme_message.organisation_id, &[])
            .await
            .unwrap()
            .is_empty()
    );
}
