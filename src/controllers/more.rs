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
    // "Switch" needs somewhere to switch to. While acting the acting org is
    // one, so any org of their own counts.
    let (active, pending) = memberships::Model::switcher_counts(&ctx.db, member.user.id).await?;
    let can_switch = active + pending >= if member.acting { 1 } else { 2 };
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
                "can_switch": can_switch,
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
