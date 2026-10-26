//! Juniper Tea Co, the made-up team the chat delete tests share.
use collab::models::{
    conversations,
    memberships::{self, role},
};
use loco_rs::{app::AppContext, TestServer};
use sea_orm::{ActiveModelTrait, ActiveValue, IntoActiveModel};

use super::{
    chat::user_id,
    chat_photos::group_of,
    prepare_data::{approve_as_owner, join, sign_up_member, TestMember},
};

/// Juniper Tea Co: Ana (owner), Maya (admin), Tom and Leo (members). #finance
/// is Ana's group with Maya and Tom; Leo is not in it.
pub(super) struct Team {
    pub(super) org_id: i64,
    pub(super) ana: TestMember,
    pub(super) maya: TestMember,
    pub(super) tom: TestMember,
    pub(super) leo: TestMember,
    pub(super) ana_id: i64,
    pub(super) maya_id: i64,
    pub(super) tom_id: i64,
    pub(super) leo_id: i64,
    pub(super) general: conversations::Model,
    pub(super) finance: conversations::Model,
}

pub(super) async fn juniper(request: &TestServer, ctx: &AppContext) -> Team {
    let ana = sign_up_member(request, "Juniper Tea Co", "ana", "ana@example.com").await;
    let org_id = ana.org_id;
    let mut people = Vec::new();
    for name in ["maya", "tom", "leo"] {
        let email = format!("{name}@example.com");
        let who = join(request, ctx, &ana.slug, name, &email).await;
        let id = user_id(ctx, &email).await;
        let pending = memberships::Model::default_for_user(&ctx.db, id)
            .await
            .unwrap()
            .unwrap();
        let approved = approve_as_owner(ctx, &pending).await;
        if name == "maya" {
            let mut admin = approved.into_active_model();
            admin.role = ActiveValue::Set(role::ADMIN.to_string());
            admin.update(&ctx.db).await.unwrap();
        }
        people.push((who, id));
    }
    let [(maya, maya_id), (tom, tom_id), (leo, leo_id)] = people.try_into().unwrap();
    let ana_id = user_id(ctx, "ana@example.com").await;
    let general = conversations::Model::find_general(&ctx.db, org_id)
        .await
        .unwrap();
    let finance = group_of(ctx, org_id, ana_id, "finance", vec![maya_id, tom_id]).await;
    Team {
        org_id,
        ana,
        maya,
        tom,
        leo,
        ana_id,
        maya_id,
        tom_id,
        leo_id,
        general,
        finance,
    }
}
