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
mod m20261008_162805_remove_progress_from_projects;
mod m20261008_165224_allow_tasks_without_project;
mod m20261008_171153_add_guide_to_memberships;
mod m20261008_173439_add_emailed_at_to_notifications;
mod m20261008_175915_saved_views;
mod m20261008_181908_add_timezone_to_organisations;
mod m20261008_213319_claude_connector;
mod m20261012_175013_add_session_version_to_users;
mod m20261012_180045_auth_attempts;
mod m20261012_181452_add_removed_to_memberships;
mod m20261012_182629_folders;
mod m20261012_182750_documents;
mod m20261012_203014_add_secrets_unlock_to_users;
mod m20261012_203023_org_keys;
mod m20261012_203253_logins;
mod m20261012_203441_login_accesses;
mod m20261012_203616_login_events;
mod m20261012_203901_logins_starter_folder;
mod m20261012_204623_add_done_at_to_tasks;
mod m20261013_140537_reactions;
mod m20261013_145508_add_parent_id_to_messages;
mod m20261015_142719_note_reactions;
mod m20261019_124040_memberships_one_per_org;
mod m20261019_124111_add_secrets_unlocked_org_id_to_users;
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
            Box::new(m20261008_162805_remove_progress_from_projects::Migration),
            Box::new(m20261008_165224_allow_tasks_without_project::Migration),
            Box::new(m20261008_171153_add_guide_to_memberships::Migration),
            Box::new(m20261008_173439_add_emailed_at_to_notifications::Migration),
            Box::new(m20261008_175915_saved_views::Migration),
            Box::new(m20261008_181908_add_timezone_to_organisations::Migration),
            Box::new(m20261008_213319_claude_connector::Migration),
            Box::new(m20261012_175013_add_session_version_to_users::Migration),
            Box::new(m20261012_180045_auth_attempts::Migration),
            Box::new(m20261012_181452_add_removed_to_memberships::Migration),
            Box::new(m20261012_182629_folders::Migration),
            Box::new(m20261012_182750_documents::Migration),
            Box::new(m20261012_203014_add_secrets_unlock_to_users::Migration),
            Box::new(m20261012_203023_org_keys::Migration),
            Box::new(m20261012_203253_logins::Migration),
            Box::new(m20261012_203441_login_accesses::Migration),
            Box::new(m20261012_203616_login_events::Migration),
            Box::new(m20261012_203901_logins_starter_folder::Migration),
            Box::new(m20261012_204623_add_done_at_to_tasks::Migration),
            Box::new(m20261013_140537_reactions::Migration),
            Box::new(m20261013_145508_add_parent_id_to_messages::Migration),
            Box::new(m20261015_142719_note_reactions::Migration),
            Box::new(m20261019_124040_memberships_one_per_org::Migration),
            Box::new(m20261019_124111_add_secrets_unlocked_org_id_to_users::Migration),
            // inject-above (do not remove this comment)
        ]
    }
}
