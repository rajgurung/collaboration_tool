//! OAuth for Claude's connector: discovery documents, client registration
//! (RFC 7591), the "Allow access" page and the token endpoint. Clients are
//! public and use PKCE (S256); tokens are bound to the `/mcp` resource.
use axum::{
    body::Bytes,
    extract::{rejection::FormRejection, DefaultBodyLimit},
    http::{header, HeaderMap, HeaderValue, StatusCode},
};
use loco_rs::prelude::*;
use serde::Deserialize;

use crate::{
    data::settings::Settings,
    extractors::{current_member::CurrentMember, current_user::CurrentUser},
    models::{
        access_tokens::{self, CodeExchange, Issued, SCOPE},
        oauth_clients::{self, RegisterError, RegisterParams},
        oauth_codes::{self, Consent},
    },
};

/// Registration bodies are small; refuse anything bigger before parsing.
const REGISTER_BODY_LIMIT: usize = 8 * 1024;

#[debug_handler]
async fn protected_resource(State(ctx): State<AppContext>) -> Result<Response> {
    let settings = Settings::from_context(&ctx)?;
    format::json(serde_json::json!({
        "resource": settings.mcp_url(),
        "resource_name": "Collab Tool",
        "authorization_servers": [settings.app_url],
        "scopes_supported": [SCOPE],
        "bearer_methods_supported": ["header"],
    }))
}

#[debug_handler]
async fn authorization_server(State(ctx): State<AppContext>) -> Result<Response> {
    let app_url = Settings::from_context(&ctx)?.app_url;
    format::json(serde_json::json!({
        "issuer": app_url,
        "authorization_endpoint": format!("{app_url}/oauth/authorize"),
        "token_endpoint": format!("{app_url}/oauth/token"),
        "registration_endpoint": format!("{app_url}/oauth/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "scopes_supported": [SCOPE],
        "authorization_response_iss_parameter_supported": true,
    }))
}

/// An OAuth error body (RFC 6749 §5.2, RFC 7591 §3.2.2).
fn oauth_error(status: StatusCode, error: &str, description: &str) -> Response {
    let mut response = (
        status,
        axum::Json(serde_json::json!({ "error": error, "error_description": description })),
    )
        .into_response();
    no_store(response.headers_mut());
    response
}

fn no_store(headers: &mut HeaderMap) {
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
}

/// Dynamic client registration. Always a public client, whatever was asked for.
#[debug_handler]
async fn register(State(ctx): State<AppContext>, body: Bytes) -> Result<Response> {
    let Ok(params) = serde_json::from_slice::<RegisterParams>(&body) else {
        return Ok(oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_client_metadata",
            "The body must be a JSON client registration.",
        ));
    };
    let client = match oauth_clients::Model::register(&ctx.db, &params).await {
        Ok(client) => client,
        Err(RegisterError::RedirectUri(why)) => {
            return Ok(oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_redirect_uri",
                why,
            ))
        }
        Err(RegisterError::Metadata(why)) => {
            return Ok(oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_client_metadata",
                why,
            ))
        }
        Err(RegisterError::Db(err)) => return Err(err.into()),
    };
    let mut response = (
        StatusCode::CREATED,
        axum::Json(serde_json::json!({
            "client_id": client.client_id,
            "client_id_issued_at": client.created_at.timestamp(),
            "client_name": client.client_name,
            "redirect_uris": client.redirect_uris,
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
            "scope": SCOPE,
        })),
    )
        .into_response();
    no_store(response.headers_mut());
    Ok(response)
}

/// The authorization request, as query parameters (GET) or hidden form
/// fields on the consent page (POST).
#[derive(Debug, Default, Deserialize)]
struct AuthorizeParams {
    response_type: Option<String>,
    client_id: Option<String>,
    redirect_uri: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    state: Option<String>,
    scope: Option<String>,
    resource: Option<String>,
    /// The button pressed on the consent page: "allow" or anything else.
    decision: Option<String>,
}

/// A request that checked out.
struct Approved {
    client: oauth_clients::Model,
    redirect_uri: String,
    code_challenge: String,
    state: Option<String>,
    resource: String,
}

/// A request that did not. Bad clients and redirect URIs get a page, since
/// they cannot be trusted with a redirect; everything else goes back to the
/// client with an error code.
enum Refusal {
    Page(&'static str),
    Redirect {
        redirect_uri: String,
        state: Option<String>,
        error: &'static str,
    },
}

async fn review(
    ctx: &AppContext,
    settings: &Settings,
    params: &AuthorizeParams,
) -> Result<std::result::Result<Approved, Refusal>> {
    let client = match params.client_id.as_deref() {
        Some(id) => oauth_clients::Model::find_by_client_id(&ctx.db, id)
            .await
            .ok(),
        None => None,
    };
    let Some(client) = client else {
        return Ok(Err(Refusal::Page(
            "This app is not registered with Collab Tool. Start again from Claude.",
        )));
    };
    let Some(redirect_uri) = params
        .redirect_uri
        .clone()
        .filter(|uri| client.accepts_redirect(uri))
    else {
        return Ok(Err(Refusal::Page(
            "This request does not come back to Claude, so it was stopped.",
        )));
    };
    let send_back = |error| {
        Ok(Err(Refusal::Redirect {
            redirect_uri: redirect_uri.clone(),
            state: params.state.clone(),
            error,
        }))
    };
    if params.response_type.as_deref() != Some("code") {
        return send_back("unsupported_response_type");
    }
    let challenge = params.code_challenge.clone().unwrap_or_default();
    if params.code_challenge_method.as_deref() != Some("S256")
        || !oauth_codes::is_challenge(&challenge)
    {
        return send_back("invalid_request");
    }
    let resource = params
        .resource
        .clone()
        .unwrap_or_else(|| settings.mcp_url());
    if resource != settings.mcp_url() {
        return send_back("invalid_target");
    }
    if !params
        .scope
        .as_deref()
        .unwrap_or_default()
        .split_whitespace()
        .all(|s| s == SCOPE)
    {
        return send_back("invalid_scope");
    }
    Ok(Ok(Approved {
        client,
        redirect_uri: redirect_uri.clone(),
        code_challenge: challenge,
        state: params.state.clone(),
        resource,
    }))
}

/// Sends the browser back to Claude with `pairs` (plus `state` and `iss`).
fn back_to_client(
    settings: &Settings,
    redirect_uri: &str,
    state: Option<&str>,
    pairs: &[(&str, &str)],
) -> Result<Response> {
    let mut url = url::Url::parse(redirect_uri).map_err(|_| Error::BadRequest(String::new()))?;
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in pairs {
            query.append_pair(key, value);
        }
        if let Some(state) = state {
            query.append_pair("state", state);
        }
        query.append_pair("iss", &settings.app_url);
    }
    let mut response = (StatusCode::FOUND, [(header::LOCATION, url.to_string())]).into_response();
    no_store(response.headers_mut());
    Ok(response)
}

/// Pages here must never be framed (clickjacking the Allow button) or cached.
fn guarded(mut response: Response) -> Response {
    let headers = response.headers_mut();
    no_store(headers);
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("frame-ancestors 'none'"),
    );
    response
}

fn refused_page(v: &TeraView, message: &str) -> Result<Response> {
    Ok(guarded(format::render().status(400).view(
        v,
        "oauth/error.html",
        data!({ "message": message }),
    )?))
}

/// A super admin working inside another organisation must leave it first:
/// Claude would otherwise act in an organisation they do not belong to.
fn acting_page(v: &TeraView, member: &CurrentMember) -> Result<Response> {
    Ok(guarded(format::render().status(403).view(
        v,
        "oauth/acting.html",
        data!({ "org": { "name": member.org.name } }),
    )?))
}

fn refuse(v: &TeraView, settings: &Settings, refusal: Refusal) -> Result<Response> {
    match refusal {
        Refusal::Page(message) => refused_page(v, message),
        Refusal::Redirect {
            redirect_uri,
            state,
            error,
        } => back_to_client(
            settings,
            &redirect_uri,
            state.as_deref(),
            &[("error", error)],
        ),
    }
}

/// The "Allow access" page. Signed-out people log in first and come back.
#[debug_handler]
async fn authorize_page(
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    user: Option<CurrentUser>,
    member: std::result::Result<CurrentMember, Response>,
    uri: axum::http::Uri,
    Query(params): Query<AuthorizeParams>,
) -> Result<Response> {
    let settings = Settings::from_context(&ctx)?;
    let approved = match review(&ctx, &settings, &params).await? {
        Ok(approved) => approved,
        Err(refusal) => return refuse(&v, &settings, refusal),
    };
    if user.is_none() {
        let here = format!("/oauth/authorize?{}", uri.query().unwrap_or_default());
        let next: String = url::form_urlencoded::byte_serialize(here.as_bytes()).collect();
        return Ok(guarded(
            (
                StatusCode::SEE_OTHER,
                [(header::LOCATION, format!("/login?next={next}"))],
            )
                .into_response(),
        ));
    }
    let member = match member {
        Ok(member) => member,
        Err(page) => return Ok(guarded(page)),
    };
    if member.acting {
        return acting_page(&v, &member);
    }
    Ok(guarded(format::render().view(
        &v,
        "oauth/authorize.html",
        data!({
            "client_name": approved.client.client_name,
            "org": { "name": member.org.name },
            "username": member.username,
            "back_to": oauth_clients::redirect_host(&approved.redirect_uri),
            "fields": {
                "response_type": "code",
                "client_id": approved.client.client_id,
                "redirect_uri": approved.redirect_uri,
                "code_challenge": approved.code_challenge,
                "code_challenge_method": "S256",
                "state": approved.state,
                "scope": SCOPE,
                "resource": approved.resource,
            },
        }),
    )?))
}

/// Allow or Deny. Everything is checked again: the form is just as easy to forge.
#[debug_handler]
async fn authorize(
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    member: CurrentMember,
    Form(params): Form<AuthorizeParams>,
) -> Result<Response> {
    let settings = Settings::from_context(&ctx)?;
    let approved = match review(&ctx, &settings, &params).await? {
        Ok(approved) => approved,
        Err(refusal) => return refuse(&v, &settings, refusal),
    };
    if member.acting {
        return acting_page(&v, &member);
    }
    let state = approved.state.as_deref();
    if params.decision.as_deref() != Some("allow") {
        return back_to_client(
            &settings,
            &approved.redirect_uri,
            state,
            &[("error", "access_denied")],
        );
    }
    let (_, code) = oauth_codes::Model::issue(
        &ctx.db,
        &Consent {
            client: &approved.client,
            org_id: member.org.id,
            user_id: member.user.id,
            redirect_uri: &approved.redirect_uri,
            code_challenge: &approved.code_challenge,
            resource: &approved.resource,
            scope: SCOPE,
        },
    )
    .await?;
    back_to_client(&settings, &approved.redirect_uri, state, &[("code", &code)])
}

#[derive(Debug, Default, Deserialize)]
struct TokenForm {
    grant_type: Option<String>,
    client_id: Option<String>,
    code: Option<String>,
    code_verifier: Option<String>,
    redirect_uri: Option<String>,
    refresh_token: Option<String>,
    resource: Option<String>,
}

/// Swaps a code or refresh token for tokens (RFC 6749 §4.1.3 and §6).
#[debug_handler]
async fn token(
    State(ctx): State<AppContext>,
    form: std::result::Result<Form<TokenForm>, FormRejection>,
) -> Result<Response> {
    let bad_request = |error, why| Ok(oauth_error(StatusCode::BAD_REQUEST, error, why));
    let Ok(Form(form)) = form else {
        return bad_request("invalid_request", "Send a form-encoded token request.");
    };
    let settings = Settings::from_context(&ctx)?;
    let client = match form.client_id.as_deref() {
        Some(id) => oauth_clients::Model::find_by_client_id(&ctx.db, id)
            .await
            .ok(),
        None => None,
    };
    let Some(client) = client else {
        return Ok(oauth_error(
            StatusCode::UNAUTHORIZED,
            "invalid_client",
            "Unknown client_id.",
        ));
    };
    access_tokens::Model::prune(&ctx.db).await?;
    let resource = form.resource.clone().unwrap_or_else(|| settings.mcp_url());
    let issued = match form.grant_type.as_deref() {
        Some("authorization_code") => {
            let (Some(code), Some(code_verifier), Some(redirect_uri)) =
                (&form.code, &form.code_verifier, &form.redirect_uri)
            else {
                return bad_request(
                    "invalid_request",
                    "code, code_verifier and redirect_uri are required.",
                );
            };
            access_tokens::Model::exchange_code(
                &ctx.db,
                &CodeExchange {
                    code,
                    code_verifier,
                    redirect_uri,
                    resource: &resource,
                    client: &client,
                },
            )
            .await?
        }
        Some("refresh_token") => {
            let Some(refresh_token) = &form.refresh_token else {
                return bad_request("invalid_request", "refresh_token is required.");
            };
            access_tokens::Model::refresh(&ctx.db, refresh_token, &client, &resource).await?
        }
        _ => {
            return bad_request(
                "unsupported_grant_type",
                "Use authorization_code or refresh_token.",
            )
        }
    };
    let Some(Issued {
        access_token,
        refresh_token,
        scope,
    }) = issued
    else {
        return bad_request(
            "invalid_grant",
            "The code or refresh token is invalid, expired or already used.",
        );
    };
    let mut response = axum::Json(serde_json::json!({
        "access_token": access_token,
        "token_type": "Bearer",
        "expires_in": access_tokens::ACCESS_SECONDS,
        "refresh_token": refresh_token,
        "scope": scope,
    }))
    .into_response();
    no_store(response.headers_mut());
    Ok(response)
}

pub fn routes() -> Routes {
    Routes::new()
        .add(
            "/.well-known/oauth-protected-resource",
            get(protected_resource),
        )
        .add(
            "/.well-known/oauth-protected-resource/mcp",
            get(protected_resource),
        )
        .add(
            "/.well-known/oauth-authorization-server",
            get(authorization_server),
        )
        .add(
            "/oauth/register",
            post(register).layer(DefaultBodyLimit::max(REGISTER_BODY_LIMIT)),
        )
        .add("/oauth/authorize", get(authorize_page))
        .add("/oauth/authorize", post(authorize))
        .add("/oauth/token", post(token))
}
