//! The single gate for tenant pages. A handler that takes `CurrentMember` only
//! runs for an approved member of an organisation (or the platform super admin
//! acting inside one), and `member.org.id` is the only tenant id it should use.
use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts, HeaderMap, HeaderValue, Method, StatusCode},
};
use loco_rs::prelude::*;

use super::{
    current_user::{CurrentUser, SessionId},
    session::redirect_response,
};
use crate::{
    initializers::current_org::{OrgClaim, StaleCurrentOrg},
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

/// The header HTMX and the raw `fetch` calls use to say which organisation
/// their page was drawn for.
pub const ORG_HEADER: &str = "x-org-id";

/// The hidden field `app.ts` adds to plain forms, for the same claim.
pub const ORG_FIELD: &str = "_org";

/// Set on a refusal when the tab's org is one this user was removed from,
/// so `app.ts` shows the removal banner instead of the switch one.
pub const ORG_GONE_HEADER: &str = "x-org-gone";

/// Shown when the Claude Allow page is posted in another organisation.
pub const SWITCHED_ORG: &str =
    "You switched organisation while this page was open. Start again from Claude.";

/// Shown when the personal token form is posted in another organisation.
pub const SWITCHED_ORG_TOKEN: &str = "You switched organisation while this page was open. Check the organisation, then create the token again.";

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
    /// Whether the layout shows the organisation switcher: always for a
    /// member, and while acting only with an org of their own. Only worked
    /// out for full page loads; `false` on HTMX requests and posts.
    pub org_menu: bool,
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
            org_menu: false,
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
            // Its own key, not `org.id`: the join and waiting pages pass an
            // `org` of their own, and must not carry a claim.
            "current_org_id": self.org.id,
            "org_menu": self.org_menu,
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

/// Picks the organisation (see [`PinnedMember`]), then checks the tab's claim
/// against it (see [`check_claim`]).
impl FromRequestParts<AppContext> for CurrentMember {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        ctx: &AppContext,
    ) -> std::result::Result<Self, Self::Rejection> {
        let PinnedMember(member) = PinnedMember::from_request_parts(parts, ctx).await?;
        if let Err(claim) = check_claim(parts, &member) {
            // Only picks the banner's words; the refusal is already decided.
            let gone = removed_from(ctx, claim, member.user.id).await;
            return Err(stale_tab_refusal(&parts.headers, claim, gone));
        }
        Ok(member)
    }
}

/// Whether the claimed org is one this user was removed from. The user id is
/// always the resolved member's, never one from the request, so a forged
/// claim can only name an org the person was removed from themselves. An
/// error counts as no.
async fn removed_from(ctx: &AppContext, claim: OrgClaim, user_id: i64) -> bool {
    let OrgClaim::Org(org_id) = claim else {
        return false;
    };
    memberships::Model::in_org_for_user(&ctx.db, org_id, user_id)
        .await
        .is_ok_and(|m| m.is_some_and(|m| m.is_removed()))
}

/// [`CurrentMember`] without the claim check. Only for forms that pin their
/// org with a hidden `org_id` and call `shown_for_this_org` themselves, so
/// they can answer a stale tab better than the generic refusal: the Claude
/// Allow page and the personal token form. Every use is listed in the route
/// walk test (`tests/requests/stale_tab.rs`); a third needs a deliberate
/// change to that list.
pub struct PinnedMember(pub CurrentMember);

/// Picks the organisation in this order: a super admin's `acting_org`, then a
/// `current_org` the user is active in, then their default membership. On a
/// full page load it also works out whether the layout shows the switcher
/// (see [`CurrentMember::org_menu`]).
impl FromRequestParts<AppContext> for PinnedMember {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        ctx: &AppContext,
    ) -> std::result::Result<Self, Self::Rejection> {
        let mut member = resolve_org(parts, ctx).await.map_err(|refused| *refused)?;
        if is_safe(&parts.method) && !is_htmx(&parts.headers) {
            // A member always has the switcher, if only for "New
            // organisation". Acting, it is about their way back, so it needs
            // an org of their own: the only case that costs a query.
            member.org_menu = if member.acting {
                let (active, pending) =
                    memberships::Model::switcher_counts(&ctx.db, member.user.id)
                        .await
                        .map_err(|e| Error::from(e).into_response())?;
                active + pending >= 1
            } else {
                true
            };
        }
        Ok(Self(member))
    }
}

/// The member, or the page or redirect that stands in for one (boxed, as
/// it is large).
async fn resolve_org(
    parts: &mut Parts,
    ctx: &AppContext,
) -> std::result::Result<CurrentMember, Box<Response>> {
    let CurrentUser(user) = CurrentUser::from_request_parts(parts, ctx).await?;
    let session_id = parts.extensions.get::<SessionId>().map(|sid| sid.0.clone());

    if user.is_super_admin {
        if let Some(org) = acting_org(parts, ctx).await {
            return Ok(CurrentMember {
                username: user.name.clone(),
                user,
                org,
                role: role::OWNER.to_string(),
                acting: true,
                via_session: true,
                session_id,
                org_menu: false,
            });
        }
    }

    let chosen = chosen_org(parts, ctx, user.id)
        .await
        .map_err(|e| e.into_response())?;
    if let Some((org, membership)) = chosen {
        return Ok(CurrentMember {
            via_session: true,
            session_id,
            ..CurrentMember::from_membership(user, org, membership)
        });
    }

    let membership = memberships::Model::default_for_user(&ctx.db, user.id)
        .await
        .map_err(|e| Error::from(e).into_response())?;
    let Some(membership) = membership else {
        if user.is_super_admin {
            return Err(Box::new(redirect_response(&parts.headers, "/admin")));
        }
        if let Some(home) = no_active_org(parts, ctx, user.id).await {
            return Err(Box::new(home));
        }
        return Err(Box::new(render(
            parts,
            StatusCode::FORBIDDEN,
            "tenancy/no_org.html",
            serde_json::json!({}),
        )));
    };
    let org = organisations::Model::find_by_id(&ctx.db, membership.organisation_id)
        .await
        .map_err(|e| Error::from(e).into_response())?;

    if membership.is_active() {
        return Ok(CurrentMember {
            via_session: true,
            session_id,
            ..CurrentMember::from_membership(user, org, membership)
        });
    }
    if let Some(home) = no_active_org(parts, ctx, user.id).await {
        return Err(Box::new(home));
    }
    let template = if membership.is_pending() {
        "tenancy/waiting.html"
    } else if membership.is_removed() {
        "tenancy/removed.html"
    } else {
        "tenancy/declined.html"
    };
    Err(Box::new(render(
        parts,
        StatusCode::FORBIDDEN,
        template,
        serde_json::json!({
            "org": { "name": org.name },
            "username": membership.username,
        }),
    )))
}

/// Refuses a request from a tab drawn for another organisation, or one that
/// does not say (fails closed), with the claim it refused. Plain page loads
/// are never checked, so links from emails, bookmarks and downloads keep
/// working. A pure comparison: no database call decides it.
fn check_claim(parts: &Parts, member: &CurrentMember) -> std::result::Result<(), OrgClaim> {
    let safe = is_safe(&parts.method);
    if safe && !is_htmx(&parts.headers) {
        return Ok(());
    }
    let claim = parts
        .extensions
        .get::<OrgClaim>()
        .copied()
        .unwrap_or(OrgClaim::Missing);
    let allowed = match claim {
        OrgClaim::Org(id) => id == member.org.id,
        // Tabs drawn before the claim existed keep polling.
        OrgClaim::Missing => safe,
        OrgClaim::Garbage => false,
    };
    if allowed {
        return Ok(());
    }
    // Only numbers are logged, as in the switch log.
    let claimed = match claim {
        OrgClaim::Org(id) => id.to_string(),
        OrgClaim::Missing => "missing".to_string(),
        OrgClaim::Garbage => "garbage".to_string(),
    };
    tracing::info!(
        user_id = member.user.id,
        claimed_org = %claimed,
        resolved_org = member.org.id,
        method = %parts.method,
        "refused a request from a tab drawn for another organisation"
    );
    Err(claim)
}

/// Sends a stale tab to Home in the org it is really in, which names the org
/// the tab was showing. Only a parsed `i64` from the request goes into the
/// address, so it can't be turned into an open redirect. `gone` adds
/// `X-Org-Gone` (see [`removed_from`]).
fn stale_tab_refusal(headers: &HeaderMap, claim: OrgClaim, gone: bool) -> Response {
    let to = match claim {
        OrgClaim::Org(id) => format!("/dashboard?from={id}"),
        OrgClaim::Missing | OrgClaim::Garbage => "/dashboard?from=".to_string(),
    };
    // Raw `fetch` calls send the org header but not HX-Request, and need the
    // same answer: a redirect they can see instead of one they follow.
    let mut response = if wants_hx_redirect(headers) {
        (StatusCode::CONFLICT, [("HX-Redirect", to)]).into_response()
    } else {
        (StatusCode::SEE_OTHER, [(header::LOCATION, to)]).into_response()
    };
    no_store(&mut response, gone);
    response
}

/// For HTMX and `fetch` calls when the user has no active organisation left:
/// a 409 to Home, which then shows the waiting, removed or declined page.
/// Without it, an open tab whose last org was removed gets 403s that HTMX
/// doesn't show, and nothing on screen changes. `None` for plain page loads
/// and forms, which get that page straight away.
async fn no_active_org(parts: &Parts, ctx: &AppContext, user_id: i64) -> Option<Response> {
    if !wants_hx_redirect(&parts.headers) {
        return None;
    }
    let claim = parts
        .extensions
        .get::<OrgClaim>()
        .copied()
        .unwrap_or(OrgClaim::Missing);
    let gone = removed_from(ctx, claim, user_id).await;
    let mut response = (StatusCode::CONFLICT, [("HX-Redirect", "/dashboard")]).into_response();
    no_store(&mut response, gone);
    Some(response)
}

/// HTMX, or a raw `fetch` call (it sends the org header but not HX-Request).
fn wants_hx_redirect(headers: &HeaderMap) -> bool {
    is_htmx(headers) || headers.contains_key(ORG_HEADER)
}

fn no_store(response: &mut Response, gone: bool) {
    let h = response.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if gone {
        h.insert(ORG_GONE_HEADER, HeaderValue::from_static("1"));
    }
}

fn is_safe(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn is_htmx(headers: &HeaderMap) -> bool {
    headers.get("hx-request").is_some_and(|v| v == "true")
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
