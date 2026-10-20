//! The organisation in the address.
//!
//! `/o/acme-studio/tasks/58` is routed as `/tasks/58`, with [`PathOrg`]
//! (`acme-studio`) in the request's extensions for `CurrentMember` to resolve.
//! Every route, handler and `Path` extractor stays as it is.
//!
//! The rewrite has to happen before routing, so this layer wraps the whole
//! router (a `Router::layer` runs after routing and couldn't change which
//! route matches). It must be the last initializer, see `App::initializers`.
//!
//! It also sets `Referrer-Policy` and `X-Robots-Tag` on every response.
use async_trait::async_trait;
use axum::{
    extract::{OriginalUri, Request},
    http::{header, uri::PathAndQuery, HeaderValue, Method, StatusCode, Uri},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Router as AxumRouter,
};
use loco_rs::{
    app::{AppContext, Initializer},
    Result,
};
use tracing::Instrument;

use crate::views::links::{is_slug, TENANT_SECTIONS};

/// The organisation slug from the address. Only `CurrentMember`'s resolver
/// and the old switch read it (`tests/requests/org_address.rs` checks).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathOrg {
    pub slug: String,
}

pub struct OrgPathInitializer;

#[async_trait]
impl Initializer for OrgPathInitializer {
    fn name(&self) -> String {
        "org-path".to_string()
    }

    async fn after_routes(&self, router: AxumRouter, _ctx: &AppContext) -> Result<AxumRouter> {
        // The outer router has no routes, so everything reaches the fallback,
        // and the inner router routes the rewritten request.
        Ok(AxumRouter::new()
            .fallback_service(router)
            .layer(middleware::from_fn(org_path)))
    }
}

/// What the layer does with an address.
#[derive(Debug, PartialEq, Eq)]
pub enum Strip {
    /// `/o/{slug}` or `/o/{slug}/` (GET or HEAD): Home in that org.
    Home(String),
    /// Under a tenant section: routed as `uri`, in `slug`.
    Rewrite { slug: String, uri: Uri },
    /// Anything else, untouched.
    Pass,
}

/// Works only on the URI's path and query. `Err` is a URI that can't be
/// rebuilt (a 400).
///
/// # Errors
/// `400 Bad Request` when the new URI doesn't build.
pub fn strip(method: &Method, uri: &Uri) -> std::result::Result<Strip, StatusCode> {
    let Some(after) = uri.path().strip_prefix("/o/") else {
        return Ok(Strip::Pass);
    };
    let (slug, rest) = match after.split_once('/') {
        Some((slug, rest)) => (slug, rest),
        None => (after, ""),
    };
    if !is_slug(slug) {
        return Ok(Strip::Pass);
    }
    if rest.is_empty() {
        let page_load = matches!(*method, Method::GET | Method::HEAD);
        return Ok(if page_load {
            Strip::Home(slug.to_string())
        } else {
            Strip::Pass
        });
    }
    let section = rest.split('/').next().unwrap_or_default();
    if !TENANT_SECTIONS.contains(&section) || rest.split('/').any(is_dot_segment) {
        return Ok(Strip::Pass);
    }
    let path_and_query = match uri.query() {
        Some(query) => format!("/{rest}?{query}"),
        None => format!("/{rest}"),
    };
    let mut parts = uri.clone().into_parts();
    parts.path_and_query =
        Some(PathAndQuery::try_from(path_and_query).map_err(|_| StatusCode::BAD_REQUEST)?);
    let uri = Uri::from_parts(parts).map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(Strip::Rewrite {
        slug: slug.to_string(),
        uri,
    })
}

/// `.` or `..`, raw or with any dot percent-encoded.
fn is_dot_segment(segment: &str) -> bool {
    let decoded = segment.to_ascii_lowercase().replace("%2e", ".");
    decoded == "." || decoded == ".."
}

async fn org_path(mut req: Request, next: Next) -> Response {
    let page_load = matches!(*req.method(), Method::GET | Method::HEAD);
    let homepage = page_load && req.uri().path() == "/";
    let mut response = match strip(req.method(), req.uri()) {
        Err(status) => status.into_response(),
        // Safe to echo: `strip` only returns slugs that pass `is_slug`.
        Ok(Strip::Home(slug)) => see_other(&format!("/o/{slug}/dashboard")),
        Ok(Strip::Rewrite { slug, uri }) => {
            let original = std::mem::replace(req.uri_mut(), uri);
            req.extensions_mut().insert(OriginalUri(original));
            req.extensions_mut().insert(PathOrg { slug: slug.clone() });
            // Loco's logger logs the stripped path, so the slug goes on a
            // span of its own around the request.
            let span = tracing::info_span!("org", slug = %slug);
            next.run(req).instrument(span).await
        }
        Ok(Strip::Pass) => next.run(req).await,
    };
    let headers = response.headers_mut();
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    if !homepage {
        headers.insert("x-robots-tag", HeaderValue::from_static("noindex"));
    }
    response
}

/// A 303 that is never cached, or a 400 if `to` isn't a header value.
fn see_other(to: &str) -> Response {
    match HeaderValue::from_str(to) {
        Ok(location) => (
            StatusCode::SEE_OTHER,
            [
                (header::LOCATION, location),
                (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            ],
        )
            .into_response(),
        Err(_) => StatusCode::BAD_REQUEST.into_response(),
    }
}

/// The layer a revert of this project keeps, so `/o/` links sent in the
/// meantime still open: GET or HEAD for `/o/{slug}/{rest}` is a 303 to
/// `/{rest}` with its query, which then opens in the cookie's org. Anything
/// else under `/o/` is a 404. No database, no cookie. Unused until then:
/// `AxumRouter::new().fallback_service(router).layer(middleware::from_fn(revert_layer))`.
pub async fn revert_layer(req: Request, next: Next) -> Response {
    match revert_redirect(req.method(), req.uri()) {
        Some(response) => response,
        None => next.run(req).await,
    }
}

/// [`revert_layer`]'s answer, or `None` outside `/o/`.
#[must_use]
pub fn revert_redirect(method: &Method, uri: &Uri) -> Option<Response> {
    let after = uri.path().strip_prefix("/o/")?;
    let (slug, rest) = after.split_once('/').unwrap_or((after, ""));
    let page_load = matches!(*method, Method::GET | Method::HEAD);
    // A rest starting with `/` or `\` would make `//host`, another site;
    // `.` and `..` segments could walk to one after the browser resolves them.
    if !page_load
        || !is_slug(slug)
        || rest.starts_with(['/', '\\'])
        || rest.split('/').any(is_dot_segment)
    {
        return Some(StatusCode::NOT_FOUND.into_response());
    }
    let to = match uri.query() {
        Some(query) => format!("/{rest}?{query}"),
        None => format!("/{rest}"),
    };
    Some(see_other(&to))
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use axum::{
        extract::{ConnectInfo, Request},
        http::{header, Method, StatusCode, Uri},
        routing::get,
        Router,
    };

    use super::{revert_redirect, strip, PathOrg, Strip};

    fn stripped(method: Method, uri: &str) -> Strip {
        strip(&method, &uri.parse::<Uri>().unwrap()).unwrap()
    }

    fn rewrite(slug: &str, uri: &str) -> Strip {
        Strip::Rewrite {
            slug: slug.to_string(),
            uri: uri.parse().unwrap(),
        }
    }

    #[test]
    fn tenant_sections_are_stripped() {
        assert_eq!(
            stripped(Method::GET, "/o/acme/tasks"),
            rewrite("acme", "/tasks")
        );
        assert_eq!(
            stripped(Method::POST, "/o/acme/tasks/58/notes?x=1&y=/o/b"),
            rewrite("acme", "/tasks/58/notes?x=1&y=/o/b")
        );
        assert_eq!(
            stripped(Method::GET, "/o/acme/chat/12/threads/9"),
            rewrite("acme", "/chat/12/threads/9")
        );
        assert_eq!(
            stripped(Method::GET, "/o/acme/settings/claude"),
            rewrite("acme", "/settings/claude")
        );
    }

    #[test]
    fn odd_shapes_are_left_alone() {
        for uri in [
            "/o/acme/tasks;x",
            "/o/acme/tasksx",
            "/o/acme/tasks%2F..%2Fadmin",
            "/o/acme//tasks",
            "/o/acme/./tasks",
            "/o/acme/tasks/../../admin",
            "/o/acme/tasks/%2e%2E/admin",
            "/o/acme/tasks/.%2e/admin",
            "/o/acme/tasks/%2e",
            "/o/Acme/tasks",
            "/o/acme/o/beta/tasks",
            "/o/acme/admin",
            "/o/acme/login",
            "/o//tasks",
            "/o/",
            "/tasks",
            "/oo/acme/tasks",
        ] {
            assert_eq!(stripped(Method::GET, uri), Strip::Pass, "{uri}");
        }
    }

    #[test]
    fn the_org_alone_is_home_for_get_and_head_only() {
        for uri in ["/o/acme", "/o/acme/", "/o/acme?x=1"] {
            assert_eq!(
                stripped(Method::GET, uri),
                Strip::Home("acme".into()),
                "{uri}"
            );
            assert_eq!(
                stripped(Method::HEAD, uri),
                Strip::Home("acme".into()),
                "{uri}"
            );
            assert_eq!(stripped(Method::POST, uri), Strip::Pass, "{uri}");
        }
    }

    #[test]
    fn an_absolute_uri_keeps_its_scheme_and_authority() {
        let Strip::Rewrite { slug, uri } = stripped(
            Method::GET,
            "http://collab.zen-scale.io/o/acme/tasks?open=5",
        ) else {
            panic!("rewritten");
        };
        assert_eq!(slug, "acme");
        assert_eq!(uri.to_string(), "http://collab.zen-scale.io/tasks?open=5");
    }

    /// The router the layer wraps in the app, cut down to one route that
    /// reports what it saw.
    fn app() -> Router {
        let inner = Router::new()
            .route(
                "/tasks/{id}",
                get(
                    |axum::extract::Path(id): axum::extract::Path<i64>, req: Request| async move {
                        let org = req.extensions().get::<PathOrg>().map(|o| o.slug.clone());
                        let marker = req.extensions().get::<Marker>().is_some();
                        let addr = req
                            .extensions()
                            .get::<ConnectInfo<SocketAddr>>()
                            .map(|c| c.0.to_string());
                        let original = req
                            .extensions()
                            .get::<axum::extract::OriginalUri>()
                            .map(|o| o.0.to_string());
                        format!("{id} {org:?} {marker} {addr:?} {original:?}")
                    },
                ),
            )
            .fallback(|| async { StatusCode::NOT_FOUND });
        Router::new()
            .fallback_service(inner)
            .layer(axum::middleware::from_fn(super::org_path))
            // Outermost, like hyper's own extensions.
            .layer(axum::middleware::from_fn(
                |mut req: Request, next: axum::middleware::Next| async move {
                    req.extensions_mut().insert(Marker);
                    next.run(req).await
                },
            ))
    }

    #[derive(Clone)]
    struct Marker;

    fn server() -> axum_test::TestServer {
        axum_test::TestServer::new(app().into_make_service_with_connect_info::<SocketAddr>())
            .unwrap()
    }

    #[tokio::test]
    async fn the_request_keeps_its_extensions_and_original_address() {
        let res = server().get("/o/acme/tasks/58").await;
        assert_eq!(res.status_code(), StatusCode::OK);
        let body = res.text();
        assert!(
            body.starts_with("58 Some(\"acme\") true Some(\"127.0.0.1:"),
            "{body}"
        );
        assert!(body.ends_with("Some(\"/o/acme/tasks/58\")"), "{body}");
        assert_eq!(res.headers()["referrer-policy"], "same-origin");
        assert_eq!(res.headers()["x-robots-tag"], "noindex");
    }

    #[tokio::test]
    async fn untouched_addresses_reach_the_router_as_they_are() {
        let server = server();
        let res = server.get("/tasks/7").await;
        assert_eq!(res.status_code(), StatusCode::OK);
        assert!(res.text().starts_with("7 None true"), "{}", res.text());
        let res = server.get("/o/acme/tasks/7x").await;
        assert_eq!(
            res.status_code(),
            StatusCode::BAD_REQUEST,
            "Path<i64> refused"
        );
        let res = server.get("/o/acme/members/7").await;
        assert_eq!(res.status_code(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_org_alone_redirects_home_uncached() {
        let res = server().get("/o/acme").await;
        assert_eq!(res.status_code(), StatusCode::SEE_OTHER);
        assert_eq!(res.headers()[header::LOCATION], "/o/acme/dashboard");
        assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
    }

    #[tokio::test]
    async fn only_the_homepage_may_be_indexed() {
        let server = server();
        let res = server.get("/").await;
        assert!(res.headers().get("x-robots-tag").is_none());
        assert_eq!(res.headers()["referrer-policy"], "same-origin");
        let res = server.post("/").await;
        assert_eq!(res.headers()["x-robots-tag"], "noindex");
    }

    #[test]
    fn a_header_value_that_wont_build_is_a_400() {
        // Every rewrite is shorter than an address the http crate already
        // parsed, so only a header value can fail in practice.
        assert_eq!(
            super::see_other("/o/acme/dashboard").status(),
            StatusCode::SEE_OTHER
        );
        assert_eq!(
            super::see_other("/o/acme/\n").status(),
            StatusCode::BAD_REQUEST
        );
    }

    fn revert(method: Method, uri: &str) -> Option<(StatusCode, Option<String>)> {
        revert_redirect(&method, &uri.parse().unwrap()).map(|res| {
            let to = res
                .headers()
                .get(header::LOCATION)
                .map(|v| v.to_str().unwrap().to_string());
            (res.status(), to)
        })
    }

    #[test]
    fn the_rollback_stub_redirects_page_loads_only() {
        let see = |to: &str| Some((StatusCode::SEE_OTHER, Some(to.to_string())));
        let not_found = Some((StatusCode::NOT_FOUND, None));
        assert_eq!(
            revert(Method::GET, "/o/acme/tasks?open=58"),
            see("/tasks?open=58")
        );
        assert_eq!(revert(Method::HEAD, "/o/acme/chat/12"), see("/chat/12"));
        assert_eq!(revert(Method::GET, "/o/acme/"), see("/"));
        assert_eq!(revert(Method::POST, "/o/acme/tasks"), not_found);
        assert_eq!(revert(Method::GET, "/o/Acme/tasks"), not_found);
        assert_eq!(revert(Method::GET, "/o/acme//evil.com"), not_found);
        assert_eq!(
            revert(Method::GET, "/o/acme/%5Cevil.com"),
            see("/%5Cevil.com")
        );
        assert_eq!(revert(Method::GET, "/tasks"), None);
        // Odd paths stay on this host: one leading `/`, then the raw rest.
        for odd in ["/o/acme/%2F%2Fevil.com", "/o/acme/../..//evil.com"] {
            let Some((_, Some(to))) = revert(Method::GET, odd) else {
                continue; // A 404: fine, nothing to follow.
            };
            assert!(
                to.starts_with('/') && !to.starts_with("//"),
                "{odd} -> {to}"
            );
            assert!(!to.starts_with("/\\"), "{odd} -> {to}");
        }
        assert_eq!(
            revert(Method::GET, "/o/acme/%2F%2Fevil.com"),
            see("/%2F%2Fevil.com")
        );
        assert_eq!(revert(Method::GET, "/o/acme/../..//evil.com"), not_found);
    }
}
