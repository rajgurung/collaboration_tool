use collab::{app::App, models::_entities::folders};
use loco_rs::testing::prelude::*;
use sea_orm::EntityTrait;
use serial_test::serial;

macro_rules! configure_insta {
    ($($expr:expr),*) => {
        let mut settings = insta::Settings::clone_current();
        settings.set_prepend_module_to_snapshot(false);
        let _guard = settings.bind_to_scope();
    };
}

/// The `Folders` entity matches the table its migration created.
///
/// Selecting every column is the cheapest assertion that says something true:
/// it fails if the migration and the entity disagree — a renamed column, a
/// type that does not round-trip, a migration that never ran — which is the
/// most common way a generated model breaks. Extend it as the model grows.
#[tokio::test]
#[serial]
async fn can_query_folders() {
    configure_insta!();

    let boot = boot_test::<App>().await.unwrap();
    seed::<App>(&boot.app_context).await.unwrap();

    // The query is the assertion. Bind the result and compare it once you have
    // seed data to compare against, e.g.:
    //
    // let items = folders::Entity::find().all(&boot.app_context.db).await.unwrap();
    // assert_debug_snapshot!(items);
    folders::Entity::find()
        .all(&boot.app_context.db)
        .await
        .expect("`folders` should be queryable — entity and migration must agree");
}

mod shared {
    use collab::{
        app::App,
        models::{
            folders::{self, FolderParams},
            login_accesses, logins, organisations,
            organisations::SignupParams,
        },
    };
    use loco_rs::{app::AppContext, model::ModelError, testing::prelude::*};
    use sea_orm::{
        ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    };
    use serial_test::serial;

    async fn org(ctx: &AppContext, name: &str) -> (i64, i64) {
        let slug = name.to_lowercase();
        let (owner, org) = organisations::Model::sign_up(
            &ctx.db,
            &SignupParams {
                organisation_name: name.to_string(),
                name: format!("ana{slug}"),
                email: format!("ana@{slug}.example.com"),
                password: "correct-horse".to_string(),
            },
        )
        .await
        .unwrap();
        (org.id, owner.id)
    }

    async fn names(ctx: &AppContext, org_id: i64) -> Vec<(String, Option<String>)> {
        let mut all: Vec<_> = folders::Entity::find()
            .filter(folders::Column::OrganisationId.eq(org_id))
            .all(&ctx.db)
            .await
            .unwrap()
            .into_iter()
            .map(|f| (f.name, f.kind))
            .collect();
        all.sort();
        all
    }

    fn params(name: &str, note: &str) -> FolderParams {
        FolderParams {
            name: name.to_string(),
            note: note.to_string(),
        }
    }

    #[tokio::test]
    #[serial]
    async fn new_organisations_get_company_logins_and_shared() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let (org_id, _) = org(ctx, "Juniper").await;
        assert_eq!(
            names(ctx, org_id).await,
            vec![
                ("Company".to_string(), None),
                ("Logins".to_string(), None),
                ("Shared".to_string(), Some("shared".to_string())),
            ]
        );
        let shared = folders::Model::shared(&ctx.db, org_id).await.unwrap();
        assert!(shared.is_shared() && shared.is_built_in());
        assert!(shared.open_to(false));
        assert_eq!(shared.note.as_deref(), Some(folders::SHARED_NOTE));
    }

    #[tokio::test]
    #[serial]
    async fn a_missing_shared_folder_is_made_once_even_when_asked_twice_at_once() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let (org_id, _) = org(ctx, "Juniper").await;
        folders::Entity::delete_many()
            .filter(folders::Column::Kind.eq("shared"))
            .exec(&ctx.db)
            .await
            .unwrap();
        let (a, b) = tokio::join!(
            folders::Model::shared(&ctx.db, org_id),
            folders::Model::shared(&ctx.db, org_id),
        );
        assert_eq!(a.unwrap().id, b.unwrap().id);
        let again = folders::Model::shared(&ctx.db, org_id).await.unwrap();
        assert_eq!(again.name, "Shared");
        let count = names(ctx, org_id)
            .await
            .into_iter()
            .filter(|(_, kind)| kind.is_some())
            .count();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    #[serial]
    async fn the_backfill_renames_old_shared_folders_and_adds_one_per_org() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let (juniper, _) = org(ctx, "Juniper").await;
        let (birch, _) = org(ctx, "Birch").await;
        // As before the migration: no built-in Shared, and ordinary folders
        // holding the name and the first fallback.
        folders::Entity::delete_many()
            .filter(folders::Column::Kind.eq("shared"))
            .exec(&ctx.db)
            .await
            .unwrap();
        for (org_id, name) in [
            (juniper, "shared"),
            (juniper, "Shared (old)"),
            (birch, "Shared"),
        ] {
            folders::ActiveModel {
                name: ActiveValue::Set(name.to_string()),
                organisation_id: ActiveValue::Set(org_id),
                ..Default::default()
            }
            .insert(&ctx.db)
            .await
            .unwrap();
        }
        let sql = migration::m20261027_090000_add_kind_to_folders::backfill_sql();
        ctx.db.execute_unprepared(&sql).await.unwrap();
        // Running it again changes nothing.
        ctx.db.execute_unprepared(&sql).await.unwrap();

        let shared = Some("shared".to_string());
        assert_eq!(
            names(ctx, juniper).await,
            vec![
                ("Company".to_string(), None),
                ("Logins".to_string(), None),
                ("Shared".to_string(), shared.clone()),
                ("Shared (old 2)".to_string(), None),
                ("Shared (old)".to_string(), None),
            ]
        );
        assert_eq!(
            names(ctx, birch).await,
            vec![
                ("Company".to_string(), None),
                ("Logins".to_string(), None),
                ("Shared".to_string(), shared),
                ("Shared (old)".to_string(), None),
            ]
        );
    }

    #[tokio::test]
    #[serial]
    async fn built_in_folders_keep_their_names_and_stay() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let (org_id, _) = org(ctx, "Juniper").await;
        let all = folders::Model::list_for_org(&ctx.db, org_id).await.unwrap();
        let find = |name: &str| {
            all.iter()
                .find(|s| s.folder.name == name)
                .unwrap()
                .folder
                .clone()
        };
        for folder in [find("Shared"), find("Logins")] {
            let err = folder
                .clone()
                .edit(&ctx.db, &params("Papers", ""))
                .await
                .unwrap_err();
            assert!(format!("{err:?}").contains("Built-in folders keep their name."));
            let edited = folder
                .clone()
                .edit(&ctx.db, &params(&folder.name, "New note"))
                .await
                .unwrap();
            assert_eq!(edited.note.as_deref(), Some("New note"));
            match folder.clone().delete_if_empty(&ctx.db).await {
                Err(ModelError::Message(m)) => assert_eq!(
                    m,
                    format!(
                        "The {} folder is built in, so it can't be deleted.",
                        folder.name
                    )
                ),
                other => panic!("{other:?}"),
            }
        }
        // Company can't take the name either.
        let err = find("Company")
            .edit(&ctx.db, &params("shared", ""))
            .await
            .unwrap_err();
        assert!(format!("{err:?}").contains("already a folder with that name."));
    }

    #[tokio::test]
    #[serial]
    async fn shared_with_you_lists_only_this_orgs_logins_naming_the_person() {
        let boot = boot_test::<App>().await.unwrap();
        let ctx = &boot.app_context;
        let (juniper, ana) = org(ctx, "Juniper").await;
        let (birch, bea) = org(ctx, "Birch").await;
        let folder = |org_id| async move {
            folders::Model::list_for_org(&ctx.db, org_id)
                .await
                .unwrap()
                .into_iter()
                .find(|s| s.folder.is_logins())
                .unwrap()
                .folder
        };
        let mut ids = Vec::new();
        for (org_id, folder_id, title) in [
            (juniper, folder(juniper).await.id, "Courier"),
            (juniper, folder(juniper).await.id, "Bank"),
            (birch, folder(birch).await.id, "Birch bank"),
        ] {
            let login = logins::ActiveModel {
                folder_id: ActiveValue::Set(folder_id),
                organisation_id: ActiveValue::Set(org_id),
                title: ActiveValue::Set(title.to_string()),
                username: ActiveValue::Set(String::new()),
                url: ActiveValue::Set(String::new()),
                notes: ActiveValue::Set(String::new()),
                password_ct: ActiveValue::Set(vec![0]),
                password_nonce: ActiveValue::Set(vec![0]),
                dek_version: ActiveValue::Set(1),
                ..Default::default()
            }
            .insert(&ctx.db)
            .await
            .unwrap();
            ids.push(login.id);
        }
        // Ana is named on Courier here, and (oddly) on Birch's login.
        for login_id in [ids[0], ids[2]] {
            login_accesses::ActiveModel {
                login_id: ActiveValue::Set(login_id),
                user_id: ActiveValue::Set(ana),
                ..Default::default()
            }
            .insert(&ctx.db)
            .await
            .unwrap();
        }
        let titles = |org_id, user_id| async move {
            login_accesses::Model::logins_for(&ctx.db, org_id, user_id)
                .await
                .unwrap()
                .into_iter()
                .map(|l| l.title)
                .collect::<Vec<_>>()
        };
        assert_eq!(titles(juniper, ana).await, vec!["Courier"]);
        assert!(titles(juniper, bea).await.is_empty());
    }
}
