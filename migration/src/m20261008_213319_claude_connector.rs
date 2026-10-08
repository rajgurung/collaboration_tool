use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Connecting Claude: OAuth clients that registered themselves, short-lived
/// authorization codes, and access tokens (OAuth and personal). Codes and
/// tokens are stored as SHA-256 hashes, never as the plain value.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "oauth_clients",
            &[
                ("id", ColType::PkAuto),
                ("client_id", ColType::StringUniq),
                ("client_name", ColType::String),
                ("redirect_uris", ColType::JsonBinary),
            ],
            &[],
        )
        .await?;
        create_table(
            m,
            "oauth_codes",
            &[
                ("id", ColType::PkAuto),
                ("code_hash", ColType::StringUniq),
                ("grant_id", ColType::Uuid),
                ("redirect_uri", ColType::Text),
                ("code_challenge", ColType::String),
                ("resource", ColType::String),
                ("scope", ColType::String),
                ("expires_at", ColType::TimestampWithTimeZone),
                ("used_at", ColType::TimestampWithTimeZoneNull),
            ],
            &[
                ("oauth_clients", "oauth_client_id"),
                ("organisation", ""),
                ("user", ""),
            ],
        )
        .await?;
        create_table(
            m,
            "access_tokens",
            &[
                ("id", ColType::PkAuto),
                ("kind", ColType::String),
                ("name", ColType::String),
                ("token_hash", ColType::StringUniq),
                ("refresh_hash", ColType::StringNull),
                ("grant_id", ColType::Uuid),
                ("oauth_client_id", ColType::BigIntegerNull),
                ("resource", ColType::String),
                ("scope", ColType::String),
                ("expires_at", ColType::TimestampWithTimeZoneNull),
                ("refresh_expires_at", ColType::TimestampWithTimeZoneNull),
                ("last_used_at", ColType::TimestampWithTimeZoneNull),
                ("revoked_at", ColType::TimestampWithTimeZoneNull),
            ],
            &[("organisation", ""), ("user", "")],
        )
        .await?;
        // A nullable reference would get ON DELETE SET NULL; tokens go with their client.
        for sql in [
            r#"ALTER TABLE access_tokens ADD CONSTRAINT "fk-access_tokens-oauth_client_id-to-oauth_clients" FOREIGN KEY (oauth_client_id) REFERENCES oauth_clients (id) ON DELETE CASCADE ON UPDATE CASCADE;"#,
            r#"CREATE UNIQUE INDEX "idx-access_tokens-refresh_hash" ON access_tokens (refresh_hash);"#,
            r#"CREATE INDEX "idx-access_tokens-org-user" ON access_tokens (organisation_id, user_id);"#,
            r#"CREATE INDEX "idx-access_tokens-grant" ON access_tokens (grant_id);"#,
            r#"CREATE INDEX "idx-access_tokens-revoked_at" ON access_tokens (revoked_at);"#,
            r#"CREATE INDEX "idx-access_tokens-expiry" ON access_tokens (expires_at, refresh_expires_at);"#,
            r#"CREATE INDEX "idx-oauth_codes-grant" ON oauth_codes (grant_id);"#,
            r#"CREATE INDEX "idx-oauth_codes-created_at" ON oauth_codes (created_at);"#,
        ] {
            m.get_connection().execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "access_tokens").await?;
        drop_table(m, "oauth_codes").await?;
        drop_table(m, "oauth_clients").await
    }
}
