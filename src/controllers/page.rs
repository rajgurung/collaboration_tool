use loco_rs::prelude::*;

use crate::extractors::current_user::CurrentUser;

/// Landing page for visitors; signed-in users go straight to their workspace.
#[debug_handler]
async fn index(
    user: Option<CurrentUser>,
    State(_ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    if user.is_some() {
        return format::redirect("/dashboard");
    }
    format::render().view(&v, "home/index.html", data!({}))
}

pub fn routes() -> Routes {
    Routes::new().add("/", get(index))
}
