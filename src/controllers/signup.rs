use loco_rs::prelude::*;
use serde::Serialize;

use crate::{
    extractors::{current_user::CurrentUser, session::session_cookie},
    models::organisations::{self, SignupParams},
    views::forms::{field_errors, FieldErrors},
};

/// What the form shows back after a failed submit. Never includes the password.
#[derive(Debug, Default, Serialize)]
struct FormValues<'a> {
    organisation_name: &'a str,
    name: &'a str,
    email: &'a str,
}

#[debug_handler]
async fn new(
    user: Option<CurrentUser>,
    State(_ctx): State<AppContext>,
    ViewEngine(v): ViewEngine<TeraView>,
) -> Result<Response> {
    if user.is_some() {
        return format::redirect("/dashboard");
    }
    render_form(&v, &FormValues::default(), &FieldErrors::new(), 200)
}

#[debug_handler]
async fn create(
    ViewEngine(v): ViewEngine<TeraView>,
    State(ctx): State<AppContext>,
    Form(params): Form<SignupParams>,
) -> Result<Response> {
    let values = FormValues {
        organisation_name: &params.organisation_name,
        name: &params.name,
        email: &params.email,
    };
    match organisations::Model::sign_up(&ctx.db, &params).await {
        Ok((user, _org)) => format::render()
            .cookies(&[session_cookie(&ctx, &user)?])?
            .redirect("/dashboard"),
        Err(ModelError::EntityAlreadyExists) => {
            let errors = FieldErrors::from([(
                "email".to_string(),
                "An account with this email already exists. Sign in instead.".to_string(),
            )]);
            render_form(&v, &values, &errors, 422)
        }
        Err(err) => match field_errors(&err) {
            Some(errors) => render_form(&v, &values, &errors, 422),
            None => Err(err.into()),
        },
    }
}

fn render_form(
    v: &TeraView,
    values: &FormValues,
    errors: &FieldErrors,
    status: u16,
) -> Result<Response> {
    format::render().status(status).view(
        v,
        "signup/new.html",
        data!({ "form": values, "errors": errors }),
    )
}

pub fn routes() -> Routes {
    Routes::new()
        .add("/signup", get(new))
        .add("/signup", post(create))
}
