use std::collections::HashMap;

use loco_rs::prelude::*;
use sea_orm::{
    sea_query::{ExprTrait, Func},
    SqlErr,
};
use serde::Deserialize;

pub use super::_entities::memberships::{ActiveModel, Column, Entity, Model};
use super::{
    access_tokens, conversation_members, conversations, field_error, login_accesses,
    login_events::{self, action, Client, Entry},
    notifications, organisations,
    users::{self, RegisterParams},
};

pub type Memberships = Entity;

pub mod role {
    pub const OWNER: &str = "owner";
    pub const ADMIN: &str = "admin";
    pub const MEMBER: &str = "member";
}

pub mod status {
    pub const PENDING: &str = "pending";
    pub const ACTIVE: &str = "active";
    pub const REJECTED: &str = "rejected";
    pub const REMOVED: &str = "removed";
}

/// How many requests to join one account may have waiting at once.
pub const MAX_PENDING_REQUESTS: u64 = 5;

/// The error key for [`MAX_PENDING_REQUESTS`]: about the whole form, not a
/// field, so the page shows it above the form.
pub const AT_PENDING_CAP: &str = "pending_cap";

/// 0 for active, 1 for pending, 2 for the rest, for ordering memberships
/// active first, then pending (ascending).
fn status_rank() -> sea_orm::sea_query::SimpleExpr {
    // Qualified, so a query that joins another table with a status column
    // still means this one.
    let status = || Expr::col((Entity, Column::Status));
    Expr::case(status().eq(status::ACTIVE), 0)
        .case(status().eq(status::PENDING), 1)
        .finally(2)
        .into()
}

/// "Ask to join" for someone signed in: the username to use in that
/// organisation, with the same rules and messages as signup.
#[derive(Debug, Deserialize, Validate)]
pub struct JoinParams {
    #[validate(
        regex(path = *users::USERNAME_RE, message = "Start with a letter and use 3 to 30 letters or numbers."),
        custom(function = "users::not_reserved")
    )]
    pub name: String,
}

/// What [`Model::request_to_join`] did.
#[derive(Debug)]
pub enum JoinOutcome {
    /// A new pending row, or a removed or declined one back to pending.
    Requested(Model),
    /// Already waiting. Nothing changed, the username included.
    AlreadyPending(Model),
    /// Already an active member. Nothing changed.
    AlreadyMember(Model),
}

/// The last decision on a membership that isn't active, from the row's own
/// dates (see [`Model::history`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum History {
    Removed(DateTimeWithTimeZone),
    Declined(DateTimeWithTimeZone),
    /// Declined before declines recorded their date.
    DeclinedBefore,
}

impl History {
    /// The owners' waiting-list line, e.g. "Removed on 3 October".
    #[must_use]
    pub fn line(&self, tz: chrono_tz::Tz) -> String {
        match self {
            Self::Removed(at) => format!("Removed on {}", crate::views::time::day(*at, tz)),
            Self::Declined(at) => format!("Declined on {}", crate::views::time::day(*at, tz)),
            Self::DeclinedBefore => "Declined before".to_string(),
        }
    }
}

/// Who approves or declines a request, from `CurrentMember`. `acting` is a
/// super admin inside an organisation they don't belong to.
#[derive(Debug, Clone, Copy)]
pub struct Approver {
    pub user_id: i64,
    pub acting: bool,
}

/// "That username is already used in {org}."
fn username_taken(org: &organisations::Model) -> ModelError {
    field_error(
        "name",
        &format!("That username is already used in {}.", org.name),
    )
}

/// Whether `err` is a violation of the unique index `index`.
fn violates(err: &DbErr, index: &str) -> bool {
    matches!(err.sql_err(), Some(SqlErr::UniqueConstraintViolation(m)) if m.contains(index))
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
    /// Each user's default membership (see [`Self::default_for_user`]), for
    /// the platform admin.
    ///
    /// # Errors
    /// On database errors.
    pub async fn default_by_user<C: ConnectionTrait>(db: &C) -> ModelResult<HashMap<i64, Self>> {
        // DISTINCT ON keeps the first row per user in this order, so the
        // database returns one membership per person, not every membership.
        Ok(Entity::find()
            .distinct_on([Column::UserId])
            .order_by_asc(Column::UserId)
            .order_by_asc(status_rank())
            .order_by_asc(Column::CreatedAt)
            .order_by_asc(Column::Id)
            .all(db)
            .await?
            .into_iter()
            .map(|m| (m.user_id, m))
            .collect())
    }

    /// Every membership of the user, in any status, oldest first.
    ///
    /// # Errors
    /// On database errors.
    pub async fn for_user<C: ConnectionTrait>(db: &C, user_id: i64) -> ModelResult<Vec<Self>> {
        Ok(Entity::find()
            .filter(Column::UserId.eq(user_id))
            .order_by_asc(Column::CreatedAt)
            .order_by_asc(Column::Id)
            .all(db)
            .await?)
    }

    /// How many active and pending memberships the user has, for whether the
    /// layout shows the organisation switcher. One small indexed query.
    ///
    /// # Errors
    /// On database errors.
    pub async fn switcher_counts<C: ConnectionTrait>(
        db: &C,
        user_id: i64,
    ) -> ModelResult<(i64, i64)> {
        let counts = Entity::find()
            .select_only()
            .column_as(
                Expr::cust(format!(
                    "count(*) FILTER (WHERE status = '{}')",
                    status::ACTIVE
                )),
                "active",
            )
            .column_as(
                Expr::cust(format!(
                    "count(*) FILTER (WHERE status = '{}')",
                    status::PENDING
                )),
                "pending",
            )
            .filter(Column::UserId.eq(user_id))
            .into_tuple::<(i64, i64)>()
            .one(db)
            .await?;
        Ok(counts.unwrap_or_default())
    }

    /// The user's active and pending memberships with their organisations,
    /// for the switcher: active first, then pending, each A to Z by name.
    /// Declined and removed ones are left out.
    ///
    /// # Errors
    /// On database errors.
    pub async fn for_user_with_orgs<C: ConnectionTrait>(
        db: &C,
        user_id: i64,
    ) -> ModelResult<Vec<(Self, organisations::Model)>> {
        Ok(Entity::find()
            .find_also_related(organisations::Entity)
            .filter(Column::UserId.eq(user_id))
            .filter(Column::Status.is_in([status::ACTIVE, status::PENDING]))
            .order_by_asc(status_rank())
            .order_by_asc(Expr::expr(Func::lower(Expr::col((
                organisations::Entity,
                organisations::Column::Name,
            )))))
            .order_by_asc(Expr::col((
                organisations::Entity,
                organisations::Column::Id,
            )))
            .all(db)
            .await?
            .into_iter()
            .filter_map(|(m, org)| org.map(|org| (m, org)))
            .collect())
    }

    /// The membership to use when no organisation has been chosen: the
    /// oldest active one, else the oldest pending one, else the oldest in
    /// any status (so the person sees their waiting, removed or declined
    /// page, a new request before an old removal). To check access to a
    /// given organisation, use [`Self::active_in`] with that organisation's
    /// id instead.
    ///
    /// # Errors
    /// On database errors.
    pub async fn default_for_user<C: ConnectionTrait>(
        db: &C,
        user_id: i64,
    ) -> ModelResult<Option<Self>> {
        Ok(Entity::find()
            .filter(Column::UserId.eq(user_id))
            .order_by_asc(status_rank())
            .order_by_asc(Column::CreatedAt)
            .order_by_asc(Column::Id)
            .one(db)
            .await?)
    }

    /// Makes `user` the active owner of `org`, known there as `username`.
    ///
    /// # Errors
    /// On database errors.
    pub async fn create_owner<C: ConnectionTrait>(
        db: &C,
        org: &organisations::Model,
        user: &users::Model,
        username: &str,
    ) -> ModelResult<Self> {
        Ok(ActiveModel {
            user_id: ActiveValue::Set(user.id),
            username: ActiveValue::Set(username.to_string()),
            role: ActiveValue::Set(role::OWNER.to_string()),
            status: ActiveValue::Set(status::ACTIVE.to_string()),
            approved_at: ActiveValue::Set(Some(chrono::Utc::now().into())),
            ..Default::default()
        }
        .set_tenant(org.id)?
        .insert(db)
        .await?)
    }

    /// Creates the account and a pending membership. An owner or admin
    /// approves it later. Never touches an existing account: a taken email
    /// is refused.
    ///
    /// # Errors
    /// Validation errors (including a username already used in this
    /// organisation) and `EntityAlreadyExists` for a taken email.
    pub async fn join_new_account(
        db: &DatabaseConnection,
        org: &organisations::Model,
        params: &RegisterParams,
    ) -> ModelResult<(users::Model, Self)> {
        ValidatorTrait::validate(params)?;
        let txn = db.begin().await?;
        let username = params.name.trim().to_string();
        let taken = Entity::find()
            .in_tenant(org.id)
            .filter(
                Expr::expr(Func::lower(Expr::col(Column::Username))).eq(username.to_lowercase()),
            )
            .one(&txn)
            .await?
            .is_some();
        if taken {
            return Err(field_error(
                "name",
                "That username is already used in this organisation.",
            ));
        }
        let user = users::Model::create_with_password(&txn, params).await?;
        let membership = ActiveModel {
            user_id: ActiveValue::Set(user.id),
            username: ActiveValue::Set(username),
            role: ActiveValue::Set(role::MEMBER.to_string()),
            status: ActiveValue::Set(status::PENDING.to_string()),
            ..Default::default()
        }
        .set_tenant(org.id)?
        .insert(&txn)
        .await?;
        txn.commit().await?;
        Ok((user, membership))
    }

    /// "Ask to join" with an account the caller has signed in as: a pending
    /// membership of `org` for `user_id`. Always pass the signed-in user's
    /// own id. It only ever writes `pending`: a removed or declined row goes
    /// back to pending (same row, role and history kept, see
    /// [`Self::history`]), and a new row is a member that has been welcomed
    /// already, as they know the app. Active and pending rows are left as
    /// they are.
    ///
    /// Locks the membership row, then the account row, the order `remove`
    /// and `approve` use. Two requests at once run one after the other, so
    /// the pending cap and the double-submit check see both; the unique
    /// indexes are the backstop for two that pass before either locks.
    ///
    /// # Errors
    /// Field errors on `name`: bad or reserved username (checked after
    /// trimming), one already used in this organisation, or a locked
    /// account. [`AT_PENDING_CAP`] at [`MAX_PENDING_REQUESTS`] waiting.
    /// Database errors.
    pub async fn request_to_join(
        db: &DatabaseConnection,
        org: &organisations::Model,
        user_id: i64,
        params: &JoinParams,
    ) -> ModelResult<JoinOutcome> {
        let params = JoinParams {
            name: params.name.trim().to_string(),
        };
        ValidatorTrait::validate(&params)?;
        match Self::file_request(db, org, user_id, &params.name).await {
            Err(ModelError::DbErr(err)) if violates(&err, "idx-memberships-org-user") => {
                // Another request inserted first. This transaction has rolled
                // back, so read what that one made.
                let row = Self::in_org_for_user(db, org.id, user_id)
                    .await?
                    .ok_or(ModelError::EntityNotFound)?;
                Ok(if row.is_active() {
                    JoinOutcome::AlreadyMember(row)
                } else {
                    JoinOutcome::AlreadyPending(row)
                })
            }
            Err(ModelError::DbErr(err)) if violates(&err, "idx-memberships-org-username") => {
                Err(username_taken(org))
            }
            other => other,
        }
    }

    async fn file_request(
        db: &DatabaseConnection,
        org: &organisations::Model,
        user_id: i64,
        username: &str,
    ) -> ModelResult<JoinOutcome> {
        let txn = db.begin().await?;
        let row = || {
            Entity::find()
                .in_tenant(org.id)
                .filter(Column::UserId.eq(user_id))
        };
        let mut existing = row().lock_exclusive().one(&txn).await?;
        if let Some(outcome) = existing.clone().and_then(Self::settled) {
            return Ok(outcome);
        }
        // FOR NO KEY UPDATE, as in `organisations::create_for_user`, so
        // inserts elsewhere that point at this user don't wait on it.
        let user = users::Entity::find_by_id(user_id)
            .lock(sea_orm::sea_query::LockType::NoKeyUpdate)
            .one(&txn)
            .await?
            .ok_or(ModelError::EntityNotFound)?;
        // A lock in another request may have landed after this one signed in.
        if user.is_locked() {
            return Err(field_error(
                "name",
                "Your account is locked. Reset your password to unlock it.",
            ));
        }
        if existing.is_none() {
            // A double submit waited on the account lock above; now it sees
            // the first one's row. A plain read: locking the membership row
            // after the account row would be the reverse of `approve`'s
            // order. The update below checks the status again.
            existing = row().one(&txn).await?;
            if let Some(outcome) = existing.clone().and_then(Self::settled) {
                return Ok(outcome);
            }
        }
        let waiting = Entity::find()
            .filter(Column::UserId.eq(user_id))
            .filter(Column::Status.eq(status::PENDING))
            .count(&txn)
            .await?;
        if waiting >= MAX_PENDING_REQUESTS {
            return Err(field_error(
                AT_PENDING_CAP,
                &format!(
                    "You've already asked to join {MAX_PENDING_REQUESTS} organisations. You can ask again once one of them lets you in or declines."
                ),
            ));
        }
        let taken = Entity::find()
            .in_tenant(org.id)
            .filter(Column::UserId.ne(user_id))
            .filter(
                Expr::expr(Func::lower(Expr::col(Column::Username))).eq(username.to_lowercase()),
            )
            .one(&txn)
            .await?
            .is_some();
        if taken {
            return Err(username_taken(org));
        }
        let now = chrono::Utc::now().fixed_offset();
        let membership = if let Some(old) = existing {
            // Role, approval and removal stay: they are the history, and
            // `approve` overwrites them.
            Entity::update_many()
                .col_expr(Column::Status, Expr::value(status::PENDING))
                .col_expr(Column::Username, Expr::value(username))
                .col_expr(Column::UpdatedAt, Expr::value(now))
                .filter(Column::Id.eq(old.id))
                .filter(Column::Status.is_in([status::REMOVED, status::REJECTED]))
                .exec_with_returning(&txn)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| field_error("name", "That request changed. Try again."))?
        } else {
            ActiveModel {
                user_id: ActiveValue::Set(user_id),
                username: ActiveValue::Set(username.to_string()),
                role: ActiveValue::Set(role::MEMBER.to_string()),
                status: ActiveValue::Set(status::PENDING.to_string()),
                welcomed_at: ActiveValue::Set(Some(now)),
                ..Default::default()
            }
            .set_tenant(org.id)?
            .insert(&txn)
            .await?
        };
        txn.commit().await?;
        Ok(JoinOutcome::Requested(membership))
    }

    /// The outcome for a row a request leaves alone: active or pending.
    fn settled(row: Self) -> Option<JoinOutcome> {
        if row.is_active() {
            Some(JoinOutcome::AlreadyMember(row))
        } else if row.is_pending() {
            Some(JoinOutcome::AlreadyPending(row))
        } else {
            None
        }
    }

    /// The last decision on a row that isn't active, from its own dates,
    /// never `updated_at`: removed when `removed_at` is the later date,
    /// else declined on `approved_at` (a decline records its date there).
    /// A decline from before declines had dates has `approved_by_id`
    /// alone. A first request, or an active row, has none.
    #[must_use]
    pub fn history(&self) -> Option<History> {
        if self.is_active() {
            return None;
        }
        match (self.removed_at, self.approved_at) {
            (Some(removed), Some(decided)) if decided > removed => Some(History::Declined(decided)),
            (Some(removed), _) => Some(History::Removed(removed)),
            (None, Some(decided)) => Some(History::Declined(decided)),
            (None, None) if self.approved_by_id.is_some() => Some(History::DeclinedBefore),
            (None, None) => None,
        }
    }

    /// Every membership in the organisation with its user, oldest first.
    ///
    /// # Errors
    /// On database errors.
    pub async fn list_for_org<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
    ) -> ModelResult<Vec<(Self, users::Model)>> {
        let memberships = Entity::find()
            .in_tenant(org_id)
            .order_by_asc(Column::CreatedAt)
            .all(db)
            .await?;
        let mut users: HashMap<i64, users::Model> = users::Entity::find()
            .filter(users::users::Column::Id.is_in(memberships.iter().map(|m| m.user_id)))
            .all(db)
            .await?
            .into_iter()
            .map(|u| (u.id, u))
            .collect();
        Ok(memberships
            .into_iter()
            .filter_map(|m| users.remove(&m.user_id).map(|u| (m, u)))
            .collect())
    }

    /// `(user_id, username)` for every approved member, oldest first.
    ///
    /// # Errors
    /// On database errors.
    pub async fn team<C: ConnectionTrait>(db: &C, org_id: i64) -> ModelResult<Vec<(i64, String)>> {
        Ok(Entity::find()
            .in_tenant(org_id)
            .filter(Column::Status.eq(status::ACTIVE))
            .order_by_asc(Column::CreatedAt)
            .all(db)
            .await?
            .into_iter()
            .map(|m| (m.user_id, m.username))
            .collect())
    }

    /// The organisation's active owners, with their accounts.
    ///
    /// # Errors
    /// On database errors.
    pub async fn owners<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
    ) -> ModelResult<Vec<(Self, users::Model)>> {
        Ok(Self::list_for_org(db, org_id)
            .await?
            .into_iter()
            .filter(|(m, _)| m.is_active() && m.role == role::OWNER)
            .collect())
    }

    /// `(user_id, username)` for the approved members among `user_ids`.
    ///
    /// # Errors
    /// On database errors.
    pub async fn team_among<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_ids: impl IntoIterator<Item = i64>,
    ) -> ModelResult<Vec<(i64, String)>> {
        Ok(Entity::find()
            .in_tenant(org_id)
            .filter(Column::Status.eq(status::ACTIVE))
            .filter(Column::UserId.is_in(user_ids))
            .all(db)
            .await?
            .into_iter()
            .map(|m| (m.user_id, m.username))
            .collect())
    }

    /// # Errors
    /// `EntityNotFound` when the membership is not in this organisation.
    pub async fn find_in_org<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        id: i64,
    ) -> ModelResult<Self> {
        Entity::find_by_id(id)
            .in_tenant(org_id)
            .one(db)
            .await?
            .ok_or(ModelError::EntityNotFound)
    }

    /// Approves a pending request in one transaction. The approver is
    /// checked again first (a page left open after a demotion can't
    /// approve), then one conditional update makes the row an active member:
    /// only from pending, never the approver's own request, and whatever the
    /// role was before (a rejoin keeps an old role as history). It clears
    /// the removal and records who approved and when. Then, after locking
    /// the account row (the order `remove` uses), it ends a logins unlock
    /// for this organisation, revokes any Claude tokens and unused codes the
    /// person still has here, clears logins shared with them by name here,
    /// and adds them to `#general` and back to their direct messages. For a
    /// first join those find nothing to undo.
    ///
    /// # Errors
    /// `EntityNotFound` when `id` is not in this organisation, `Message` when
    /// the approver may not, the request is not pending or it is their own,
    /// or database errors.
    pub async fn approve(
        db: &DatabaseConnection,
        org_id: i64,
        approver: Approver,
        id: i64,
    ) -> ModelResult<Self> {
        let txn = db.begin().await?;
        Self::check_approver(
            &txn,
            org_id,
            approver,
            "Only owners and admins can approve.",
        )
        .await?;
        let now = chrono::Utc::now().fixed_offset();
        let approved = Entity::update_many()
            .col_expr(Column::Status, Expr::value(status::ACTIVE))
            .col_expr(Column::Role, Expr::value(role::MEMBER))
            .col_expr(Column::ApprovedById, Expr::value(Some(approver.user_id)))
            .col_expr(Column::ApprovedAt, Expr::value(Some(now)))
            .col_expr(
                Column::RemovedAt,
                Expr::value(None::<chrono::DateTime<chrono::FixedOffset>>),
            )
            .col_expr(Column::RemovedById, Expr::value(None::<i64>))
            .col_expr(Column::UpdatedAt, Expr::value(now))
            .filter(Column::Id.eq(id))
            .filter(Column::OrganisationId.eq(org_id))
            .filter(Column::Status.eq(status::PENDING))
            .filter(Column::UserId.ne(approver.user_id))
            .exec_with_returning(&txn)
            .await?
            .into_iter()
            .next();
        let Some(membership) = approved else {
            let row = Self::find_in_org(&txn, org_id, id).await?;
            // Only an acting super admin can be both approver and asker.
            if row.is_pending() && row.user_id == approver.user_id {
                return Err(ModelError::msg("You can't approve your own request."));
            }
            return Err(ModelError::msg("Only pending requests can be approved."));
        };
        let user_id = membership.user_id;
        users::Entity::find_by_id(user_id)
            .lock(sea_orm::sea_query::LockType::NoKeyUpdate)
            .one(&txn)
            .await?;
        users::Model::end_unlock_in(&txn, user_id, org_id).await?;
        access_tokens::Model::revoke_all_for(&txn, org_id, user_id).await?;
        login_accesses::Model::remove_for_member(&txn, org_id, user_id).await?;
        let general = conversations::Model::find_general(&txn, org_id).await?;
        conversation_members::Model::add_if_missing(&txn, &general, user_id).await?;
        // Groups are not restored, as for a first join.
        for dm in conversations::Model::dms_of(&txn, org_id, user_id).await? {
            conversation_members::Model::add_if_missing(&txn, &dm, user_id).await?;
        }
        txn.commit().await?;
        tracing::info!(
            org_id,
            user_id,
            approver_id = approver.user_id,
            "approved member"
        );
        Ok(membership)
    }

    /// Declines a pending request with one conditional update, so a page
    /// left open can't turn a member who was just approved into a declined
    /// one. Records who declined and when (`approved_at` is the time of the
    /// last decision). The approver is checked again, as in
    /// [`Self::approve`].
    ///
    /// # Errors
    /// `EntityNotFound` when `id` is not in this organisation, `Message` when
    /// the approver may not or the request is not pending, or database
    /// errors.
    pub async fn reject(
        db: &DatabaseConnection,
        org_id: i64,
        approver: Approver,
        id: i64,
    ) -> ModelResult<Self> {
        let txn = db.begin().await?;
        Self::check_approver(
            &txn,
            org_id,
            approver,
            "Only owners and admins can decline.",
        )
        .await?;
        let now = chrono::Utc::now().fixed_offset();
        let declined = Entity::update_many()
            .col_expr(Column::Status, Expr::value(status::REJECTED))
            .col_expr(Column::ApprovedById, Expr::value(Some(approver.user_id)))
            .col_expr(Column::ApprovedAt, Expr::value(Some(now)))
            .col_expr(Column::UpdatedAt, Expr::value(now))
            .filter(Column::Id.eq(id))
            .filter(Column::OrganisationId.eq(org_id))
            .filter(Column::Status.eq(status::PENDING))
            .exec_with_returning(&txn)
            .await?
            .into_iter()
            .next();
        let Some(membership) = declined else {
            Self::find_in_org(&txn, org_id, id).await?;
            return Err(ModelError::msg("Only pending requests can be declined."));
        };
        txn.commit().await?;
        Ok(membership)
    }

    /// Refuses unless the approver is an active owner or admin here, read
    /// `FOR SHARE` so a demotion or removal that is committing is seen.
    /// Skipped while acting: the super admin has no membership here.
    async fn check_approver<C: ConnectionTrait>(
        txn: &C,
        org_id: i64,
        approver: Approver,
        refusal: &str,
    ) -> ModelResult<()> {
        if approver.acting {
            return Ok(());
        }
        Self::active_in_for_share(txn, org_id, approver.user_id)
            .await?
            .filter(|m| m.role == role::OWNER || m.role == role::ADMIN)
            .map(|_| ())
            .ok_or_else(|| ModelError::msg(refusal))
    }

    /// Removes an active member. The rules are checked in a transaction that
    /// locks the organisation's owners: nobody removes themselves, only an
    /// owner removes an owner, and the last owner stays. Removal ends access
    /// to this organisation only: it revokes the person's Claude tokens here,
    /// ends an unlock of logins made here, takes them out of every
    /// conversation here and stops mention emails from here, then closes
    /// their chat sockets and notification streams for this organisation on
    /// every device. Their sessions, and everything in their other
    /// organisations, are untouched; `CurrentMember` stops serving this one
    /// on the next request. Login activity records who removed whom.
    ///
    /// # Errors
    /// `EntityNotFound` when `id` is not in this organisation, `Message` when
    /// a rule refuses it, or database errors.
    pub async fn remove(
        ctx: &AppContext,
        org_id: i64,
        remover_id: i64,
        id: i64,
        client: &Client,
    ) -> ModelResult<Self> {
        let txn = ctx.db.begin().await?;
        let owners = Entity::find()
            .in_tenant(org_id)
            .filter(Column::Role.eq(role::OWNER))
            .filter(Column::Status.eq(status::ACTIVE))
            .lock_exclusive()
            .all(&txn)
            .await?;
        let target = Entity::find_by_id(id)
            .in_tenant(org_id)
            .lock_exclusive()
            .one(&txn)
            .await?
            .ok_or(ModelError::EntityNotFound)?;
        let remover = Self::active_in(&txn, org_id, remover_id)
            .await?
            .filter(|m| m.role == role::OWNER || m.role == role::ADMIN)
            .ok_or_else(|| ModelError::msg("Only owners and admins can remove people."))?;
        if target.user_id == remover_id {
            return Err(ModelError::msg("You can't remove yourself."));
        }
        if !target.is_active() {
            return Err(ModelError::msg("Only approved members can be removed."));
        }
        if target.role == role::OWNER {
            if remover.role != role::OWNER {
                return Err(ModelError::msg("Only an owner can remove an owner."));
            }
            if owners.len() <= 1 {
                return Err(ModelError::msg("The last owner can't be removed."));
            }
        }
        let user_id = target.user_id;
        let mut membership = target.into_active_model();
        membership.status = ActiveValue::Set(status::REMOVED.to_string());
        membership.removed_at = ActiveValue::Set(Some(chrono::Utc::now().into()));
        membership.removed_by_id = ActiveValue::Set(Some(remover_id));
        let membership = membership.update(&txn).await?;
        // The account row before the tokens, the order a password reset, a
        // lock and a token refresh use, so they wait for each other instead
        // of deadlocking. FOR NO KEY UPDATE, so inserts elsewhere that point
        // at this user (their FOR KEY SHARE checks) don't wait on it, and
        // nor does `removed_by_id` in a removal the other way round.
        users::Entity::find_by_id(user_id)
            .lock(sea_orm::sea_query::LockType::NoKeyUpdate)
            .one(&txn)
            .await?;
        users::Model::end_unlock_in(&txn, user_id, org_id).await?;
        access_tokens::Model::revoke_all_for(&txn, org_id, user_id).await?;
        conversation_members::Model::remove_everywhere(&txn, org_id, user_id).await?;
        notifications::Model::stop_emails(&txn, org_id, user_id).await?;
        login_events::Model::record(
            &txn,
            client,
            Entry {
                org_id,
                login: None,
                user: (remover_id, &remover.username),
                action: action::MEMBER_REMOVED,
                detail: Some(membership.username.clone()),
            },
        )
        .await?;
        txn.commit().await?;
        users::Model::close_in_org(ctx, org_id, user_id);
        tracing::info!(org_id, user_id, remover_id, "removed member");
        Ok(membership)
    }

    /// Who may see a Remove button for `target`. [`Self::remove`] checks the
    /// same rules again when it runs.
    #[must_use]
    pub fn can_remove(&self, remover_id: i64, remover_role: &str, owners: usize) -> bool {
        let manager = remover_role == role::OWNER || remover_role == role::ADMIN;
        let owner_rule = self.role != role::OWNER || (remover_role == role::OWNER && owners > 1);
        manager && self.user_id != remover_id && self.is_active() && owner_rule
    }

    /// Makes an active member an admin or turns an admin back into a member.
    /// Owners keep their role.
    ///
    /// # Errors
    /// For an unknown role, an owner, an inactive member, or database errors.
    pub async fn change_role<C: ConnectionTrait>(
        self,
        db: &C,
        new_role: &str,
    ) -> ModelResult<Self> {
        if new_role != role::ADMIN && new_role != role::MEMBER {
            return Err(ModelError::msg("Choose admin or member."));
        }
        if self.role == role::OWNER {
            return Err(ModelError::msg("The owner's role cannot be changed."));
        }
        if !self.is_active() {
            return Err(ModelError::msg(
                "Approve the request before changing the role.",
            ));
        }
        let mut membership = self.into_active_model();
        membership.role = ActiveValue::Set(new_role.to_string());
        Ok(membership.update(db).await?)
    }

    /// Whether `user_id` is an approved member of the organisation.
    ///
    /// # Errors
    /// On database errors.
    pub async fn is_active_member<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
    ) -> ModelResult<bool> {
        Ok(Entity::find()
            .in_tenant(org_id)
            .filter(Column::UserId.eq(user_id))
            .filter(Column::Status.eq(status::ACTIVE))
            .one(db)
            .await?
            .is_some())
    }

    /// The viewer's own approved membership in this organisation, if any.
    /// A platform admin visiting another organisation has none.
    ///
    /// # Errors
    /// On database errors.
    pub async fn active_in<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
    ) -> ModelResult<Option<Self>> {
        Ok(Entity::find()
            .in_tenant(org_id)
            .filter(Column::UserId.eq(user_id))
            .filter(Column::Status.eq(status::ACTIVE))
            .one(db)
            .await?)
    }

    /// [`Self::active_in`], reading the row `FOR SHARE`: it waits for a
    /// removal of this membership that is committing, then sees `removed`.
    /// Other readers never wait. For minting tokens and unlocking logins,
    /// inside their transaction.
    ///
    /// # Errors
    /// On database errors.
    pub async fn active_in_for_share<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
    ) -> ModelResult<Option<Self>> {
        // Locks the row whatever its status, then checks it, so a removal
        // that commits while this waits is seen.
        Ok(Entity::find()
            .in_tenant(org_id)
            .filter(Column::UserId.eq(user_id))
            .lock_shared()
            .one(db)
            .await?
            .filter(Self::is_active))
    }

    /// This user's membership in the organisation in any status, for naming
    /// an organisation they were removed from. Always pass the signed-in
    /// user's own id, never one from the request.
    ///
    /// # Errors
    /// On database errors.
    pub async fn in_org_for_user<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
    ) -> ModelResult<Option<Self>> {
        Ok(Entity::find()
            .in_tenant(org_id)
            .filter(Column::UserId.eq(user_id))
            .one(db)
            .await?)
    }

    /// Closes the "Getting started" guide (`hidden`) or brings it back.
    ///
    /// # Errors
    /// On database errors.
    pub async fn set_guide_hidden<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
        hidden: bool,
    ) -> ModelResult<()> {
        let at = hidden.then(|| chrono::Utc::now().fixed_offset());
        Self::set_for(db, org_id, user_id, Column::GuideDismissedAt, at).await
    }

    /// Records that the welcome pop-up was seen, so it shows only once.
    ///
    /// # Errors
    /// On database errors.
    pub async fn mark_welcomed<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
    ) -> ModelResult<()> {
        let at = Some(chrono::Utc::now().fixed_offset());
        Self::set_for(db, org_id, user_id, Column::WelcomedAt, at).await
    }

    async fn set_for<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
        column: Column,
        at: Option<chrono::DateTime<chrono::FixedOffset>>,
    ) -> ModelResult<()> {
        Entity::update_many()
            .col_expr(column, sea_orm::sea_query::Expr::value(at))
            .col_expr(
                Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(chrono::Utc::now().fixed_offset()),
            )
            .filter(Column::OrganisationId.eq(org_id))
            .filter(Column::UserId.eq(user_id))
            .exec(db)
            .await?;
        Ok(())
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.status == status::ACTIVE
    }

    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.status == status::PENDING
    }

    #[must_use]
    pub fn is_removed(&self) -> bool {
        self.status == status::REMOVED
    }
}

impl loco_rs::prelude::TenantEntity for Entity {
    type TenantId = i64;

    fn tenant_column() -> super::_entities::memberships::Column {
        super::_entities::memberships::Column::OrganisationId
    }
}
