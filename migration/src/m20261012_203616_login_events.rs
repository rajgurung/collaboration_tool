use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

/// The login activity log. Append-only: a trigger refuses every UPDATE and
/// DELETE, except the one Postgres makes itself when a login or user is
/// deleted and the link is set to NULL (the title and username stay). Clearing
/// it for tests is `TRUNCATE`, which row triggers do not see.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "login_events",
            &[
                ("id", ColType::PkAuto),
                ("login_title", ColType::StringNull),
                ("username", ColType::String),
                ("action", ColType::String),
                ("ip", ColType::String),
                ("user_agent", ColType::String),
                ("detail", ColType::StringNull),
            ],
            &[("organisation", ""), ("login?", ""), ("user?", "")],
        )
        .await?;
        for sql in [
            r#"CREATE INDEX "idx-login_events-org-created" ON login_events (organisation_id, created_at);"#,
            r#"CREATE INDEX "idx-login_events-login" ON login_events (login_id);"#,
            r#"CREATE INDEX "idx-login_events-user" ON login_events (user_id, action);"#,
            r"CREATE OR REPLACE FUNCTION login_events_append_only() RETURNS trigger LANGUAGE plpgsql AS $$
              BEGIN
                IF TG_OP = 'UPDATE'
                   AND (NEW.login_id IS NULL OR NEW.login_id IS NOT DISTINCT FROM OLD.login_id)
                   AND (NEW.user_id IS NULL OR NEW.user_id IS NOT DISTINCT FROM OLD.user_id)
                   AND (NEW.id, NEW.organisation_id, NEW.login_title, NEW.username, NEW.action,
                        NEW.ip, NEW.user_agent, NEW.detail, NEW.created_at, NEW.updated_at)
                       IS NOT DISTINCT FROM
                       (OLD.id, OLD.organisation_id, OLD.login_title, OLD.username, OLD.action,
                        OLD.ip, OLD.user_agent, OLD.detail, OLD.created_at, OLD.updated_at)
                THEN
                  RETURN NEW;
                END IF;
                RAISE EXCEPTION 'login_events is append-only';
              END $$;",
            r"CREATE TRIGGER login_events_append_only BEFORE UPDATE OR DELETE ON login_events
              FOR EACH ROW EXECUTE FUNCTION login_events_append_only();",
        ] {
            m.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TRIGGER IF EXISTS login_events_append_only ON login_events;
                 DROP FUNCTION IF EXISTS login_events_append_only();",
            )
            .await?;
        drop_table(m, "login_events").await
    }
}
