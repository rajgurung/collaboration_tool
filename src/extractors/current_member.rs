//! The single gate for tenant pages. A handler that takes `CurrentMember` only
//! runs for an approved member of an organisation (or the platform super admin
//! acting inside one), and `member.org.id` is the only tenant id it should use.
use axum::{extract::FromRequestParts, http::request::Parts, http::StatusCode};
use loco_rs::prelude::*;

use super::{current_user::CurrentUser, session::redirect_response};
use crate::{
    models::{
        memberships::{self, role},
        organisations, users,
    },
    views::layout::avatar_color,
};

/// Cookie a super admin sets to work inside another organisation.
pub const ACTING_ORG_COOKIE: &str = "acting_org";

#[derive(Clone)]
pub struct CurrentMember {
    pub user: users::Model,
    pub org: organisations::Model,
    pub role: String,
    /// The username shown inside this organisation.
    pub username: String,
    /// True when a super admin is inside an organisation they do not belong to.
    pub acting: bool,
}

impl CurrentMember {
    /// A member through their own approved membership. Used by the session
    /// cookie and by Claude's bearer tokens alike.
    #[must_use]
    pub fn from_membership(
        user: users::Model,
        org: organisations::Model,
        membership: memberships::Model,
    ) -> Self {
        Self {
            user,
            org,
            role: membership.role,
            username: membership.username,
            acting: false,
        }
    }

    /// Owners and admins approve members and manage settings.
    #[must_use]
    pub fn can_manage(&self) -> bool {
        self.role == role::OWNER || self.role == role::ADMIN
    }

    /// The organisation's time zone, for showing times.
    #[must_use]
    pub fn tz(&self) -> chrono_tz::Tz {
        crate::views::time::zone(&self.org.timezone)
    }

    #[must_use]
    pub fn is_owner(&self) -> bool {
        self.role == role::OWNER
    }

    /// # Errors
    /// 403 unless the member is an owner or admin.
    pub fn require_manager(&self) -> Result<()> {
        if self.can_manage() {
            Ok(())
        } else {
            Err(forbidden("Only owners and admins can do that."))
        }
    }

    /// # Errors
    /// 403 unless the member is an owner.
    pub fn require_owner(&self) -> Result<()> {
        if self.is_owner() {
            Ok(())
        } else {
            Err(forbidden("Only owners can do that."))
        }
    }

    /// Context every app-layout page needs: org, signed-in user and active tab.
    #[must_use]
    pub fn page(&self, active: &str, mut extra: serde_json::Value) -> serde_json::Value {
        let mut base = serde_json::json!({
            "active": active,
            "org": { "name": self.org.name, "slug": self.org.slug },
            "me": {
                "id": self.user.id,
                "username": self.username,
                "color": avatar_color(&self.username),
                "role": self.role,
                "can_manage": self.can_manage(),
                "is_super_admin": self.user.is_super_admin,
                "acting": self.acting,
            },
        });
        if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object_mut()) {
            base.append(extra);
        }
        base
    }
}

impl FromRequestParts<AppContext> for CurrentMember {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        ctx: &AppContext,
    ) -> std::result::Result<Self, Self::Rejection> {
        let CurrentUser(user) = CurrentUser::from_request_parts(parts, ctx).await?;

        if user.is_super_admin {
            if let Some(org) = acting_org(parts, ctx).await {
                return Ok(Self {
                    username: user.name.clone(),
                    user,
                    org,
                    role: role::OWNER.to_string(),
                    acting: true,
                });
            }
        }

        let membership = memberships::Model::find_for_user(&ctx.db, user.id)
            .await
            .map_err(|e| Error::from(e).into_response())?;
        let Some(membership) = membership else {
            if user.is_super_admin {
                return Err(redirect_response(&parts.headers, "/admin"));
            }
            return Err(render(
                parts,
                StatusCode::FORBIDDEN,
                "tenancy/no_org.html",
                serde_json::json!({}),
            ));
        };
        let org = organisations::Model::find_by_id(&ctx.db, membership.organisation_id)
            .await
            .map_err(|e| Error::from(e).into_response())?;

        if membership.is_active() {
            return Ok(Self::from_membership(user, org, membership));
        }
        let template = if membership.is_pending() {
            "tenancy/waiting.html"
        } else {
            "tenancy/declined.html"
        };
        Err(render(
            parts,
            StatusCode::FORBIDDEN,
            template,
            serde_json::json!({
                "org": { "name": org.name },
                "username": membership.username,
            }),
        ))
    }
}

fn forbidden(message: &str) -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new("forbidden", message),
    )
}

async fn acting_org(parts: &Parts, ctx: &AppContext) -> Option<organisations::Model> {
    let jar = cookie::CookieJar::from_headers(&parts.headers);
    let id = jar.get(ACTING_ORG_COOKIE)?.value().parse::<i64>().ok()?;
    organisations::Model::find_by_id(&ctx.db, id).await.ok()
}

fn render(parts: &Parts, status: StatusCode, template: &str, data: serde_json::Value) -> Response {
    let Some(ViewEngine(view)) = parts.extensions.get::<ViewEngine<TeraView>>().cloned() else {
        return Error::string("view engine missing").into_response();
    };
    match view.render(template, data) {
        Ok(html) => (status, axum::response::Html(html)).into_response(),
        Err(err) => err.into_response(),
    }
}
