pub mod _entities;
pub mod conversation_members;
pub mod conversations;
pub mod meeting_attendees;
pub mod meetings;
pub mod memberships;
pub mod messages;
pub mod organisations;
pub mod projects;
pub mod reactions;
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
pub mod access_tokens;
pub mod auth_attempts;
pub mod claude_update_notices;
pub mod documents;
pub mod folders;
pub mod guide;
pub mod login_accesses;
pub mod login_events;
pub mod logins;
pub mod note_reactions;
pub mod notifications;
pub mod oauth_clients;
pub mod oauth_codes;
pub mod org_keys;
pub mod project_owners;
pub mod saved_views;
pub mod secrets;
pub mod task_assignees;

/// Invisible characters that can make text read differently from what it is:
/// direction marks and overrides (a right-to-left override shows
/// "photo[RLO]gpj.exe" as "photoexe.jpg"), zero-width characters, and Unicode
/// line and paragraph separators.
const HIDDEN: [char; 18] = [
    '\u{061C}', '\u{200B}', '\u{200C}', '\u{200D}', '\u{200E}', '\u{200F}', '\u{202A}', '\u{202B}',
    '\u{202C}', '\u{202D}', '\u{202E}', '\u{2028}', '\u{2029}', '\u{2066}', '\u{2067}', '\u{2068}',
    '\u{2069}', '\u{FEFF}',
];

/// Text without control or hidden characters. `keep_newlines` keeps line
/// breaks, for multi-line notes.
#[must_use]
pub fn visible_text(text: &str, keep_newlines: bool) -> String {
    text.chars()
        .filter(|c| (keep_newlines && *c == '\n') || !(c.is_control() || HIDDEN.contains(c)))
        .collect()
}
