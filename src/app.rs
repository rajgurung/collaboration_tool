use async_trait::async_trait;
use loco_rs::{
    app::{AppContext, Hooks, Initializer},
    bgworker::{BackgroundWorker, Queue},
    boot::{create_app, BootResult, StartMode},
    config::Config,
    controller::AppRoutes,
    db::{self, truncate_table},
    environment::Environment,
    storage::{drivers, Storage},
    task::Tasks,
    Error, Result,
};
use migration::Migrator;
use std::{path::Path, sync::Arc};

#[allow(unused_imports)]
use crate::{
    controllers,
    data::settings::Settings,
    initializers,
    models::_entities::{
        access_tokens, auth_attempts, conversation_members, conversations, documents, folders,
        meeting_attendees, meetings, memberships, messages, oauth_clients, oauth_codes,
        organisations, projects, task_notes, tasks as task_items, users,
    },
    tasks,
    workers::downloader::DownloadWorker,
};

/// Where uploaded documents live: R2 in production, memory in tests, and
/// `tmp/storage` on disk in development.
fn document_storage(ctx: &AppContext) -> Result<Storage> {
    let driver = match ctx.environment {
        Environment::Production => {
            let r2 = Settings::from_context(ctx)?
                .r2
                .ok_or_else(|| Error::string("settings.r2 is required in production"))?;
            r2_driver(
                &r2.bucket,
                &r2.endpoint,
                r2.access_key_id,
                r2.secret_access_key,
            )?
        }
        Environment::Test => drivers::mem::new(),
        _ => {
            std::fs::create_dir_all(DEV_STORAGE)?;
            drivers::local::new_with_prefix(DEV_STORAGE)?
        }
    };
    Ok(Storage::single(driver))
}

/// The R2 driver. Loco builds OpenDAL without an HTTP client, so install
/// OpenDAL's default one first; it is a no-op after the first call.
fn r2_driver(
    bucket: &str,
    endpoint: &str,
    key_id: String,
    secret_key: String,
) -> Result<Box<dyn drivers::StoreDriver>> {
    opendal::install_default();
    Ok(drivers::aws::with_credentials_and_endpoint(
        bucket,
        "auto",
        endpoint,
        drivers::aws::Credential {
            key_id,
            secret_key,
            token: None,
        },
    )?)
}

const DEV_STORAGE: &str = "tmp/storage";

pub struct App;
#[async_trait]
impl Hooks for App {
    fn app_name() -> &'static str {
        env!("CARGO_CRATE_NAME")
    }

    fn app_version() -> String {
        format!(
            "{} ({})",
            env!("CARGO_PKG_VERSION"),
            option_env!("BUILD_SHA")
                .or(option_env!("GITHUB_SHA"))
                .unwrap_or("dev")
        )
    }

    async fn boot(
        mode: StartMode,
        environment: &Environment,
        config: Config,
    ) -> Result<BootResult> {
        create_app::<Self, Migrator>(mode, environment, config).await
    }

    async fn after_context(ctx: AppContext) -> Result<AppContext> {
        ctx.shared_store
            .insert(crate::data::chat_hub::ChatHub::new());
        ctx.shared_store
            .insert(crate::data::notify_hub::NotifyHub::new());
        let storage = document_storage(&ctx)?;
        Ok(ctx.into_builder().storage(Arc::new(storage)).build())
    }

    async fn initializers(_ctx: &AppContext) -> Result<Vec<Box<dyn Initializer>>> {
        Ok(vec![
            Box::new(initializers::view_engine::ViewEngineInitializer),
            Box::new(initializers::origin_check::OriginCheckInitializer),
            Box::new(initializers::mention_emails::MentionEmailsInitializer),
        ])
    }

    fn routes(ctx: &AppContext) -> AppRoutes {
        AppRoutes::with_default_routes() // controller routes below
            .add_route(controllers::documents::routes())
            .add_route(controllers::claude::routes())
            .add_route(controllers::oauth::routes())
            .add_route(controllers::mcp::routes(ctx))
            .add_route(controllers::saved_views::routes())
            .add_route(controllers::guide::routes())
            .add_route(controllers::notifications::routes())
            .add_route(controllers::more::routes())
            .add_route(controllers::chat_ws::routes())
            .add_route(controllers::admin::routes())
            .add_route(controllers::members::routes())
            .add_route(controllers::chat::routes())
            .add_route(controllers::meetings::routes())
            .add_route(controllers::tasks::routes())
            .add_route(controllers::roadmap::routes())
            .add_route(controllers::dashboard::routes())
            .add_route(controllers::join::routes())
            .add_route(controllers::signup::routes())
            .add_route(controllers::auth::routes())
            .add_route(controllers::page::routes())
    }
    async fn connect_workers(ctx: &AppContext, queue: &Queue) -> Result<()> {
        queue
            .register(crate::workers::resend_email::Worker::build(ctx))
            .await?;
        queue.register(DownloadWorker::build(ctx)).await?;
        Ok(())
    }

    #[allow(unused_variables)]
    fn register_tasks(tasks: &mut Tasks) {
        tasks.register(tasks::super_admin::SuperAdmin);
        // tasks-inject (do not remove)
        tasks.register(tasks::user_create::UserCreate);
        tasks.register(tasks::user_delete::UserDelete);
    }
    async fn truncate(ctx: &AppContext) -> Result<()> {
        // Children before parents so foreign keys never block the delete.
        truncate_table(&ctx.db, documents::Entity).await?;
        truncate_table(&ctx.db, folders::Entity).await?;
        truncate_table(&ctx.db, auth_attempts::Entity).await?;
        truncate_table(&ctx.db, access_tokens::Entity).await?;
        truncate_table(&ctx.db, oauth_codes::Entity).await?;
        truncate_table(&ctx.db, oauth_clients::Entity).await?;
        truncate_table(&ctx.db, messages::Entity).await?;
        truncate_table(&ctx.db, conversation_members::Entity).await?;
        truncate_table(&ctx.db, conversations::Entity).await?;
        truncate_table(&ctx.db, meeting_attendees::Entity).await?;
        truncate_table(&ctx.db, meetings::Entity).await?;
        truncate_table(&ctx.db, task_notes::Entity).await?;
        truncate_table(&ctx.db, task_items::Entity).await?;
        truncate_table(&ctx.db, projects::Entity).await?;
        truncate_table(&ctx.db, memberships::Entity).await?;
        truncate_table(&ctx.db, organisations::Entity).await?;
        truncate_table(&ctx.db, users::Entity).await?;
        Ok(())
    }
    async fn seed(ctx: &AppContext, base: &Path) -> Result<()> {
        db::seed::<users::ActiveModel>(&ctx.db, &base.join("users.yaml").display().to_string())
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use std::path::Path;

    /// Production's R2 path never runs in tests (they use memory storage), so
    /// check it can at least make an HTTP request: against a closed port that
    /// is a connection error, not a missing transport.
    #[tokio::test]
    async fn r2_driver_has_an_http_client() {
        let driver = super::r2_driver(
            "bucket",
            "http://127.0.0.1:9",
            "key".to_string(),
            "secret".to_string(),
        )
        .expect("driver builds");
        let err = driver
            .upload(Path::new("probe.txt"), &Bytes::from_static(b"x"))
            .await
            .expect_err("nothing listens on port 9");
        assert!(
            !err.to_string().contains("HTTP transport is not installed"),
            "{err}"
        );
    }

    /// Round trip against the real bucket. Opt-in, needs the R2_* variables:
    /// `cargo test --lib r2_round_trip -- --ignored`.
    #[tokio::test]
    #[ignore = "talks to the real R2 bucket"]
    async fn r2_round_trip() {
        let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"));
        let driver = super::r2_driver(
            &var("R2_BUCKET"),
            &var("R2_ENDPOINT"),
            var("R2_ACCESS_KEY_ID"),
            var("R2_SECRET_ACCESS_KEY"),
        )
        .expect("driver builds");
        let path = Path::new("_probe/round-trip.txt");
        driver
            .upload(path, &Bytes::from_static(b"probe"))
            .await
            .expect("upload");
        let body = driver
            .get(path)
            .await
            .expect("get")
            .bytes()
            .await
            .expect("read");
        assert_eq!(&body[..], b"probe");
        driver.delete(path).await.expect("delete");
    }
}
