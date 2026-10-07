use loco_rs::prelude::*;
use serde::Serialize;

use crate::{
    extractors::{current_user::CurrentUser, session::session_cookie},
    models::{memberships, organisations, users::RegisterParams},
    views::forms::{field_errors, FieldErrors},
};

#[derive(Debug, Default, Serialize)]
struct FormValues<'a> {
    name: &'a str,
    email: &'a str,
}

#[debug_handler]
async fn new(
    user: Option<CurrentUser>,
    ViewEngine(v): ViewEngine<TeraView>,
    State(ctx): State<AppContext>,
    Path(slug): Path<String>,
) -> Result<Response> {
    if user.is_some() {
        return format::redirect("/dashboard");
    }
    let Ok(org) = organisations::Model::find_by_slug(&ctx.db, &slug).await else {
        return not_found_page(&v);
    };
    render_form(&v, &org, &FormValues::default(), &FieldErrors::new(), 200)
}

#[debug_handler]
async fn create(
    ViewEngine(v): ViewEngine<TeraView>,
    State(ctx): State<AppContext>,
    Path(slug): Path<String>,
    Form(params): Form<RegisterParams>,
) -> Result<Response> {
    let Ok(org) = organisations::Model::find_by_slug(&ctx.db, &slug).await else {
        return not_found_page(&v);
    };
    let values = FormValues {
        name: &params.name,
        email: &params.email,
    };
    match memberships::Model::join(&ctx.db, &org, &params).await {
        Ok((user, _membership)) => format::render()
            .cookies(&[session_cookie(&ctx, &user)?])?
            .redirect("/dashboard"),
        Err(ModelError::EntityAlreadyExists) => {
            let errors = FieldErrors::from([(
                "email".to_string(),
                "An account with this email already exists. Sign in instead.".to_string(),
            )]);
            render_form(&v, &org, &values, &errors, 422)
        }
        Err(err) => match field_errors(&err) {
            Some(errors) => render_form(&v, &org, &values, &errors, 422),
            None => Err(err.into()),
        },
    }
}

fn render_form(
    v: &TeraView,
    org: &organisations::Model,
    values: &FormValues,
    errors: &FieldErrors,
    status: u16,
) -> Result<Response> {
    format::render().status(status).view(
        v,
        "join/new.html",
        data!({
            "org": { "name": org.name, "slug": org.slug },
            "form": values,
            "errors": errors,
        }),
    )
}

fn not_found_page(v: &TeraView) -> Result<Response> {
    format::render()
        .status(404)
        .view(v, "errors/not_found.html", data!({}))
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/join/{slug}", get(new))
        .add("/join/{slug}", post(create))
}
