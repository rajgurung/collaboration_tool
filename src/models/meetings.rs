use std::collections::HashMap;

use loco_rs::prelude::*;
use serde::Deserialize;

pub use super::_entities::meetings::{ActiveModel, Column, Entity, Model};
use super::{field_error, meeting_attendees, memberships};

pub type Meetings = Entity;

/// The log-a-meeting form. `attendee_ids` comes from repeated checkbox fields.
#[derive(Debug, Deserialize, Validate)]
pub struct MeetingParams {
    #[validate(length(
        min = 1,
        max = 120,
        message = "Give the meeting a title (up to 120 characters)."
    ))]
    pub title: String,
    #[serde(default)]
    pub held_on: String,
    #[validate(length(
        min = 1,
        max = 5000,
        message = "Write the minutes (up to 5,000 characters)."
    ))]
    pub summary: String,
    #[serde(default)]
    #[validate(length(max = 5000, message = "Keep decisions under 5,000 characters."))]
    pub decisions: String,
    #[serde(default)]
    pub attendee_ids: Vec<i64>,
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
    /// Meetings, newest first, each with its attendees' user ids.
    ///
    /// # Errors
    /// On database errors.
    pub async fn list_for_org<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
    ) -> ModelResult<Vec<(Self, Vec<i64>)>> {
        let meetings = Entity::find()
            .in_tenant(org_id)
            .order_by_desc(Column::HeldOn)
            .order_by_desc(Column::Id)
            .all(db)
            .await?;
        let mut attendees: HashMap<i64, Vec<i64>> = HashMap::new();
        for a in meeting_attendees::Entity::find()
            .in_tenant(org_id)
            .order_by_asc(meeting_attendees::Column::Id)
            .all(db)
            .await?
        {
            attendees.entry(a.meeting_id).or_default().push(a.user_id);
        }
        Ok(meetings
            .into_iter()
            .map(|m| {
                let ids = attendees.remove(&m.id).unwrap_or_default();
                (m, ids)
            })
            .collect())
    }

    /// Records a meeting and its attendees together.
    ///
    /// # Errors
    /// Validation errors (including attendees outside the team), or database errors.
    pub async fn create(
        db: &DatabaseConnection,
        org_id: i64,
        creator_id: i64,
        params: &MeetingParams,
    ) -> ModelResult<Self> {
        ValidatorTrait::validate(params)?;
        let held_on = chrono::NaiveDate::parse_from_str(params.held_on.trim(), "%Y-%m-%d")
            .map_err(|_| field_error("held_on", "Choose the date of the meeting."))?;
        let team: Vec<i64> = memberships::Model::team(db, org_id)
            .await?
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let mut attendee_ids = params.attendee_ids.clone();
        attendee_ids.sort_unstable();
        attendee_ids.dedup();
        if attendee_ids.iter().any(|id| !team.contains(id)) {
            return Err(field_error(
                "attendee_ids",
                "Attendees must be on the team.",
            ));
        }

        let txn = db.begin().await?;
        let meeting = ActiveModel {
            title: ActiveValue::Set(params.title.trim().to_string()),
            held_on: ActiveValue::Set(held_on),
            summary: ActiveValue::Set(params.summary.trim().to_string()),
            decisions: ActiveValue::Set(params.decisions.trim().to_string()),
            created_by_id: ActiveValue::Set(Some(creator_id)),
            ..Default::default()
        }
        .set_tenant(org_id)?
        .insert(&txn)
        .await?;
        for user_id in attendee_ids {
            meeting_attendees::ActiveModel {
                meeting_id: ActiveValue::Set(meeting.id),
                user_id: ActiveValue::Set(user_id),
                ..Default::default()
            }
            .set_tenant(org_id)?
            .insert(&txn)
            .await?;
        }
        txn.commit().await?;
        Ok(meeting)
    }
}

impl loco_rs::prelude::TenantEntity for Entity {
    type TenantId = i64;

    fn tenant_column() -> super::_entities::meetings::Column {
        super::_entities::meetings::Column::OrganisationId
    }
}
