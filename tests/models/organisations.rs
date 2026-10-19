use collab::{
    app::App,
    models::{
        _entities::organisations,
        organisations::{NewOrgParams, SignupParams},
    },
};
use loco_rs::testing::prelude::*;
use sea_orm::EntityTrait;
use serial_test::serial;
use validator::Validate;

macro_rules! configure_insta {
    ($($expr:expr),*) => {
        let mut settings = insta::Settings::clone_current();
        settings.set_prepend_module_to_snapshot(false);
        let _guard = settings.bind_to_scope();
    };
}

/// The `Organisations` entity matches the table its migration created.
///
/// Selecting every column is the cheapest assertion that says something true:
/// it fails if the migration and the entity disagree — a renamed column, a
/// type that does not round-trip, a migration that never ran — which is the
/// most common way a generated model breaks. Extend it as the model grows.
#[tokio::test]
#[serial]
async fn can_query_organisations() {
    configure_insta!();

    let boot = boot_test::<App>().await.unwrap();
    seed::<App>(&boot.app_context).await.unwrap();

    // The query is the assertion. Bind the result and compare it once you have
    // seed data to compare against, e.g.:
    //
    // let items = organisations::Entity::find().all(&boot.app_context.db).await.unwrap();
    // assert_debug_snapshot!(items);
    organisations::Entity::find()
        .all(&boot.app_context.db)
        .await
        .expect("`organisations` should be queryable — entity and migration must agree");
}

fn new_org(name: &str, username: &str) -> NewOrgParams {
    NewOrgParams {
        organisation_name: name.to_string(),
        name: username.to_string(),
    }
}

fn signup(name: &str) -> SignupParams {
    SignupParams {
        organisation_name: name.to_string(),
        name: "alice".to_string(),
        email: "alice@example.com".to_string(),
        password: "correct-horse".to_string(),
    }
}

/// The message on `organisation_name`, if validation refused it.
fn name_error(result: Result<(), validator::ValidationErrors>) -> Option<String> {
    result.err().and_then(|e| {
        e.field_errors()
            .get("organisation_name")
            .and_then(|list| list.first())
            .and_then(|err| err.message.as_ref().map(ToString::to_string))
    })
}

#[test]
fn a_spaces_only_name_is_refused() {
    for name in ["   ", " a ", ""] {
        assert_eq!(
            name_error(new_org(name, "alice").validate()).as_deref(),
            Some("Use 2 to 80 characters."),
            "create: {name:?}"
        );
        assert_eq!(
            name_error(signup(name).validate()).as_deref(),
            Some("Use 2 to 80 characters."),
            "signup: {name:?}"
        );
    }
    // Spaces around a real name don't count against it.
    let long = format!("  {}  ", "a".repeat(80));
    assert_eq!(name_error(new_org(&long, "alice").validate()), None);
    assert_eq!(name_error(signup(" ab ").validate()), None);
    assert!(name_error(signup(&"a".repeat(81)).validate()).is_some());
}
