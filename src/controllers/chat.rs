use loco_rs::prelude::*;

use crate::extractors::current_member::CurrentMember;

#[debug_handler]
async fn index(
    member: CurrentMember,
    State(_ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    format::render().view(&v, "chat/index.html", member.page("chat", data!({})))
}

pub fn routes() -> Routes {
    Routes::new().add("/chat", get(index))
}
