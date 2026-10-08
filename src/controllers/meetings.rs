use std::collections::HashMap;

use axum::http::HeaderMap;
use loco_rs::prelude::*;

use crate::{
    extractors::{current_member::CurrentMember, session::redirect_response},
    models::{
        meetings::{self, MeetingParams},
        memberships,
    },
    views::{
        forms::{field_errors, invalid_form, toast, FieldErrors},
        layout::{avatar_color, Person},
        time,
    },
};

const FORM_ID: &str = "meeting-form";

async fn list(ctx: &AppContext, org_id: i64) -> Result<Vec<serde_json::Value>> {
    let names: HashMap<i64, String> = memberships::Model::team(&ctx.db, org_id)
        .await?
        .into_iter()
        .collect();
    Ok(meetings::Model::list_for_org(&ctx.db, org_id)
        .await?
        .into_iter()
        .map(|(m, ids)| {
            let attendees: Vec<serde_json::Value> = ids
                .iter()
                .filter_map(|id| names.get(id))
                .map(|name| serde_json::json!({ "name": name, "color": avatar_color(name) }))
                .collect();
            serde_json::json!({
                "id": m.id,
                "title": m.title,
                "held_on": m.held_on.format("%d %b %Y").to_string(),
                "summary": m.summary,
                "decisions": m.decisions,
                "attendees": attendees,
            })
        })
        .collect())
}

#[debug_handler]
async fn index(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let meetings = list(&ctx, member.org.id).await?;
    format::render().view(
        &v,
        "meetings/index.html",
        member.page("meetings", data!({ "meetings": meetings })),
    )
}

async fn form_data(
    ctx: &AppContext,
    member: &CurrentMember,
    values: serde_json::Value,
    errors: &FieldErrors,
) -> Result<serde_json::Value> {
    let team = Person::from_team(memberships::Model::team(&ctx.db, member.org.id).await?);
    Ok(member.page(
        "meetings",
        data!({ "form": values, "errors": errors, "team": team }),
    ))
}

#[debug_handler]
async fn new(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    let values = serde_json::json!({
        "title": "Team weekly",
        "held_on": time::today(member.tz()).format("%Y-%m-%d").to_string(),
        "summary": "",
        "decisions": "",
        "attendee_ids": [member.user.id],
    });
    let data = form_data(&ctx, &member, values, &FieldErrors::new()).await?;
    format::render().view(&v, "meetings/_form.html", data)
}

#[debug_handler]
async fn create(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    axum_extra::extract::Form(params): axum_extra::extract::Form<MeetingParams>,
) -> Result<Response> {
    match meetings::Model::create(&ctx.db, member.org.id, member.user.id, &params).await {
        Ok(_) if !headers.contains_key("hx-request") => {
            Ok(redirect_response(&headers, "/meetings"))
        }
        Ok(_) => {
            let meetings = list(&ctx, member.org.id).await?;
            format::render()
                .header("HX-Trigger", toast("success", "Meeting saved"))
                .view(&v, "meetings/_list.html", data!({ "meetings": meetings }))
        }
        Err(err) => {
            let errors = field_errors(&err).ok_or(err)?;
            let values = serde_json::json!({
                "title": params.title, "held_on": params.held_on, "summary": params.summary,
                "decisions": params.decisions, "attendee_ids": params.attendee_ids,
            });
            let data = form_data(&ctx, &member, values, &errors).await?;
            invalid_form(&v, "meetings/_form.html", FORM_ID, data)
        }
    }
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/meetings", get(index))
        .add("/meetings", post(create))
        .add("/meetings/new", get(new))
}
