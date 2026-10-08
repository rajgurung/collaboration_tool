use async_trait::async_trait;
use loco_rs::{
    app::{AppContext, Hooks, Initializer},
    bgworker::{BackgroundWorker, Queue},
    boot::{create_app, BootResult, StartMode},
    config::Config,
    controller::AppRoutes,
    db::{self, truncate_table},
    environment::Environment,
    task::Tasks,
    Result,
};
use migration::Migrator;
use std::path::Path;

#[allow(unused_imports)]
use crate::{
    controllers, initializers,
    models::_entities::{
        conversation_members, conversations, meeting_attendees, meetings, memberships, messages,
        organisations, projects, task_notes, tasks as task_items, users,
    },
    tasks,
    workers::downloader::DownloadWorker,
};

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
        Ok(ctx)
    }

    async fn initializers(_ctx: &AppContext) -> Result<Vec<Box<dyn Initializer>>> {
        Ok(vec![
            Box::new(initializers::view_engine::ViewEngineInitializer),
            Box::new(initializers::origin_check::OriginCheckInitializer),
            Box::new(initializers::mention_emails::MentionEmailsInitializer),
        ])
    }

    fn routes(_ctx: &AppContext) -> AppRoutes {
        AppRoutes::with_default_routes() // controller routes below
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
