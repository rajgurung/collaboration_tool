use loco_rs::prelude::*;

use crate::extractors::current_member::CurrentMember;

#[debug_handler]
async fn index(
    member: CurrentMember,
    State(_ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    format::render().view(
        &v,
        "meetings/index.html",
        member.page("meetings", data!({})),
    )
}

pub fn routes() -> Routes {
    Routes::new().add("/meetings", get(index))
}
