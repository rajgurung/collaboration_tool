mod users;

mod organisations;

mod memberships;

mod projects;

mod tasks;

mod task_notes;

mod meetings;

mod meeting_attendees;

mod conversations;

mod conversation_members;

mod messages;

mod task_assignees;

mod notifications;

mod saved_views;

mod auth_attempts;

mod folders;

mod documents;

mod org_keys;

mod logins;

mod login_accesses;

mod login_events;

mod reactions;

mod note_reactions;

/// How many lock requests are waiting right now, in the whole database.
/// Race tests use it to prove the other side is blocked, not just slow.
pub async fn waiting_locks(db: &sea_orm::DatabaseConnection) -> i64 {
    use sea_orm::{ConnectionTrait, Statement};
    db.query_one_raw(Statement::from_string(
        db.get_database_backend(),
        "SELECT count(*) AS n FROM pg_locks WHERE NOT granted",
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get::<i64>("", "n")
    .unwrap()
}
