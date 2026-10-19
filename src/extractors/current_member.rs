//! The single gate for tenant pages. A handler that takes `CurrentMember` only
//! runs for an approved member of an organisation (or the platform super admin
//! acting inside one), and `member.org.id` is the only tenant id it should use.
use axum::{extract::FromRequestParts, http::request::Parts, http::StatusCode};
use loco_rs::prelude::*;

use super::{
    current_user::{CurrentUser, SessionId},
    session::redirect_response,
};
use crate::{
    initializers::current_org::StaleCurrentOrg,
    models::{
        memberships::{self, role},
        organisations, users,
    },
    views::layout::avatar_color,
};

/// Cookie a super admin sets to work inside another organisation.
pub const ACTING_ORG_COOKIE: &str = "acting_org";

/// Cookie that says which organisation this browser is working in. Only a
/// hint: every request checks the membership again.
pub const CURRENT_ORG_COOKIE: &str = "current_org";

/// Shown when a form pinned to one organisation is posted in another.
pub const SWITCHED_ORG: &str =
    "You switched organisation while this page was open. Start again from Claude.";

#[derive(Clone)]
pub struct CurrentMember {
    pub user: users::Model,
    pub org: organisations::Model,
    pub role: String,
    /// The username shown inside this organisation.
    pub username: String,
    /// True when a super admin is inside an organisation they do not belong to.
    pub acting: bool,
    /// True for the browser's session cookie, false for Claude's bearer
    /// tokens. Only sessions can see login passwords.
    pub via_session: bool,
    /// The session's sign-in id. `None` for bearer tokens, and for cookies
    /// issued before sign-ins had ids.
    pub session_id: Option<String>,
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
            via_session: false,
            session_id: None,
        }
    }

    /// Whether a form's hidden `org_id` names this member's organisation.
    /// Forms that grant Claude access pin the org they were shown for, so a
    /// switch in another tab can't bind a token to a different one.
    #[must_use]
    pub fn shown_for_this_org(&self, org_id: Option<&str>) -> bool {
        org_id.and_then(|id| id.parse::<i64>().ok()) == Some(self.org.id)
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

    /// # Errors
    /// 403 when a super admin is acting inside an organisation they do not
    /// belong to. Documents and removing members need their own membership.
    pub fn require_own_membership(&self) -> Result<()> {
        if self.acting {
            Err(forbidden(
                "Leave admin mode and use your own membership to do that.",
            ))
        } else {
            Ok(())
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

/// Picks the organisation in this order: a super admin's `acting_org`, then a
/// `current_org` the user is active in, then their default membership.
impl FromRequestParts<AppContext> for CurrentMember {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        ctx: &AppContext,
    ) -> std::result::Result<Self, Self::Rejection> {
        let CurrentUser(user) = CurrentUser::from_request_parts(parts, ctx).await?;
        let session_id = parts.extensions.get::<SessionId>().map(|sid| sid.0.clone());

        if user.is_super_admin {
            if let Some(org) = acting_org(parts, ctx).await {
                return Ok(Self {
                    username: user.name.clone(),
                    user,
                    org,
                    role: role::OWNER.to_string(),
                    acting: true,
                    via_session: true,
                    session_id,
                });
            }
        }

        let chosen = chosen_org(parts, ctx, user.id)
            .await
            .map_err(|e| e.into_response())?;
        if let Some((org, membership)) = chosen {
            return Ok(Self {
                via_session: true,
                session_id,
                ..Self::from_membership(user, org, membership)
            });
        }

        let membership = memberships::Model::default_for_user(&ctx.db, user.id)
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
            return Ok(Self {
                via_session: true,
                session_id,
                ..Self::from_membership(user, org, membership)
            });
        }
        let template = if membership.is_pending() {
            "tenancy/waiting.html"
        } else if membership.is_removed() {
            "tenancy/removed.html"
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

/// A 403 with a message for the person.
#[must_use]
pub fn forbidden(message: &str) -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new("forbidden", message),
    )
}

/// The organisation this browser chose with the `current_org` cookie, if the
/// user is still an active member there. A cookie that fails the check raises
/// [`StaleCurrentOrg`], so the response clears it.
async fn chosen_org(
    parts: &Parts,
    ctx: &AppContext,
    user_id: i64,
) -> Result<Option<(organisations::Model, memberships::Model)>> {
    let jar = cookie::CookieJar::from_headers(&parts.headers);
    let Some(cookie) = jar.get(CURRENT_ORG_COOKIE) else {
        return Ok(None);
    };
    let membership = match cookie.value().parse::<i64>() {
        Ok(org_id) => memberships::Model::active_in(&ctx.db, org_id, user_id).await?,
        Err(_) => None,
    };
    let Some(membership) = membership else {
        if let Some(flag) = parts.extensions.get::<StaleCurrentOrg>() {
            flag.raise();
        }
        return Ok(None);
    };
    let org = organisations::Model::find_by_id(&ctx.db, membership.organisation_id).await?;
    Ok(Some((org, membership)))
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
