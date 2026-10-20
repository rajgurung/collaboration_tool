//! Response headers shared by pages that must not be framed or cached.
use axum::http::{header, HeaderValue};
use loco_rs::prelude::Response;

/// For pages with a button that grants something (Claude's "Allow access",
/// "Ask to join"): never framed, so the button can't be clickjacked, and
/// never cached.
#[must_use]
pub fn guarded(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("frame-ancestors 'none'"),
    );
    response
}
