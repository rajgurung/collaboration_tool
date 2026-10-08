//! Saved views on the Tasks page: a name for a setup (whose tasks, lanes,
//! board or list, search) that brings it back in one tap.
use axum::http::HeaderMap;
use loco_rs::prelude::*;
use serde::Deserialize;

use crate::{
    extractors::{current_member::CurrentMember, session::redirect_response},
    models::saved_views::{self, Setup, ViewParams},
    views::forms::{field_errors, invalid_form, FieldErrors},
};

const FORM_ID: &str = "view-form";

/// The save form, describing the setup about to be saved.
#[debug_handler]
async fn new(
    _member: CurrentMember,
    State(_ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Query(setup): Query<Setup>,
) -> Result<Response> {
    let setup = setup.cleaned();
    format::render().view(
        &v,
        "tasks/_view_form.html",
        data!({ "view": null, "name": "", "is_default": false, "setup": setup, "errors": FieldErrors::new() }),
    )
}

#[debug_handler]
async fn create(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    Form(params): Form<ViewParams>,
) -> Result<Response> {
    match saved_views::Model::create(&ctx.db, member.org.id, member.user.id, &params).await {
        Ok(view) => Ok(redirect_response(&headers, &view.setup().url())),
        Err(err) => {
            let errors = field_errors(&err).ok_or(err)?;
            invalid_form(
                &v,
                "tasks/_view_form.html",
                FORM_ID,
                data!({
                    "view": null, "name": params.name, "is_default": params.is_default.is_some(),
                    "setup": params.setup.cleaned(), "errors": errors,
                }),
            )
        }
    }
}

/// Rename, set as default, update to the current setup, or delete. `setup` is
/// what the page shows now.
#[debug_handler]
async fn edit(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    Path(id): Path<i64>,
    Query(setup): Query<Setup>,
) -> Result<Response> {
    let view = saved_views::Model::find_mine(&ctx.db, member.org.id, member.user.id, id).await?;
    let current = setup.cleaned();
    format::render().view(
        &v,
        "tasks/_view_form.html",
        data!({
            "view": { "id": view.id, "same": view.setup() == current },
            "name": view.name, "is_default": view.is_default,
            "setup": current, "errors": FieldErrors::new(),
        }),
    )
}

#[derive(Debug, Deserialize)]
struct EditForm {
    name: String,
    #[serde(default)]
    is_default: Option<String>,
}

#[debug_handler]
async fn update(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(form): Form<EditForm>,
) -> Result<Response> {
    let view = saved_views::Model::find_mine(&ctx.db, member.org.id, member.user.id, id).await?;
    let setup = view.setup();
    match view
        .edit(&ctx.db, &form.name, form.is_default.is_some())
        .await
    {
        Ok(view) => Ok(redirect_response(&headers, &view.setup().url())),
        Err(err) => {
            let errors = field_errors(&err).ok_or(err)?;
            invalid_form(
                &v,
                "tasks/_view_form.html",
                FORM_ID,
                data!({
                    "view": { "id": id, "same": true }, "name": form.name,
                    "is_default": form.is_default.is_some(), "setup": setup, "errors": errors,
                }),
            )
        }
    }
}

/// Saves the page's current setup into the view.
#[debug_handler]
async fn update_setup(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(setup): Form<Setup>,
) -> Result<Response> {
    let view = saved_views::Model::find_mine(&ctx.db, member.org.id, member.user.id, id).await?;
    let view = view.update_setup(&ctx.db, &setup).await?;
    Ok(redirect_response(&headers, &view.setup().url()))
}

/// Deletes the view and stays on the current setup.
#[debug_handler]
async fn destroy(
    member: CurrentMember,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(setup): Form<Setup>,
) -> Result<Response> {
    let view = saved_views::Model::find_mine(&ctx.db, member.org.id, member.user.id, id).await?;
    view.remove(&ctx.db).await?;
    Ok(redirect_response(&headers, &setup.cleaned().url()))
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/tasks/views/new", get(new))
        .add("/tasks/views", post(create))
        .add("/tasks/views/{id}/edit", get(edit))
        .add("/tasks/views/{id}", post(update))
        .add("/tasks/views/{id}/setup", post(update_setup))
        .add("/tasks/views/{id}/delete", post(destroy))
}
