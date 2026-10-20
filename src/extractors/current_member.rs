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
    initializers::{
        current_org::{OrgClaim, OrgCookie},
        org_path::PathOrg,
    },
    models::{
        memberships::{self, role},
        organisations, users,
    },
    views::{
        layout::avatar_color,
        links::{is_slug, old_link_item, org_base, org_path, Item},
    },
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

/// Set on a refusal when the tab's org is one this user no longer has (see
/// [`removed_from`]), so `app.ts` shows the removal banner instead of the
/// switch one.
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
    /// Whether the person has two or more organisations to tell apart, for
    /// the phone org line. Page loads only, like `org_menu`.
    pub multi_org: bool,
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
            multi_org: false,
        }
    }

    /// Whether a form's hidden `org_id` names this member's organisation.
    /// Forms that grant Claude access pin the org they were shown for, so a
    /// switch in another tab can't bind a token to a different one.
    #[must_use]
    pub fn shown_for_this_org(&self, org_id: Option<&str>) -> bool {
        org_id.and_then(|id| id.parse::<i64>().ok()) == Some(self.org.id)
    }

    /// The person approving or declining a request, for
    /// `memberships::Model::approve` and `reject`.
    #[must_use]
    pub fn approver(&self) -> memberships::Approver {
        memberships::Approver {
            user_id: self.user.id,
            acting: self.acting,
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

    /// `path` inside this member's organisation: `/tasks` gives
    /// `/o/{slug}/tasks`. Every app link and redirect goes through it.
    #[must_use]
    pub fn url(&self, path: &str) -> String {
        org_path(&self.org.slug, path)
    }

    /// `org_base` for templates' `org_url`, added to a fragment's data. For
    /// HTMX fragments and write responses that don't use [`Self::page`].
    #[must_use]
    pub fn fragment(&self, mut data: serde_json::Value) -> serde_json::Value {
        if let Some(map) = data.as_object_mut() {
            map.insert("org_base".to_string(), org_base(&self.org.slug).into());
        }
        data
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
            "org_base": org_base(&self.org.slug),
            "org_menu": self.org_menu,
            "multi_org": self.multi_org,
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
            let home = if parts.extensions.get::<PathOrg>().is_some() {
                member.url("/dashboard")
            } else {
                "/dashboard".to_string()
            };
            return Err(stale_tab_refusal(&parts.headers, &home, claim, gone));
        }
        Ok(member)
    }
}

/// Whether the claimed org is one this user no longer has: their membership
/// there is removed, declined, or pending again after asking to rejoin. The
/// user id is always the resolved member's, never one from the request, so
/// a forged claim can only name an org the person has a row in themselves.
/// An error counts as no.
async fn removed_from(ctx: &AppContext, claim: OrgClaim, user_id: i64) -> bool {
    let OrgClaim::Org(org_id) = claim else {
        return false;
    };
    memberships::Model::in_org_for_user(&ctx.db, org_id, user_id)
        .await
        .is_ok_and(|m| m.is_some_and(|m| !m.is_active()))
}

/// [`CurrentMember`] without the claim check. Only for forms that pin their
/// org with a hidden `org_id` and call `shown_for_this_org` themselves, so
/// they can answer a stale tab better than the generic refusal. Its three
/// uses: the Claude Allow page (GET and POST, bare, in the last org) and
/// the personal token form (under `/o/{slug}/`). `not_found_page` calls it
/// too, never as a handler. Every use is listed in the route walk test
/// (`tests/requests/stale_tab.rs`); another needs a deliberate change to
/// that list.
pub struct PinnedMember(pub CurrentMember);

/// Picks the organisation: the address's (see [`in_address_org`]), else the
/// last one (see [`last_org`]). On a full page load it also works out
/// whether the layout shows the switcher and the phone org line (see
/// [`CurrentMember::org_menu`]).
impl FromRequestParts<AppContext> for PinnedMember {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        ctx: &AppContext,
    ) -> std::result::Result<Self, Self::Rejection> {
        let mut member = resolve_org(parts, ctx).await.map_err(|refused| *refused)?;
        if is_safe(&parts.method) && !is_htmx(&parts.headers) {
            let (active, pending) = memberships::Model::switcher_counts(&ctx.db, member.user.id)
                .await
                .map_err(|e| Error::from(e).into_response())?;
            // A member always has the switcher, if only for "New
            // organisation". Acting, it is about their way back, so it needs
            // an org of their own. The acting org counts as one more.
            if member.acting {
                member.org_menu = active + pending >= 1;
                member.multi_org = active >= 1;
            } else {
                member.org_menu = true;
                member.multi_org = active >= 2;
            }
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
    if let Some(PathOrg { slug }) = parts.extensions.get::<PathOrg>().cloned() {
        return in_address_org(parts, ctx, user, session_id, &slug).await;
    }
    // The Claude Allow page stays bare, in the last org, until PR 3's picker.
    let allow_page = parts.uri.path() == "/oauth/authorize"
        && matches!(parts.method, Method::GET | Method::POST);
    if allow_page {
        return last_org(parts, ctx, user, session_id).await;
    }
    Err(Box::new(bare_address(parts, ctx, user, session_id).await))
}

/// A tenant route without `/o/{slug}/`: an old bookmark, email or link, or a
/// tab drawn before the deploy. A page load is redirected into an org; any
/// other request is refused and never replayed, so a write can't land in the
/// last-used org.
async fn bare_address(
    parts: &Parts,
    ctx: &AppContext,
    user: users::Model,
    session_id: Option<String>,
) -> Response {
    if is_page_load(parts) {
        bare_page_load(parts, ctx, user, session_id).await
    } else {
        refuse_bare(parts, ctx, user, session_id).await
    }
}

/// 303 to the same path and query under `/o/{slug}`: the item's org when the
/// address names an item in an org you're active (or acting) in, else the
/// last org. Built from `uri.path()` and `uri.query()` behind `/o/{slug}/`,
/// so it is always on this site.
async fn bare_page_load(
    parts: &Parts,
    ctx: &AppContext,
    user: users::Model,
    session_id: Option<String>,
) -> Response {
    let last = match last_org(parts, ctx, user, session_id).await {
        Ok(last) => last,
        // No org at all: the page that says so, as before.
        Err(page) => return *page,
    };
    let mut slug = last.org.slug.clone();
    if let Some((item, id)) = old_link_item(parts.uri.path(), parts.uri.query()) {
        let acting = last.acting.then_some(last.org.id);
        match item_org_for(ctx, item, id, last.user.id, acting).await {
            Ok(Some(found)) => slug = found,
            Ok(None) => {}
            Err(err) => return Error::from(err).into_response(),
        }
    }
    let path_and_query = parts
        .uri
        .path_and_query()
        .map_or("/dashboard", |pq| pq.as_str());
    let Some(to) = into_org(&slug, path_and_query) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let Ok(location) = HeaderValue::from_str(&to) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let mut response = (StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response();
    no_store(&mut response, false);
    response
}

/// `path` under `/o/{slug}` for a redirect. `None` for a slug in the
/// database that isn't one, which the caller answers with a 500: `org_path`
/// would fall back to a bare `/dashboard`, which redirects back here for
/// ever.
fn into_org(slug: &str, path: &str) -> Option<String> {
    if is_slug(slug) {
        Some(org_path(slug, path))
    } else {
        tracing::error!("an organisation's slug is not a slug; refusing to redirect");
        None
    }
}

/// The org a bare link's item is in, only where this user is active, or the
/// org a super admin is acting in. One query, the same whether the id is
/// missing or in someone else's org, so it never tells them apart. The table
/// comes from [`Item`], never from the request.
async fn item_org_for(
    ctx: &AppContext,
    item: Item,
    id: i64,
    user_id: i64,
    acting_org: Option<i64>,
) -> std::result::Result<Option<String>, sea_orm::DbErr> {
    use sea_orm::{ConnectionTrait, Statement};
    let table = item.table();
    let sql = format!(
        "SELECT o.slug FROM {table} t \
         JOIN memberships m ON m.organisation_id = t.organisation_id \
           AND m.user_id = $2 AND m.status = '{active}' \
         JOIN organisations o ON o.id = t.organisation_id \
         WHERE t.id = $1 \
         UNION ALL \
         SELECT o.slug FROM {table} t \
         JOIN organisations o ON o.id = t.organisation_id \
         WHERE t.id = $1 AND t.organisation_id = $3 \
         LIMIT 1",
        active = memberships::status::ACTIVE,
    );
    let row = ctx
        .db
        .query_one_raw(Statement::from_sql_and_values(
            ctx.db.get_database_backend(),
            sql,
            [id.into(), user_id.into(), acting_org.into()],
        ))
        .await?;
    row.map(|row| row.try_get::<String>("", "slug")).transpose()
}

/// Refuses a bare request that isn't a page load: 409 with `HX-Redirect` for
/// HTMX and `fetch`, 303 for a plain form, never 307 or 308 (which repeat the
/// body). It lands on Home in the org the tab was showing when you're still
/// active there, else in the last org; a claim for an org your row was
/// removed from (or is pending or declined in) gets `?from=` and
/// `X-Org-Gone`, for PR 5's notice. Only a slug from the database and a
/// parsed `i64` go into the address.
async fn refuse_bare(
    parts: &Parts,
    ctx: &AppContext,
    user: users::Model,
    session_id: Option<String>,
) -> Response {
    let claim = parts
        .extensions
        .get::<OrgClaim>()
        .copied()
        .unwrap_or(OrgClaim::Missing);
    let user_id = user.id;
    let mut showing = None;
    let mut gone = None;
    if let OrgClaim::Org(id) = claim {
        match memberships::Model::in_org_for_user(&ctx.db, id, user_id).await {
            Ok(Some(m)) if m.is_active() => {
                showing = organisations::Model::find_by_id(&ctx.db, id).await.ok();
            }
            Ok(Some(_)) => gone = Some(id),
            _ => {}
        }
    }
    let home = match gone {
        Some(id) => format!("/dashboard?from={id}"),
        None => "/dashboard?from=".to_string(),
    };
    let to = match showing {
        Some(org) => into_org(&org.slug, &home),
        None => match last_org(parts, ctx, user, session_id).await {
            Ok(last) => into_org(&last.org.slug, &home),
            // No org at all: the answer that says so (`/admin` for a super
            // admin, else the waiting or no-org page), as before.
            Err(answer) => {
                let mut answer = *answer;
                no_store(&mut answer, gone.is_some());
                return answer;
            }
        },
    };
    let Some(to) = to else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    tracing::info!(
        user_id,
        claimed_org = match claim {
            OrgClaim::Org(id) => id.to_string(),
            OrgClaim::Missing => "missing".to_string(),
            OrgClaim::Garbage => "garbage".to_string(),
        },
        method = %parts.method,
        "refused a request without an organisation in its address"
    );
    let mut response = if wants_hx_redirect(&parts.headers) {
        (StatusCode::CONFLICT, [("HX-Redirect", to)]).into_response()
    } else {
        (StatusCode::SEE_OTHER, [(header::LOCATION, to)]).into_response()
    };
    no_store(&mut response, gone.is_some());
    response
}

/// The organisation in the address, `/o/{slug}/...`, checked on every
/// request: a super admin acting there, else the person's own membership.
/// No row and no such org give the same answer from the same query.
async fn in_address_org(
    parts: &Parts,
    ctx: &AppContext,
    user: users::Model,
    session_id: Option<String>,
    slug: &str,
) -> std::result::Result<CurrentMember, Box<Response>> {
    if user.is_super_admin {
        if let Some(org) = acting_org(parts, ctx).await.filter(|org| org.slug == slug) {
            return Ok(acting_member(user, org, session_id));
        }
    }
    let found = memberships::Model::in_org_by_slug(&ctx.db, slug, user.id)
        .await
        .map_err(|e| Box::new(Error::from(e).into_response()))?;
    let Some((membership, org)) = found else {
        return Err(Box::new(not_here(&parts.headers)));
    };
    if membership.is_active() {
        remember(parts, org.id);
        return Ok(CurrentMember {
            via_session: true,
            session_id,
            ..CurrentMember::from_membership(user, org, membership)
        });
    }
    // A super admin with no active membership here has /admin, never the
    // waiting, removed or declined page.
    if user.is_super_admin {
        return Err(Box::new(redirect_response(&parts.headers, "/admin")));
    }
    if wants_hx_redirect(&parts.headers) {
        let to = org_path(&org.slug, "/dashboard");
        let mut response = (StatusCode::CONFLICT, [("HX-Redirect", to)]).into_response();
        no_store(&mut response, true);
        return Err(Box::new(response));
    }
    Err(Box::new(status_page(parts, ctx, &membership, &org).await))
}

/// Not a member, or no such organisation. A page load gets `not_found_page`'s
/// plain page; HTMX and `fetch` go to Home in the last org.
fn not_here(headers: &HeaderMap) -> Response {
    if wants_hx_redirect(headers) {
        let mut response = (StatusCode::CONFLICT, [("HX-Redirect", "/dashboard")]).into_response();
        no_store(&mut response, false);
        return response;
    }
    Error::NotFound.into_response()
}

/// Moves `current_org` to `org_id` on a page load the person started on this
/// site or by hand (`Sec-Fetch-Site` is `same-origin` or `none`, or missing).
/// A navigation another site started never moves it, nor do HTMX, `fetch`,
/// writes or acting (the caller never asks for acting).
fn remember(parts: &Parts, org_id: i64) {
    // A browser says what a request is for; only a document moves it.
    let document = parts
        .headers
        .get("sec-fetch-dest")
        .is_none_or(|dest| dest == "document");
    if !is_page_load(parts) || !document {
        return;
    }
    let ours = match parts.headers.get("sec-fetch-site") {
        None => true,
        Some(site) => site == "same-origin" || site == "none",
    };
    let already = cookie::CookieJar::from_headers(&parts.headers)
        .get(CURRENT_ORG_COOKIE)
        .is_some_and(|c| c.value() == org_id.to_string());
    if ours && !already {
        if let Some(slot) = parts.extensions.get::<OrgCookie>() {
            slot.remember(org_id);
        }
    }
}

/// The waiting, removed or declined page for this org (403), with the
/// person's other active organisations (§3): the last used first, then A to
/// Z. One query, only on this path.
async fn status_page(
    parts: &Parts,
    ctx: &AppContext,
    membership: &memberships::Model,
    org: &organisations::Model,
) -> Response {
    let template = if membership.is_pending() {
        "tenancy/waiting.html"
    } else if membership.is_removed() {
        "tenancy/removed.html"
    } else {
        "tenancy/declined.html"
    };
    let last = cookie::CookieJar::from_headers(&parts.headers)
        .get(CURRENT_ORG_COOKIE)
        .and_then(|c| c.value().parse::<i64>().ok());
    let mut others: Vec<_> =
        match memberships::Model::for_user_with_orgs(&ctx.db, membership.user_id).await {
            Ok(rows) => rows.into_iter().filter(|(m, _)| m.is_active()).collect(),
            Err(err) => return Error::from(err).into_response(),
        };
    let last_used = others.iter().position(|(_, o)| Some(o.id) == last);
    if let Some(at) = last_used {
        let row = others.remove(at);
        others.insert(0, row);
    }
    let others: Vec<_> = others
        .iter()
        .enumerate()
        .map(|(i, (m, o))| {
            serde_json::json!({
                "name": o.name,
                "url": org_path(&o.slug, "/dashboard"),
                "role": m.role,
                "last": i == 0 && last_used.is_some(),
            })
        })
        .collect();
    // The same address again, which starts with `/o/{slug}/`.
    let check_again = parts
        .extensions
        .get::<axum::extract::OriginalUri>()
        .and_then(|uri| uri.0.path_and_query().map(ToString::to_string))
        .filter(|to| to.starts_with(&format!("/o/{}/", org.slug)))
        .unwrap_or_else(|| org_path(&org.slug, "/dashboard"));
    render(
        parts,
        StatusCode::FORBIDDEN,
        template,
        serde_json::json!({
            "org": { "name": org.name },
            "username": membership.username,
            "others": others,
            "check_again": check_again,
        }),
    )
}

fn acting_member(
    user: users::Model,
    org: organisations::Model,
    session_id: Option<String>,
) -> CurrentMember {
    CurrentMember {
        username: user.name.clone(),
        user,
        org,
        role: role::OWNER.to_string(),
        acting: true,
        via_session: true,
        session_id,
        org_menu: false,
        multi_org: false,
    }
}

/// The last organisation, for addresses without one: a super admin's
/// `acting_org`, then a `current_org` the user is active in, then their
/// default membership in any status.
async fn last_org(
    parts: &Parts,
    ctx: &AppContext,
    user: users::Model,
    session_id: Option<String>,
) -> std::result::Result<CurrentMember, Box<Response>> {
    if user.is_super_admin {
        if let Some(org) = acting_org(parts, ctx).await {
            return Ok(acting_member(user, org, session_id));
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
    // A super admin with no active membership has /admin, never the
    // waiting, removed or declined page, even when they asked to join.
    if user.is_super_admin
        && !membership
            .as_ref()
            .is_some_and(memberships::Model::is_active)
    {
        return Err(Box::new(redirect_response(&parts.headers, "/admin")));
    }
    let Some(membership) = membership else {
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
    // No active org anywhere, so no other organisations to offer.
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

/// Sends a stale tab to `home` (Home in the org it resolved), with the org
/// the tab was showing in `?from=`. Only a parsed `i64` from the request goes
/// into the address, so it can't be turned into an open redirect. `gone`
/// adds `X-Org-Gone` (see [`removed_from`]).
fn stale_tab_refusal(headers: &HeaderMap, home: &str, claim: OrgClaim, gone: bool) -> Response {
    let to = match claim {
        OrgClaim::Org(id) => format!("{home}?from={id}"),
        OrgClaim::Missing | OrgClaim::Garbage => format!("{home}?from="),
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

/// A browser loading a page: GET or HEAD, not HTMX or `fetch`, not the
/// socket upgrade, and not asking for the notification stream.
fn is_page_load(parts: &Parts) -> bool {
    matches!(parts.method, Method::GET | Method::HEAD)
        && !wants_hx_redirect(&parts.headers)
        && !parts.headers.contains_key(header::UPGRADE)
        && !parts
            .headers
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|accept| accept.contains("text/event-stream"))
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
/// user is still an active member there. A cookie that fails the check is
/// cleared (see [`OrgCookie`]).
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
        if let Some(slot) = parts.extensions.get::<OrgCookie>() {
            slot.clear();
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
