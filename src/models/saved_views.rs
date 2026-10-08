use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};

pub use super::_entities::saved_views::{ActiveModel, Column, Entity, Model};
use super::field_error;

pub type SavedViews = Entity;

/// Each person keeps at most this many views, so the chip row stays usable.
pub const MAX_PER_PERSON: u64 = 10;

/// A Tasks setup: whose tasks, how they're grouped, board or list, and search.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Setup {
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub view: String,
    #[serde(default)]
    pub q: String,
}

impl Setup {
    /// Unknown values fall back to the board's defaults.
    #[must_use]
    pub fn cleaned(&self) -> Self {
        let pick = |value: &str, allowed: &[&str]| {
            if allowed.contains(&value) {
                value.to_string()
            } else {
                allowed[0].to_string()
            }
        };
        Self {
            scope: pick(&self.scope, &["all", "mine"]),
            group: pick(&self.group, &["project", "person", "none"]),
            view: pick(&self.view, &["board", "list"]),
            q: self.q.trim().chars().take(100).collect(),
        }
    }

    /// The setup as query parameters. The phone list's filter follows whose tasks.
    #[must_use]
    pub fn query(&self) -> String {
        let mut query = format!(
            "view={}&scope={}&group={}&filter={}",
            self.view, self.scope, self.group, self.scope
        );
        if !self.q.is_empty() {
            query.push_str("&q=");
            query.push_str(&urlencoding_lite(&self.q));
        }
        query
    }

    /// The Tasks address that shows this setup.
    #[must_use]
    pub fn url(&self) -> String {
        format!("/tasks?{}", self.query())
    }
}

/// Enough escaping for a search term in a query string.
fn urlencoding_lite(raw: &str) -> String {
    raw.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The save and edit form: a name, whether Tasks opens with it, and the setup.
#[derive(Debug, Deserialize, Validate)]
pub struct ViewParams {
    #[validate(length(
        min = 1,
        max = 40,
        message = "Give the view a short name (up to 40 characters)."
    ))]
    pub name: String,
    /// A ticked checkbox sends "on"; an unticked one sends nothing.
    #[serde(default)]
    pub is_default: Option<String>,
    #[serde(flatten)]
    pub setup: Setup,
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
    #[must_use]
    pub fn setup(&self) -> Setup {
        Setup {
            scope: self.scope.clone(),
            group: self.lanes.clone(),
            view: self.layout.clone(),
            q: self.q.clone(),
        }
    }

    /// Someone's views, oldest first.
    ///
    /// # Errors
    /// On database errors.
    pub async fn list_for<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
    ) -> ModelResult<Vec<Self>> {
        Ok(Entity::find()
            .in_tenant(org_id)
            .filter(Column::UserId.eq(user_id))
            .order_by_asc(Column::Id)
            .all(db)
            .await?)
    }

    /// One of the viewer's own views.
    ///
    /// # Errors
    /// `EntityNotFound` when it belongs to someone else.
    pub async fn find_mine<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
        id: i64,
    ) -> ModelResult<Self> {
        Entity::find_by_id(id)
            .in_tenant(org_id)
            .filter(Column::UserId.eq(user_id))
            .one(db)
            .await?
            .ok_or(ModelError::EntityNotFound)
    }

    /// # Errors
    /// Validation errors, the per-person limit, or database errors.
    pub async fn create(
        db: &DatabaseConnection,
        org_id: i64,
        user_id: i64,
        params: &ViewParams,
    ) -> ModelResult<Self> {
        let name = params.name.trim().to_string();
        ValidatorTrait::validate(&ViewParams {
            name: name.clone(),
            is_default: None,
            setup: Setup::default(),
        })?;
        let count = Entity::find()
            .in_tenant(org_id)
            .filter(Column::UserId.eq(user_id))
            .count(db)
            .await?;
        if count >= MAX_PER_PERSON {
            return Err(field_error(
                "name",
                "You can keep up to 10 views. Delete one first.",
            ));
        }
        let setup = params.setup.cleaned();
        let is_default = params.is_default.is_some();
        let txn = db.begin().await?;
        if is_default {
            Self::clear_default(&txn, org_id, user_id).await?;
        }
        let view = ActiveModel {
            user_id: ActiveValue::Set(user_id),
            name: ActiveValue::Set(name),
            scope: ActiveValue::Set(setup.scope),
            lanes: ActiveValue::Set(setup.group),
            layout: ActiveValue::Set(setup.view),
            q: ActiveValue::Set(setup.q),
            is_default: ActiveValue::Set(is_default),
            ..Default::default()
        }
        .set_tenant(org_id)?
        .insert(&txn)
        .await?;
        txn.commit().await?;
        Ok(view)
    }

    /// Renames the view and sets whether Tasks opens with it.
    ///
    /// # Errors
    /// Validation or database errors.
    pub async fn edit(
        self,
        db: &DatabaseConnection,
        name: &str,
        is_default: bool,
    ) -> ModelResult<Self> {
        let name = name.trim().to_string();
        ValidatorTrait::validate(&ViewParams {
            name: name.clone(),
            is_default: None,
            setup: Setup::default(),
        })?;
        let txn = db.begin().await?;
        if is_default {
            Self::clear_default(&txn, self.organisation_id, self.user_id).await?;
        }
        let mut view = self.into_active_model();
        view.name = ActiveValue::Set(name);
        view.is_default = ActiveValue::Set(is_default);
        let view = view.update(&txn).await?;
        txn.commit().await?;
        Ok(view)
    }

    /// Replaces the saved setup with the current one.
    ///
    /// # Errors
    /// On database errors.
    pub async fn update_setup<C: ConnectionTrait>(
        self,
        db: &C,
        setup: &Setup,
    ) -> ModelResult<Self> {
        let setup = setup.cleaned();
        let mut view = self.into_active_model();
        view.scope = ActiveValue::Set(setup.scope);
        view.lanes = ActiveValue::Set(setup.group);
        view.layout = ActiveValue::Set(setup.view);
        view.q = ActiveValue::Set(setup.q);
        Ok(view.update(db).await?)
    }

    /// # Errors
    /// On database errors.
    pub async fn remove<C: ConnectionTrait>(self, db: &C) -> ModelResult<()> {
        self.into_active_model().delete(db).await?;
        Ok(())
    }

    async fn clear_default<C: ConnectionTrait>(
        db: &C,
        org_id: i64,
        user_id: i64,
    ) -> ModelResult<()> {
        Entity::update_many()
            .col_expr(Column::IsDefault, sea_orm::sea_query::Expr::value(false))
            .filter(Column::OrganisationId.eq(org_id))
            .filter(Column::UserId.eq(user_id))
            .exec(db)
            .await?;
        Ok(())
    }
}

impl loco_rs::prelude::TenantEntity for Entity {
    type TenantId = i64;

    fn tenant_column() -> Column {
        Column::OrganisationId
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setups_clean_up_and_link_to_tasks() {
        let setup = Setup {
            scope: "mine".into(),
            group: "person".into(),
            view: "nonsense".into(),
            q: "  launch & go  ".into(),
        }
        .cleaned();
        assert_eq!(setup.view, "board");
        assert_eq!(
            setup.url(),
            "/tasks?view=board&scope=mine&group=person&filter=mine&q=launch+%26+go"
        );
    }
}
