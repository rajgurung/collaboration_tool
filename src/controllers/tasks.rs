use loco_rs::prelude::*;

use crate::extractors::current_member::CurrentMember;

#[debug_handler]
async fn index(
    member: CurrentMember,
    State(_ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    format::render().view(&v, "tasks/index.html", member.page("tasks", data!({})))
}

pub fn routes() -> Routes {
    Routes::new().add("/tasks", get(index))
}
