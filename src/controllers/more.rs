use loco_rs::prelude::*;

use crate::{extractors::current_member::CurrentMember, models::memberships};

/// The mobile "More" tab: links that do not fit in the bottom bar.
#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let waiting = if member.can_manage() {
        memberships::Model::list_for_org(&ctx.db, member.org.id)
            .await?
            .iter()
            .filter(|(m, _)| m.is_pending())
            .count()
    } else {
        0
    };
    format::render().view(
        &v,
        "more/index.html",
        member.page("more", data!({ "waiting": waiting })),
    )
}

pub fn routes() -> Routes {
    Routes::new().add("/more", get(index))
}
