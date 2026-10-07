//! Turns model validation failures into one message per form field.
use std::collections::BTreeMap;

use loco_rs::{
    model::ModelError,
    validation::{ModelValidationErrors, ModelValidationMessage},
};

pub type FieldErrors = BTreeMap<String, String>;

/// Per-field messages for a validation failure, or `None` for any other error.
///
/// Validation can fail in two places: on params checked before a query
/// (`ModelError::Validation`), or in `before_save`, where Loco serializes the
/// errors into a `DbErr::Custom` JSON string.
#[must_use]
pub fn field_errors(err: &ModelError) -> Option<FieldErrors> {
    match err {
        ModelError::Validation(errors) => Some(from_validation(errors)),
        ModelError::DbErr(sea_orm::DbErr::Custom(json)) => {
            serde_json::from_str::<BTreeMap<String, Vec<ModelValidationMessage>>>(json)
                .ok()
                .map(|fields| {
                    fields
                        .into_iter()
                        .filter_map(|(field, messages)| {
                            let first = messages.into_iter().next()?;
                            Some((field, first.message.unwrap_or(first.code)))
                        })
                        .collect()
                })
        }
        _ => None,
    }
}

fn from_validation(errors: &ModelValidationErrors) -> FieldErrors {
    errors
        .errors
        .iter()
        .filter_map(|(field, list)| {
            let first = list.first()?;
            Some((
                field.clone(),
                first.message.clone().unwrap_or_else(|| first.code.clone()),
            ))
        })
        .collect()
}
