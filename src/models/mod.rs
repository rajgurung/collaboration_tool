pub mod _entities;
pub mod conversation_members;
pub mod conversations;
pub mod meeting_attendees;
pub mod meetings;
pub mod memberships;
pub mod messages;
pub mod organisations;
pub mod projects;
pub mod task_notes;
pub mod tasks;
pub mod users;

use std::collections::{BTreeMap, HashMap};

use loco_rs::{
    model::ModelError,
    validation::{ModelValidationErrors, ValidationError},
};

/// A validation error for one field, shaped like the ones `Validatable` produces,
/// for rules that need the database (taken usernames, owners outside the org).
#[must_use]
pub fn field_error(field: &str, message: &str) -> ModelError {
    ModelError::Validation(ModelValidationErrors {
        errors: BTreeMap::from([(
            field.to_string(),
            vec![ValidationError {
                code: "invalid".to_string(),
                message: Some(message.to_string()),
                params: HashMap::new(),
            }],
        )]),
    })
}
pub mod guide;
pub mod notifications;
pub mod saved_views;
pub mod task_assignees;
