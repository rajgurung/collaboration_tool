use loco_rs::prelude::*;
use serde::Deserialize;

pub use super::_entities::organisations::{ActiveModel, Column, Entity, Model};
use super::{
    conversations, folders, memberships, projects, tasks,
    users::{self, RegisterParams},
};

/// One row of the platform admin's organisation list.
#[derive(Debug, serde::Serialize)]
pub struct OrgSummary {
    pub id: i64,
    pub name: String,
    pub slug: String,
    pub created_at: String,
    pub active_members: u64,
    pub pending_members: u64,
    pub projects: u64,
    pub tasks: u64,
}

pub type Organisations = Entity;

/// How many organisations one person can create, ever, counting the one
/// from signup. Counted from `created_by_id`, so leaving one frees nothing.
pub const MAX_ORGS_CREATED: u64 = 10;
/// How many organisations one person can create in any 24 hours.
pub const ORGS_PER_DAY: u64 = 3;
/// The error key `create_for_user` uses for the cap, so the form can show
/// the at-cap state instead of its fields.
pub const AT_CAP: &str = "at_cap";

/// Everything needed to create an organisation and its owner in one step.
#[derive(Debug, Deserialize, Validate)]
pub struct SignupParams {
    #[validate(custom(function = "trimmed_name_length"))]
    pub organisation_name: String,
    #[validate(
        regex(path = *users::USERNAME_RE, message = "Start with a letter and use 3 to 30 letters or numbers."),
        custom(function = "users::not_reserved")
    )]
    pub name: String,
    #[validate(email(message = "Enter a valid email address."))]
    pub email: String,
    #[validate(length(min = users::PASSWORD_MIN_LENGTH, max = 128, message = "Use at least 8 characters."))]
    pub password: String,
}

/// "New organisation" for someone already signed in: the same two fields as
/// signup, with the same rules and messages.
#[derive(Debug, Deserialize, Validate)]
pub struct NewOrgParams {
    #[validate(custom(function = "trimmed_name_length"))]
    pub organisation_name: String,
    #[validate(
        regex(path = *users::USERNAME_RE, message = "Start with a letter and use 3 to 30 letters or numbers."),
        custom(function = "users::not_reserved")
    )]
    pub name: String,
}

/// An organisation name is 2 to 80 characters once trimmed, so a name of
/// only spaces is refused instead of saved empty.
///
/// # Errors
/// A field error when the trimmed name is too short or too long.
pub fn trimmed_name_length(name: &str) -> Result<(), validator::ValidationError> {
    let len = name.trim().chars().count();
    if (2..=80).contains(&len) {
        return Ok(());
    }
    Err(validator::ValidationError::new("length").with_message("Use 2 to 80 characters.".into()))
}

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        if !insert && self.updated_at.is_unchanged() {
            let mut this = self;
            this.updated_at = sea_orm::ActiveValue::Set(chrono::Utc::now().into());
            Ok(this)
        } else {
            Ok(self)
        }
    }
}

impl Model {
    /// # Errors
    /// `EntityNotFound` when no organisation has this slug.
    pub async fn find_by_slug<C: ConnectionTrait>(db: &C, slug: &str) -> ModelResult<Self> {
        Entity::find()
            .filter(Column::Slug.eq(slug))
            .one(db)
            .await?
            .ok_or(ModelError::EntityNotFound)
    }

    /// # Errors
    /// `EntityNotFound` when the id does not exist.
    pub async fn find_by_id<C: ConnectionTrait>(db: &C, id: i64) -> ModelResult<Self> {
        Entity::find_by_id(id)
            .one(db)
            .await?
            .ok_or(ModelError::EntityNotFound)
    }

    /// Sets the time zone people in this organisation read times in. Times
    /// stay in UTC in the database.
    ///
    /// # Errors
    /// A field error for an unknown zone, or database errors.
    pub async fn set_timezone<C: ConnectionTrait>(self, db: &C, name: &str) -> ModelResult<Self> {
        let name = name.trim();
        if name.parse::<chrono_tz::Tz>().is_err() {
            return Err(super::field_error(
                "timezone",
                "Choose a time zone from the list.",
            ));
        }
        let mut org = self.into_active_model();
        org.timezone = ActiveValue::Set(name.to_string());
        Ok(org.update(db).await?)
    }

    /// Every organisation with headline counts, newest first. For the platform admin only.
    ///
    /// # Errors
    /// On database errors.
    pub async fn admin_overview<C: ConnectionTrait>(db: &C) -> ModelResult<Vec<OrgSummary>> {
        let orgs = Entity::find()
            .order_by_desc(Column::CreatedAt)
            .all(db)
            .await?;
        let mut rows = Vec::with_capacity(orgs.len());
        for org in orgs {
            let members = |status: &'static str| {
                memberships::Entity::find()
                    .in_tenant(org.id)
                    .filter(memberships::Column::Status.eq(status))
                    .count(db)
            };
            rows.push(OrgSummary {
                active_members: members(memberships::status::ACTIVE).await?,
                pending_members: members(memberships::status::PENDING).await?,
                projects: projects::Entity::find().in_tenant(org.id).count(db).await?,
                tasks: tasks::Entity::find().in_tenant(org.id).count(db).await?,
                created_at: org.created_at.format("%d %b %Y").to_string(),
                id: org.id,
                name: org.name,
                slug: org.slug,
            });
        }
        Ok(rows)
    }

    /// Creates the owner's account, the organisation, the owner membership and
    /// the `general` channel together. Nothing is saved if any step fails.
    ///
    /// # Errors
    /// Validation errors for bad input, `EntityAlreadyExists` for a taken email.
    pub async fn sign_up(
        db: &DatabaseConnection,
        params: &SignupParams,
    ) -> ModelResult<(users::Model, Self)> {
        ValidatorTrait::validate(params)?;
        let txn = db.begin().await?;
        let user = users::Model::create_with_password(
            &txn,
            &RegisterParams {
                email: params.email.clone(),
                password: params.password.clone(),
                name: params.name.trim().to_string(),
            },
        )
        .await?;
        let org =
            Self::create_with_owner(&txn, &params.organisation_name, &user, &user.name).await?;
        txn.commit().await?;
        Ok((user, org))
    }

    /// How many organisations this person has created, ever.
    ///
    /// # Errors
    /// On database errors.
    pub async fn created_count<C: ConnectionTrait>(db: &C, user_id: i64) -> ModelResult<u64> {
        Ok(Entity::find()
            .filter(Column::CreatedById.eq(user_id))
            .count(db)
            .await?)
    }

    /// "New organisation": another organisation for someone who already has
    /// an account, with them as its active owner under the username they
    /// chose. It copies `timezone` when given (the caller passes the current
    /// org's, except while acting). The welcome pop-up is marked seen, as
    /// they already use the app. Nothing else is taken from anywhere.
    ///
    /// The user's row is locked first, so two creates at once run one after
    /// the other and the limits and the duplicate check see both.
    ///
    /// # Errors
    /// Field errors for bad input, a name they already have and the daily
    /// limit; a field error on [`AT_CAP`] at the cap; database errors.
    pub async fn create_for_user(
        db: &DatabaseConnection,
        user: &users::Model,
        params: &NewOrgParams,
        timezone: Option<&str>,
    ) -> ModelResult<Self> {
        ValidatorTrait::validate(params)?;
        let txn = db.begin().await?;
        // FOR NO KEY UPDATE, so inserts elsewhere that point at this user
        // (their FOR KEY SHARE checks) don't wait on it.
        users::Entity::find_by_id(user.id)
            .lock(sea_orm::sea_query::LockType::NoKeyUpdate)
            .one(&txn)
            .await?
            .ok_or(ModelError::EntityNotFound)?;
        if Self::created_count(&txn, user.id).await? >= MAX_ORGS_CREATED {
            return Err(super::field_error(
                AT_CAP,
                "That's the most one account can set up for now.",
            ));
        }
        let today = Entity::find()
            .filter(Column::CreatedById.eq(user.id))
            .filter(Column::CreatedAt.gt(chrono::Utc::now() - chrono::Duration::hours(24)))
            .count(&txn)
            .await?;
        if today >= ORGS_PER_DAY {
            return Err(super::field_error(
                "organisation_name",
                &format!("You've set up {ORGS_PER_DAY} organisations today. Try again tomorrow."),
            ));
        }
        let wanted = comparable(&params.organisation_name);
        let taken = memberships::Model::for_user_with_orgs(&txn, user.id)
            .await?
            .into_iter()
            .find(|(m, org)| m.is_active() && comparable(&org.name) == wanted);
        if let Some((_, org)) = taken {
            return Err(super::field_error(
                "organisation_name",
                &format!(
                    "You already have an organisation called {}. Pick a name you can tell apart.",
                    org.name
                ),
            ));
        }

        let mut org =
            Self::create_with_owner(&txn, &params.organisation_name, user, params.name.trim())
                .await?;
        if let Some(zone) = timezone {
            org = org.set_timezone(&txn, zone).await?;
        }
        memberships::Model::mark_welcomed(&txn, org.id, user.id).await?;
        txn.commit().await?;
        Ok(org)
    }

    /// Creates an organisation with `owner` as its active owner, known there
    /// as `username`, the `general` channel and the starter document
    /// folders. Signup, "New organisation" and the super admin task all use
    /// it. Run it inside a transaction.
    ///
    /// # Errors
    /// On database errors.
    pub async fn create_with_owner<C: ConnectionTrait>(
        db: &C,
        name: &str,
        owner: &users::Model,
        username: &str,
    ) -> ModelResult<Self> {
        let name = name.trim().to_string();
        let org = ActiveModel {
            slug: ActiveValue::Set(unique_slug(db, &name).await?),
            name: ActiveValue::Set(name),
            created_by_id: ActiveValue::Set(Some(owner.id)),
            ..Default::default()
        }
        .insert(db)
        .await?;
        memberships::Model::create_owner(db, &org, owner, username).await?;
        conversations::Model::create_general(db, &org, owner).await?;
        folders::Model::create_starters(db, org.id, owner.id).await?;
        Ok(org)
    }
}

/// A name as the duplicate check compares it: trimmed, runs of whitespace
/// as one space, lowercase.
fn comparable(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// "Acme Ltd!" becomes "acme-ltd". Falls back to "org" when nothing usable is left.
fn slugify(name: &str) -> String {
    let mut slug = String::new();
    for ch in name.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_end_matches('-').chars().take(40).collect();
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() {
        "org".to_string()
    } else {
        slug
    }
}

async fn unique_slug<C: ConnectionTrait>(db: &C, name: &str) -> ModelResult<String> {
    let base = slugify(name);
    for n in 1.. {
        let candidate = if n == 1 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        if Entity::find()
            .filter(Column::Slug.eq(&candidate))
            .one(db)
            .await?
            .is_none()
        {
            return Ok(candidate);
        }
    }
    unreachable!("the loop only ends by returning")
}

#[cfg(test)]
mod tests {
    use super::{comparable, slugify};

    #[test]
    fn comparable_ignores_case_and_spacing() {
        assert_eq!(comparable("  acme   studio "), "acme studio");
        assert_eq!(comparable("ACME\tSTUDIO"), "acme studio");
        assert_eq!(comparable("ÉCOLE"), comparable("école"));
    }

    #[test]
    fn slugify_cleans_names() {
        assert_eq!(slugify("Himalayan Ritual"), "himalayan-ritual");
        assert_eq!(slugify("  Acme   Ltd!! "), "acme-ltd");
        assert_eq!(slugify("***"), "org");
        assert_eq!(slugify("Café Été"), "caf-t");
    }
}
