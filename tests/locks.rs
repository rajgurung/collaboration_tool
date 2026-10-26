//! Helpers for race tests: hold a row lock on a second connection, and see
//! whether anyone is waiting on one.
use loco_rs::app::AppContext;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, Statement, TransactionTrait,
};

/// A transaction on its own connection holding `lock` ("FOR UPDATE", "FOR
/// SHARE", "FOR KEY SHARE") on a message, which never waits more than a few
/// seconds itself.
pub async fn hold(
    ctx: &AppContext,
    message_id: i64,
    lock: &str,
) -> (DatabaseConnection, DatabaseTransaction) {
    let conn = sea_orm::Database::connect(&ctx.config.database.uri)
        .await
        .unwrap();
    let txn = conn.begin().await.unwrap();
    txn.execute_unprepared("SET LOCAL lock_timeout = '10s'")
        .await
        .unwrap();
    txn.execute_unprepared(&format!(
        "SELECT id FROM messages WHERE id = {message_id} {lock}"
    ))
    .await
    .unwrap();
    (conn, txn)
}

/// How many lock requests are waiting right now, in the whole database.
/// Race tests use it to prove the other side is blocked, not just slow.
pub async fn waiting_locks(db: &DatabaseConnection) -> i64 {
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
