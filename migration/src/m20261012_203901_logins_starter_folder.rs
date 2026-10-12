use sea_orm_migration::prelude::*;

/// New organisations start with "Company" and "Logins". Existing ones get a
/// "Logins" folder here, unless they already have a folder by that name.
#[derive(DeriveMigrationName)]
pub struct Migration;

const NOTE: &str = "Shared logins for the team. Never put passwords in notes.";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(&format!(
                "INSERT INTO folders (organisation_id, name, note, created_at, updated_at)
                 SELECT o.id, 'Logins', '{NOTE}', now(), now() FROM organisations o
                 WHERE NOT EXISTS (
                   SELECT 1 FROM folders f WHERE f.organisation_id = o.id AND lower(f.name) = 'logins'
                 );"
            ))
            .await?;
        Ok(())
    }

    /// Removes the starter "Logins" folders that are still empty. Folders
    /// with files or logins in them stay.
    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(&format!(
                "DELETE FROM folders f WHERE f.name = 'Logins' AND f.note = '{NOTE}'
                 AND NOT EXISTS (SELECT 1 FROM documents d WHERE d.folder_id = f.id)
                 AND NOT EXISTS (SELECT 1 FROM logins l WHERE l.folder_id = f.id);"
            ))
            .await?;
        Ok(())
    }
}
