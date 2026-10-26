use loco_rs::prelude::*;

use crate::{data::settings::Settings, extractors::current_user::CurrentUser};

/// Landing page for visitors; signed-in users go straight to their workspace.
#[debug_handler]
async fn index(
    user: Option<CurrentUser>,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    if user.is_some() {
        return format::redirect("/dashboard");
    }
    // The app also answers on its Railway address. Naming APP_URL tells
    // search engines which address to index.
    let canonical = format!(
        "{}/",
        Settings::from_context(&ctx)?.app_url.trim_end_matches('/')
    );
    format::render().view(&v, "home/index.html", data!({ "canonical": canonical }))
}

/// App addresses name organisations, so crawlers stay out. `/join/` is left
/// open on purpose: a crawler has to fetch a join page to see its `noindex`.
const ROBOTS: &str =
    "User-agent: *\nDisallow: /o/\nDisallow: /oauth/\nDisallow: /admin\nDisallow: /mcp\n";

#[debug_handler]
async fn robots() -> Response {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        ROBOTS,
    )
        .into_response()
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/", get(index))
        .add("/robots.txt", get(robots))
}
