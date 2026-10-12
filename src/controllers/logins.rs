//! Logins in Documents: add, read, change and delete them, unlock with the
//! app password, then show or copy a secret. Every route goes through
//! `CurrentMember` and refuses a super admin acting inside the organisation.
//! Secrets are shown only to a browser session that may see them and
//! re-entered the app password; each one is logged before it is decrypted.
use std::collections::HashMap;

use axum::{
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware,
};
use loco_rs::prelude::*;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

use super::documents::{folder_rows, no_framing};
use crate::{
    extractors::{
        current_member::{forbidden, CurrentMember},
        session::{append_cookie, redirect_response, session_cookie_for},
    },
    mailers::security::SecurityMailer,
    models::{
        auth_attempts::{self, ReauthRefusal, REAUTH_LIMIT},
        folders, login_accesses,
        login_events::{self, action, Client, Entry},
        logins::{self, field, LoginParams},
        memberships::{self, role},
        secrets::crypto::Kek,
        users,
    },
    views::{
        forms::{field_errors, invalid_form},
        layout::avatar_color,
        time,
    },
};

/// Someone in the "Who can see the password" picker.
#[derive(Debug, Serialize)]
struct Pick {
    id: i64,
    username: String,
    color: &'static str,
    /// Owners and admins always can; their chip cannot be turned off.
    fixed: bool,
    checked: bool,
    is_me: bool,
}

/// The picker for a login: every active member, owners and admins first.
async fn picker(ctx: &AppContext, member: &CurrentMember, named: &[i64]) -> Result<Vec<Pick>> {
    let mut people: Vec<Pick> = memberships::Model::list_for_org(&ctx.db, member.org.id)
        .await?
        .into_iter()
        .filter(|(m, _)| m.is_active())
        .map(|(m, _)| {
            let fixed = m.role == role::OWNER || m.role == role::ADMIN;
            Pick {
                id: m.user_id,
                color: avatar_color(&m.username),
                fixed,
                checked: !fixed && named.contains(&m.user_id),
                is_me: m.user_id == member.user.id,
                username: m.username,
            }
        })
        .collect();
    people.sort_by_key(|p| !p.fixed);
    Ok(people)
}

/// What `documents/_login_form.html` needs to add a login to `folder`.
///
/// # Errors
/// On database errors.
pub async fn new_login_form(
    ctx: &AppContext,
    member: &CurrentMember,
    folder: &folders::Model,
) -> Result<serde_json::Value> {
    Ok(data!({
        "form_id": "new-login-form",
        "action": format!("/documents/folders/{}/logins", folder.id),
        "editing": false,
        "values": { "title": "", "username": "", "url": "", "notes": "" },
        "people": picker(ctx, member, &[]).await?,
        "errors": {},
        "submit": "Save login",
    }))
}

/// "RajG007, Shusma and Kreepa".
fn and_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Everything the login page, its side sheet and its edit form show.
async fn login_data(
    ctx: &AppContext,
    member: &CurrentMember,
    login: &logins::Model,
) -> Result<serde_json::Value> {
    let all = memberships::Model::list_for_org(&ctx.db, member.org.id).await?;
    let names: HashMap<i64, String> = all
        .iter()
        .map(|(m, _)| (m.user_id, m.username.clone()))
        .collect();
    let active_with = |roles: &[&str]| -> Vec<String> {
        all.iter()
            .filter(|(m, _)| m.is_active() && roles.contains(&m.role.as_str()))
            .map(|(m, _)| m.username.clone())
            .collect()
    };
    let managers = active_with(&[role::OWNER, role::ADMIN]);
    let owners = active_with(&[role::OWNER]);
    let named_ids = login_accesses::Model::user_ids(&ctx.db, login.id).await?;
    let named: Vec<String> = all
        .iter()
        .filter(|(m, _)| m.is_active() && m.role == role::MEMBER && named_ids.contains(&m.user_id))
        .map(|(m, _)| m.username.clone())
        .collect();
    let creator = login.created_by_id.and_then(|id| names.get(&id).cloned());
    // Who to ask: the person who added it, then an owner.
    let mut ask: Vec<String> = creator.iter().cloned().collect();
    ask.extend(
        owners
            .iter()
            .filter(|o| Some(*o) != creator.as_ref())
            .cloned(),
    );
    ask.truncate(2);
    let folder = folders::Model::find_in_org(&ctx.db, member.org.id, login.folder_id).await?;
    let can_reveal = login.can_reveal(&ctx.db, member).await?;
    let unlocked = can_reveal && member.user.secrets_unlocked(member.session_id.as_deref());
    let folders_list: Vec<serde_json::Value> = folders::Model::list_for_org(&ctx.db, member.org.id)
        .await?
        .into_iter()
        .map(|s| data!({ "id": s.folder.id, "name": s.folder.name }))
        .collect();
    let added_on = time::local(login.created_at, member.tz())
        .format("%-d %b")
        .to_string();
    Ok(data!({
        "login": {
            "id": login.id,
            "title": login.title,
            "username": login.username,
            "url": login.url,
            "display_url": logins::display_url(&login.url),
            "notes": login.notes,
            "has_recovery": login.has_recovery(),
            "added_by": creator.unwrap_or_else(|| "a former member".to_string()),
            "added_on": added_on,
        },
        "folder": { "id": folder.id, "name": folder.name },
        "can_reveal": can_reveal,
        "unlocked_minutes": if unlocked { member.user.unlocked_minutes_left() } else { 0 },
        "managers": and_list(&managers),
        "named": and_list(&named),
        "ask": ask.join(" or "),
        "owners": and_list(&owners),
        "form": {
            "form_id": format!("login-{}-form", login.id),
            "action": format!("/documents/logins/{}", login.id),
            "editing": true,
            "has_recovery": login.has_recovery(),
            "values": {
                "title": login.title,
                "username": login.username,
                "url": logins::display_url(&login.url),
                "notes": login.notes,
                "folder_id": login.folder_id,
            },
            "folders": folders_list,
            "people": picker(ctx, member, &named_ids).await?,
            "errors": {},
            "submit": "Save changes",
        },
    }))
}

/// A login's page. HTMX requests (the desktop side sheet) get only the
/// details.
#[debug_handler]
async fn show(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response> {
    member.require_own_membership()?;
    let login = logins::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    let mut page = login_data(&ctx, &member, &login).await?;
    if headers.contains_key("hx-request") {
        page["sheet"] = true.into();
        return format::render()
            .header(header::CACHE_CONTROL.as_str(), "no-store")
            .view(&v, "documents/_login.html", member.page("documents", page));
    }
    page["folders"] =
        serde_json::to_value(folder_rows(&ctx, &member, Some(login.folder_id)).await?)?;
    format::render()
        .header(header::CACHE_CONTROL.as_str(), "no-store")
        .view(&v, "documents/login.html", member.page("documents", page))
}

/// The form again, with its errors and what was typed. Secrets are never
/// sent back.
fn login_form_error(
    v: &TeraView,
    member: &CurrentMember,
    mut form: serde_json::Value,
    params: &LoginParams,
    err: ModelError,
) -> Result<Response> {
    let errors = field_errors(&err).ok_or(err)?;
    let form_id = form["form_id"].as_str().unwrap_or_default().to_string();
    form["values"]["title"] = params.title.clone().into();
    form["values"]["username"] = params.username.clone().into();
    form["values"]["url"] = params.url.clone().into();
    form["values"]["notes"] = params.notes.clone().into();
    if let Some(people) = form["people"].as_array_mut() {
        for p in people {
            let id = p["id"].as_i64().unwrap_or_default();
            let fixed = p["fixed"].as_bool().unwrap_or(false);
            p["checked"] = (!fixed && params.access.contains(&id)).into();
        }
    }
    form["errors"] = serde_json::to_value(errors)?;
    invalid_form(
        v,
        "documents/_login_form.html",
        &form_id,
        member.page("documents", data!({ "form": form })),
    )
}

/// Owners and admins add a login to a folder, then see it.
#[debug_handler]
async fn create(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    client: Client,
    Path(folder_id): Path<i64>,
    axum_extra::extract::Form(params): axum_extra::extract::Form<LoginParams>,
) -> Result<Response> {
    member.require_own_membership()?;
    member.require_manager()?;
    let folder = folders::Model::find_in_org(&ctx.db, member.org.id, folder_id).await?;
    match logins::Model::create(&ctx, &folder, &member, &client, &params).await {
        Ok(login) => Ok(redirect_response(
            &headers,
            &format!("/documents/logins/{}", login.id),
        )),
        Err(err) => {
            let form = new_login_form(&ctx, &member, &folder).await?;
            login_form_error(&v, &member, form, &params, err)
        }
    }
}

/// Owners and admins change a login.
#[debug_handler]
async fn edit(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    client: Client,
    Path(id): Path<i64>,
    axum_extra::extract::Form(params): axum_extra::extract::Form<LoginParams>,
) -> Result<Response> {
    member.require_own_membership()?;
    member.require_manager()?;
    let login = logins::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    match login.clone().edit(&ctx, &member, &client, &params).await {
        Ok(login) => Ok(redirect_response(
            &headers,
            &format!("/documents/logins/{}", login.id),
        )),
        Err(err) => {
            let form = login_data(&ctx, &member, &login).await?["form"].take();
            login_form_error(&v, &member, form, &params, err)
        }
    }
}

/// Owners and admins delete a login, then go back to its folder.
#[debug_handler]
async fn delete(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    client: Client,
    Path(id): Path<i64>,
) -> Result<Response> {
    member.require_own_membership()?;
    member.require_manager()?;
    let login = logins::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    let folder_id = login.folder_id;
    login.delete_login(&ctx.db, &member, &client).await?;
    Ok(redirect_response(
        &headers,
        &format!("/documents/folders/{folder_id}"),
    ))
}

#[derive(Deserialize)]
struct UnlockForm {
    password: SecretString,
    /// The login they were trying to see, for the log.
    #[serde(default)]
    login_id: Option<i64>,
}

/// A plain-text answer for the page's script, never cached.
fn plain(status: StatusCode, message: &str) -> Result<Response> {
    format::render()
        .status(status)
        .header(header::CACHE_CONTROL.as_str(), "no-store")
        .text(message)
}

/// Locks the account, logs it, and tells the owners once per lock.
async fn lock(
    ctx: &AppContext,
    member: &CurrentMember,
    client: &Client,
    login: Option<(i64, &str)>,
) -> Result<Response> {
    if users::Model::lock_account(ctx, member.org.id, member.user.id).await? {
        login_events::Model::record(
            &ctx.db,
            client,
            Entry {
                org_id: member.org.id,
                login,
                user: (member.user.id, &member.username),
                action: action::LOCK,
                detail: None,
            },
        )
        .await?;
        if let Err(err) = SecurityMailer::account_locked(ctx, &member.org, &member.username).await {
            tracing::error!(error = %err, "could not email owners about a locked account");
        }
    }
    plain(StatusCode::UNAUTHORIZED, "Your account is locked.")
}

/// Re-enter the app password to see logins for the next few minutes, on
/// this sign-in only. Five wrong tries lock the account until a password
/// reset, and sign it out everywhere.
#[debug_handler]
async fn unlock(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    client: Client,
    Form(form): Form<UnlockForm>,
) -> Result<Response> {
    member.require_own_membership()?;
    if !member.via_session {
        return Err(forbidden("Passwords can only be shown in the app."));
    }
    let login = match form.login_id {
        Some(id) => logins::Model::find_in_org(&ctx.db, member.org.id, id)
            .await
            .ok(),
        None => None,
    };
    let about = login.as_ref().map(|l| (l.id, l.title.as_str()));
    let user = &member.user;
    match auth_attempts::Model::begin_reauth(&ctx.db, &user.email, user.id, &client.ip).await? {
        Some(ReauthRefusal::Locked) => return lock(&ctx, &member, &client, about).await,
        Some(ReauthRefusal::TooManyFromIp) => {
            return plain(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many tries from this network. Wait 15 minutes, then try again.",
            )
        }
        None => {}
    }
    let entry = |action: &'static str, detail: Option<String>| Entry {
        org_id: member.org.id,
        login: about,
        user: (user.id, &member.username),
        action,
        detail,
    };
    if !user.verify_password(form.password.expose_secret()) {
        let failures = auth_attempts::Model::reauth_failures(&ctx.db, &user.email).await?;
        let detail = format!("{} of {REAUTH_LIMIT}", failures.min(REAUTH_LIMIT));
        login_events::Model::record(&ctx.db, &client, entry(action::REAUTH_FAILED, Some(detail)))
            .await?;
        if failures >= REAUTH_LIMIT {
            return lock(&ctx, &member, &client, about).await;
        }
        let left = REAUTH_LIMIT - failures;
        let tries = if left == 1 {
            "1 more try".to_string()
        } else {
            format!("{left} more tries")
        };
        return plain(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("That's not your password. {tries}, then your account is locked."),
        );
    }
    auth_attempts::Model::clear_reauth(&ctx.db, &user.email).await?;
    // A cookie from before sign-ins had ids gets one now, so the unlock is
    // still tied to this browser.
    let (session_id, cookie) = match &member.session_id {
        Some(sid) => (sid.clone(), None),
        None => {
            let sid = uuid::Uuid::new_v4().to_string();
            let cookie = session_cookie_for(&ctx, user, &sid)?;
            (sid, Some(cookie))
        }
    };
    users::Model::unlock_secrets(&ctx.db, user.id, &session_id).await?;
    login_events::Model::record(&ctx.db, &client, entry(action::UNLOCK, None)).await?;
    let mut response = plain(StatusCode::OK, "Unlocked")?;
    if let Some(cookie) = cookie {
        append_cookie(&mut response, &cookie)?;
    }
    Ok(response)
}

#[derive(Deserialize)]
struct SecretForm {
    #[serde(default)]
    field: String,
}

/// The checks before a secret is decrypted, then the log entry. Returns the
/// login and the field, or the answer to send instead.
async fn prepare_secret(
    ctx: &AppContext,
    member: &CurrentMember,
    client: &Client,
    id: i64,
    form: &SecretForm,
    copied: bool,
) -> Result<std::result::Result<(logins::Model, &'static str), Response>> {
    member.require_own_membership()?;
    let login = logins::Model::find_in_org(&ctx.db, member.org.id, id).await?;
    if !login.can_reveal(&ctx.db, member).await? {
        return Err(forbidden(
            "Only owners, admins and people named on this login can see the password.",
        ));
    }
    let name = match form.field.as_str() {
        field::RECOVERY if login.has_recovery() => field::RECOVERY,
        field::PASSWORD | "" => field::PASSWORD,
        _ => return Err(Error::NotFound),
    };
    if !member.user.secrets_unlocked(member.session_id.as_deref()) {
        // 423: the page asks for the app password, then tries again.
        return Ok(Err(plain(
            StatusCode::LOCKED,
            "Enter your app password first.",
        )?));
    }
    // Logged before decrypting: if this fails, nothing is shown.
    let kind = if copied { action::COPY } else { action::REVEAL };
    login
        .log(&ctx.db, member, client, kind, Some(name.to_string()))
        .await?;
    Ok(Ok((login, name)))
}

/// Tells the owners, after the secret has gone out. A failed email is
/// logged; Login activity already has the event.
async fn tell_owners(
    ctx: &AppContext,
    member: &CurrentMember,
    login: &logins::Model,
    name: &str,
    copied: bool,
) {
    if let Err(err) = SecurityMailer::login_seen(
        ctx,
        &member.org,
        &member.username,
        &login.title,
        copied,
        name == field::RECOVERY,
    )
    .await
    {
        tracing::error!(error = %err, login_id = login.id, "could not email owners about a shown password");
    }
}

/// Shows a password or recovery codes, as a fragment that hides itself.
#[debug_handler]
async fn reveal(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    client: Client,
    Path(id): Path<i64>,
    Form(form): Form<SecretForm>,
) -> Result<Response> {
    let (login, name) = match prepare_secret(&ctx, &member, &client, id, &form, false).await? {
        Ok(ready) => ready,
        Err(answer) => return Ok(answer),
    };
    let kek = Kek::get(&ctx)?;
    let secret = login.reveal(&ctx.db, &kek, name).await?;
    let response = format::render()
        .header(header::CACHE_CONTROL.as_str(), "no-store")
        .view(
            &v,
            "documents/_secret.html",
            data!({
                "secret": secret.expose_secret(),
                "field": name,
                "login": { "id": login.id },
            }),
        )?;
    tell_owners(&ctx, &member, &login, name, false).await;
    Ok(response)
}

/// Copies a password or recovery codes: the plain text, for the clipboard.
#[debug_handler]
async fn copy(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    client: Client,
    Path(id): Path<i64>,
    Form(form): Form<SecretForm>,
) -> Result<Response> {
    let (login, name) = match prepare_secret(&ctx, &member, &client, id, &form, true).await? {
        Ok(ready) => ready,
        Err(answer) => return Ok(answer),
    };
    let kek = Kek::get(&ctx)?;
    let secret = login.reveal(&ctx.db, &kek, name).await?;
    let mut response = Response::new(axum::body::Body::from(secret.expose_secret().to_string()));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    tell_owners(&ctx, &member, &login, name, true).await;
    Ok(response)
}

/// One line of Login activity.
#[derive(Debug, Serialize)]
struct EventRow {
    icon: &'static str,
    bad: bool,
    who: String,
    text: String,
    title: Option<String>,
    meta: String,
}

fn event_row(event: login_events::Model, tz: chrono_tz::Tz) -> EventRow {
    let recovery = event.detail.as_deref() == Some(field::RECOVERY);
    let what = if recovery {
        "the recovery codes for"
    } else {
        "the password for"
    };
    let detail = event.detail.clone().unwrap_or_default();
    let (icon, bad, text) = match event.action.as_str() {
        action::REVEAL => ("eye", false, format!("saw {what}")),
        action::COPY => ("copy", false, format!("copied {what}")),
        action::CREATE => ("plus", false, "added".to_string()),
        action::EDIT if detail == field::PASSWORD => {
            ("pencil", false, "changed the password for".to_string())
        }
        action::EDIT => ("pencil", false, "changed".to_string()),
        action::DELETE => ("trash", false, "deleted".to_string()),
        action::REAUTH_FAILED => (
            "circle-alert",
            true,
            format!("typed a wrong app password ({detail})"),
        ),
        action::LOCK => (
            "lock",
            true,
            "was locked out after too many wrong app passwords".to_string(),
        ),
        action::MEMBER_REMOVED => ("user-minus", false, format!("removed {detail}")),
        action::SIGN_OUT_EVERYWHERE => ("log-out", false, "signed out everywhere".to_string()),
        _ => ("circle-alert", false, event.action.clone()),
    };
    // Re-entering the password is about the login they were trying to see,
    // which goes in the second line.
    let trying = matches!(event.action.as_str(), action::REAUTH_FAILED | action::LOCK);
    let at = time::local(event.created_at, tz)
        .format("%H:%M")
        .to_string();
    let (title, meta) = match event.login_title {
        Some(title) if trying => (None, format!("{at} · trying to see {title}")),
        title => (title, at),
    };
    EventRow {
        icon,
        bad,
        who: event.username,
        text,
        title,
        meta,
    }
}

/// Owners read the login activity log, newest first, by day.
#[debug_handler]
async fn activity(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    member.require_own_membership()?;
    member.require_owner()?;
    let tz = member.tz();
    let today = time::today(tz);
    let mut days: Vec<serde_json::Value> = Vec::new();
    let mut current = None;
    // Right app passwords are kept in the log but not listed: the reveal or
    // copy that follows says the same thing.
    let shown = login_events::Model::for_org(&ctx.db, member.org.id, 500)
        .await?
        .into_iter()
        .filter(|e| e.action != action::UNLOCK);
    for event in shown {
        let day = time::local(event.created_at, tz).date_naive();
        if current != Some(day) {
            current = Some(day);
            let label = if day == today {
                "Today".to_string()
            } else if today.pred_opt() == Some(day) {
                "Yesterday".to_string()
            } else {
                day.format("%A %-d %B").to_string()
            };
            days.push(data!({ "label": label, "events": [] }));
        }
        if let Some(events) = days.last_mut().and_then(|d| d["events"].as_array_mut()) {
            events.push(serde_json::to_value(event_row(event, tz))?);
        }
    }
    format::render().view(
        &v,
        "documents/activity.html",
        member.page("documents", data!({ "days": days })),
    )
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/documents")
        .add("/activity", get(activity))
        .add("/folders/{id}/logins", post(create))
        .add("/logins/unlock", post(unlock))
        .add("/logins/{id}", get(show))
        .add("/logins/{id}", post(edit))
        .add("/logins/{id}/delete", post(delete))
        .add("/logins/{id}/reveal", post(reveal))
        .add("/logins/{id}/copy", post(copy))
        .layer(middleware::from_fn(no_framing))
}
