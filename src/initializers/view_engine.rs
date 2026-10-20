use async_trait::async_trait;
use axum::{Extension, Router as AxumRouter};
use fluent_templates::{ArcLoader, FluentLoader};
use loco_rs::{
    app::{AppContext, Initializer},
    controller::views::{engines, ViewEngine},
    Error, Result,
};
use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    path::Path,
    sync::{Mutex, OnceLock},
};
use tracing::info;

use crate::views::links;

const I18N_DIR: &str = "assets/i18n";
// NOTE: must live OUTSIDE `I18N_DIR` so the locale scan doesn't also pick it up;
// otherwise the shared resource is registered twice and bundle building fails
// ("Failed to add FTL resources to the bundle"). See loco-rs/loco#1749.
const I18N_SHARED: &str = "assets/shared.ftl";
const STATIC_DIR: &str = "assets/static";
#[allow(clippy::module_name_repetitions)]
pub struct ViewEngineInitializer;

#[async_trait]
impl Initializer for ViewEngineInitializer {
    fn name(&self) -> String {
        "view-engine".to_string()
    }

    async fn after_routes(&self, router: AxumRouter, _ctx: &AppContext) -> Result<AxumRouter> {
        let tera_engine = if std::path::Path::new(I18N_DIR).exists() {
            let arc = std::sync::Arc::new(
                ArcLoader::builder(&I18N_DIR, unic_langid::langid!("en-US"))
                    .shared_resources(Some(&[I18N_SHARED.into()]))
                    .customize(|bundle| bundle.set_use_isolating(false))
                    .build()
                    .map_err(|e| Error::string(&e.to_string()))?,
            );
            info!("locales loaded");

            engines::TeraView::build_with_post_process(move |tera| {
                tera.register_function("t", FluentLoader::new(arc.clone()));
                tera.register_function("asset_url", asset_url);
                tera.register_function("org_url", org_url);
                Ok(())
            })?
        } else {
            engines::TeraView::build_with_post_process(|tera| {
                tera.register_function("asset_url", asset_url);
                tera.register_function("org_url", org_url);
                Ok(())
            })?
        };

        Ok(router.layer(Extension(ViewEngine::from(tera_engine))))
    }
}

/// `{{ asset_url(path="js/app.js") }}` gives `/static/js/app.js?v=<hash of the
/// file>`, so browsers fetch a file again only when it changed. Release builds
/// hash each file once; debug builds re-hash so edits show up straight away.
/// A missing file gets a plain URL.
pub fn asset_url(kwargs: tera::Kwargs, _: &tera::State) -> tera::TeraResult<String> {
    let path: &str = kwargs.must_get("path")?;
    let url = format!("/static/{path}");
    if cfg!(debug_assertions) {
        return Ok(file_version(path).map_or(url.clone(), |v| format!("{url}?v={v}")));
    }
    static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| tera::Error::message("asset cache poisoned"))?;
    let version = cache
        .entry(path.to_string())
        .or_insert_with(|| file_version(path))
        .clone();
    Ok(version.map_or(url.clone(), |v| format!("{url}?v={v}")))
}

/// `{{ org_url(path="/tasks/" ~ task.id) }}` gives `/o/{slug}/tasks/58`, from
/// the page's `org_base` (`CurrentMember::page` and `fragment` set it). A
/// template drawn without `org_base` fails to render, so a missed fragment
/// shows up in tests. An empty `org_base` (the admin page, which belongs to
/// no org) gives the bare path, which redirects to the last org.
///
/// # Errors
/// When `path` or `org_base` is missing.
pub fn org_url(kwargs: tera::Kwargs, state: &tera::State) -> tera::TeraResult<String> {
    let path: &str = kwargs.must_get("path")?;
    let Some(base) = state.get::<String>("org_base")? else {
        return Err(tera::Error::message(
            "org_url needs org_base: use member.page() or member.fragment()",
        ));
    };
    if base.is_empty() {
        return Ok(if links::is_org_relative(path) {
            path.to_string()
        } else {
            "/dashboard".to_string()
        });
    }
    Ok(links::org_path(
        base.strip_prefix("/o/").unwrap_or_default(),
        path,
    ))
}

/// A short hash of a file under `assets/static`, or `None` if it is missing.
fn file_version(path: &str) -> Option<String> {
    if path.contains("..") {
        return None;
    }
    let bytes = std::fs::read(Path::new(STATIC_DIR).join(path)).ok()?;
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    Some(format!("{:08x}", hasher.finish() as u32))
}

#[cfg(test)]
mod tests {
    use super::file_version;

    #[test]
    fn versions_follow_file_contents() {
        let version = file_version("favicon.svg").expect("favicon is committed");
        assert_eq!(version.len(), 8);
        assert_eq!(file_version("favicon.svg"), Some(version));
        assert_eq!(file_version("no-such-file.js"), None);
        assert_eq!(file_version("../Cargo.toml"), None);
    }
}
