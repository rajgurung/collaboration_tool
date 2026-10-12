use loco_rs::prelude::*;
use serde::Deserialize;

use crate::{extractors::current_member::CurrentMember, models::memberships};

#[derive(Debug, Deserialize)]
struct MoreQuery {
    signed_out: Option<String>,
}

/// The mobile "More" tab: links that do not fit in the bottom bar.
#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Query(query): Query<MoreQuery>,
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
        member.page(
            "more",
            data!({
                "waiting": waiting,
                "flash": query.signed_out.map(|_| data!({
                    "kind": "success",
                    "message": "Signed out on every other device.",
                })),
            }),
        ),
    )
}

pub fn routes() -> Routes {
    Routes::new().add("/more", get(index))
}
