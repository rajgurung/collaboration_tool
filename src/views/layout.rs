/// Colours from the original design, assigned to people by name.
const AVATAR_COLORS: [&str; 6] = [
    "#ffb454", "#df85ff", "#72e5b4", "#6cb6ff", "#ff758f", "#c9d76a",
];

/// The same name always gets the same colour.
#[must_use]
pub fn avatar_color(name: &str) -> &'static str {
    let sum = name.to_lowercase().bytes().fold(0usize, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(usize::from(b))
    });
    AVATAR_COLORS[sum % AVATAR_COLORS.len()]
}
