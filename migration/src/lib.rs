#![allow(elided_lifetimes_in_paths)]
#![allow(clippy::wildcard_imports)]
pub use sea_orm_migration::prelude::*;
mod m20220101_000001_users;

mod m20261007_143211_organisations;
mod m20261007_143345_memberships;
mod m20261007_143455_projects;
mod m20261007_143605_tasks;
mod m20261007_143712_task_notes;
mod m20261007_143819_meetings;
mod m20261007_143925_meeting_attendees;
mod m20261007_144032_conversations;
mod m20261007_144139_conversation_members;
mod m20261007_144248_messages;
mod m20261007_144359_add_super_admin_to_users;
mod m20261008_094956_add_last_read_at_to_conversation_members;
mod m20261008_134056_task_assignees;
mod m20261008_134252_remove_owner_id_from_tasks;
mod m20261008_155033_notifications;
pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20220101_000001_users::Migration),
            Box::new(m20261007_143211_organisations::Migration),
            Box::new(m20261007_143345_memberships::Migration),
            Box::new(m20261007_143455_projects::Migration),
            Box::new(m20261007_143605_tasks::Migration),
            Box::new(m20261007_143712_task_notes::Migration),
            Box::new(m20261007_143819_meetings::Migration),
            Box::new(m20261007_143925_meeting_attendees::Migration),
            Box::new(m20261007_144032_conversations::Migration),
            Box::new(m20261007_144139_conversation_members::Migration),
            Box::new(m20261007_144248_messages::Migration),
            Box::new(m20261007_144359_add_super_admin_to_users::Migration),
            Box::new(m20261008_094956_add_last_read_at_to_conversation_members::Migration),
            Box::new(m20261008_134056_task_assignees::Migration),
            Box::new(m20261008_134252_remove_owner_id_from_tasks::Migration),
            Box::new(m20261008_155033_notifications::Migration),
            // inject-above (do not remove this comment)
        ]
    }
}
