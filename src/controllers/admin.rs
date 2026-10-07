use loco_rs::prelude::*;

use crate::{extractors::current_user::CurrentUser, views::layout::avatar_color};

/// Platform admin. Anyone who is not the super admin gets a plain 404, so the
/// page's existence is not advertised.
#[debug_handler]
async fn index(
    CurrentUser(user): CurrentUser,
    State(_ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    if !user.is_super_admin {
        return Err(Error::NotFound);
    }
    format::render().view(
        &v,
        "admin/index.html",
        data!({
            "active": "admin",
            "org": { "name": "Platform admin" },
            "me": {
                "username": user.name,
                "color": avatar_color(&user.name),
                "is_super_admin": true,
            },
        }),
    )
}

pub fn routes() -> Routes {
    Routes::new().add("/admin", get(index))
}
