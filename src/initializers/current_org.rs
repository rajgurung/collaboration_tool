//! Two jobs for the organisation a request works in.
//!
//! **Sets the `current_org` cookie.** The cookie remembers the last
//! organisation, for bare addresses and sign-in. `CurrentMember` decides
//! when it should be cleared (it names an org the person isn't active in) or
//! moved (a page load at `/o/{slug}/`), but an extractor cannot set cookies.
//! So this layer puts an [`OrgCookie`] slot in every request, and adds the
//! matching `Set-Cookie` to the response, whatever it is.
//!
//! **Reads the tab's org claim.** Every app page says which organisation it
//! was drawn for, and sends that back as an `X-Org-Id` header (HTMX and the
//! raw `fetch` calls) or a hidden `_org` field (plain forms). An extractor
//! can't read a form body without consuming it, so this layer lifts the claim
//! into an [`OrgClaim`] extension, and puts the body back for `Form` to read.
//! `CurrentMember` compares it with the org it resolved.
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::{
    body::Body,
    extract::Request,
    http::{
        header::{CONTENT_LENGTH, CONTENT_TYPE, SET_COOKIE},
        HeaderMap, HeaderValue, Method, StatusCode,
    },
    middleware::{self, Next},
    response::IntoResponse,
    Router as AxumRouter,
};
use futures_util::StreamExt;
use loco_rs::{
    app::{AppContext, Initializer},
    Error, Result,
};

use crate::extractors::{
    current_member::{ORG_FIELD, ORG_HEADER},
    session::{cleared_current_org_cookie, current_org_cookie, SESSION_COOKIE},
};

/// The largest form body the layer reads. The same as Loco's default payload
/// limit (`DefaultBodyLimitKind::Limit(2_000_000)`), so the layer never
/// accepts or refuses a body differently from `Form`.
pub const FORM_BODY_LIMIT: usize = 2_000_000;

/// What the response does to the `current_org` cookie. Filled in by
/// `CurrentMember`; nothing unless it says so.
#[derive(Clone, Default)]
pub struct OrgCookie(Arc<Mutex<CookieChange>>);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CookieChange {
    #[default]
    Nothing,
    Clear,
    Remember(i64),
}

impl OrgCookie {
    /// The cookie named an org the person isn't active in.
    pub fn clear(&self) {
        self.set(CookieChange::Clear);
    }

    /// Remember `org_id` as the last organisation.
    pub fn remember(&self, org_id: i64) {
        self.set(CookieChange::Remember(org_id));
    }

    fn set(&self, change: CookieChange) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = change;
        }
    }

    fn take(&self) -> CookieChange {
        self.0.lock().map(|slot| *slot).unwrap_or_default()
    }
}

/// Which organisation the tab that sent the request was drawn for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrgClaim {
    /// No header and no `_org` field.
    Missing,
    Org(i64),
    /// Present but not one number: empty, several headers, or not an `i64`.
    Garbage,
}

pub struct CurrentOrgInitializer;

#[async_trait]
impl Initializer for CurrentOrgInitializer {
    fn name(&self) -> String {
        "current-org".to_string()
    }

    async fn after_routes(&self, router: AxumRouter, ctx: &AppContext) -> Result<AxumRouter> {
        let clear = HeaderValue::from_str(&cleared_current_org_cookie(ctx)?.to_string())
            .map_err(|_| Error::string("invalid cookie header"))?;
        let ctx = ctx.clone();
        Ok(
            router.layer(middleware::from_fn(move |req: Request, next: Next| {
                let clear = clear.clone();
                let ctx = ctx.clone();
                async move {
                    let (mut req, claim) = match read_claim(req).await {
                        Ok(read) => read,
                        Err(status) => return status.into_response(),
                    };
                    let slot = OrgCookie::default();
                    req.extensions_mut().insert(slot.clone());
                    req.extensions_mut().insert(claim);
                    let mut response = next.run(req).await;
                    let set = match slot.take() {
                        CookieChange::Nothing => None,
                        CookieChange::Clear => Some(clear),
                        CookieChange::Remember(id) => current_org_cookie(&ctx, id)
                            .ok()
                            .and_then(|c| HeaderValue::from_str(&c.to_string()).ok()),
                    };
                    if let Some(set) = set {
                        response.headers_mut().append(SET_COOKIE, set);
                    }
                    response
                }
            })),
        )
    }
}

/// The claim from the header, else from a signed-in urlencoded form body.
/// Multipart and JSON bodies, and requests without a session, are never
/// buffered.
async fn read_claim(req: Request) -> std::result::Result<(Request, OrgClaim), StatusCode> {
    let headers = req.headers();
    let mut values = headers.get_all(ORG_HEADER).iter();
    if let Some(value) = values.next() {
        let claim = match (values.next(), parse_id(value.as_bytes())) {
            (None, Some(id)) => OrgClaim::Org(id),
            _ => OrgClaim::Garbage,
        };
        return Ok((req, claim));
    }
    let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if safe || !has_session(headers) || !is_urlencoded(headers) {
        return Ok((req, OrgClaim::Missing));
    }

    let declared = headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|len| len > FORM_BODY_LIMIT as u64) {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let (parts, body) = req.into_parts();
    let bytes = read_body(body).await?;
    let claim = url::form_urlencoded::parse(&bytes)
        .find(|(key, _)| key == ORG_FIELD)
        .map_or(OrgClaim::Missing, |(_, value)| {
            parse_id(value.as_bytes()).map_or(OrgClaim::Garbage, OrgClaim::Org)
        });
    Ok((Request::from_parts(parts, Body::from(bytes)), claim))
}

/// The whole body, or 413 once it passes the limit (a chunked body has no
/// length to check first), or 400 if it can't be read.
async fn read_body(body: Body) -> std::result::Result<Vec<u8>, StatusCode> {
    let mut stream = body.into_data_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            return Err(StatusCode::BAD_REQUEST);
        };
        if bytes.len() + chunk.len() > FORM_BODY_LIMIT {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn parse_id(value: &[u8]) -> Option<i64> {
    std::str::from_utf8(value).ok()?.parse::<i64>().ok()
}

fn has_session(headers: &HeaderMap) -> bool {
    axum_extra::extract::cookie::CookieJar::from_headers(headers)
        .get(SESSION_COOKIE)
        .is_some()
}

/// `application/x-www-form-urlencoded`, in any case, with any parameters.
fn is_urlencoded(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|base| {
            base.trim()
                .eq_ignore_ascii_case("application/x-www-form-urlencoded")
        })
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, extract::Request, http::StatusCode};
    use futures_util::stream;

    use super::{read_claim, OrgClaim, FORM_BODY_LIMIT};

    fn signed_in_form() -> axum::http::request::Builder {
        Request::post("/tasks")
            .header("cookie", "auth_token=x")
            .header("content-type", "application/x-www-form-urlencoded")
    }

    // Tested here rather than over the test server's socket: the server
    // answers before reading the body, so a client still sending it may see a
    // reset instead of the 413.
    #[tokio::test]
    async fn a_body_over_the_limit_is_413_before_reading() {
        let req = signed_in_form()
            .header("content-length", (FORM_BODY_LIMIT + 1).to_string())
            .body(Body::from("_org=1"))
            .unwrap();
        assert_eq!(
            read_claim(req).await.err(),
            Some(StatusCode::PAYLOAD_TOO_LARGE)
        );

        // No length to check first: going over while reading is 413 too.
        let chunks = vec![
            Ok::<_, std::io::Error>(vec![b'a'; FORM_BODY_LIMIT]),
            Ok(b"&_org=1".to_vec()),
        ];
        let req = signed_in_form()
            .body(Body::from_stream(stream::iter(chunks)))
            .unwrap();
        assert_eq!(
            read_claim(req).await.err(),
            Some(StatusCode::PAYLOAD_TOO_LARGE)
        );
    }

    #[tokio::test]
    async fn the_body_is_put_back_for_form() {
        let req = signed_in_form()
            .body(Body::from("title=Hi&_org=7&_org=8"))
            .unwrap();
        let (req, claim) = read_claim(req).await.unwrap();
        assert_eq!(claim, OrgClaim::Org(7), "the first _org");
        let bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"title=Hi&_org=7&_org=8");
    }

    #[tokio::test]
    async fn the_header_wins_and_the_body_is_not_read() {
        let req = signed_in_form()
            .header("x-org-id", "3")
            .header("content-length", (FORM_BODY_LIMIT + 1).to_string())
            .body(Body::from("_org=9"))
            .unwrap();
        assert_eq!(read_claim(req).await.unwrap().1, OrgClaim::Org(3));
    }
}
