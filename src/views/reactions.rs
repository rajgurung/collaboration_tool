//! Reaction chips and picks, shared by chat messages and task notes.
use std::collections::HashMap;

use serde::Serialize;

use crate::{models::reactions::EMOJI, views::layout::avatar_color};

/// Shown for someone who has left the team; their reactions still count.
pub const FORMER_MEMBER: &str = "Former member";

/// One person's reaction, with the name the team knows them by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReactionRow {
    pub emoji: String,
    pub user_id: i64,
    pub username: String,
}

impl ReactionRow {
    /// A row named for the team, or "Former member" when the person has left.
    #[must_use]
    pub fn named(emoji: String, user_id: i64, names: &HashMap<i64, String>) -> Self {
        Self {
            username: names
                .get(&user_id)
                .cloned()
                .unwrap_or_else(|| FORMER_MEMBER.to_string()),
            emoji,
            user_id,
        }
    }
}

/// Someone who reacted, as the viewer sees them: `name` is "You" for the
/// viewer, and the avatar still uses their username.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reactor {
    pub name: String,
    pub username: String,
    pub color: &'static str,
}

/// One emoji under a message, with its count and who chose it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReactionChip {
    pub emoji: String,
    pub count: usize,
    /// The viewer is one of them.
    pub mine: bool,
    /// "You" first, then others in the order they reacted.
    pub people: Vec<Reactor>,
    /// `people` as one line, for the sheet.
    pub names: String,
    /// The chip's screen-reader label, such as "👍 3, you reacted".
    pub label: String,
}

/// One of the six choices on the toolbar and in the sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pick {
    pub emoji: &'static str,
    pub mine: bool,
}

/// Reactions grouped into chips for one viewer, in the fixed [`EMOJI`] order
/// so chips never move around.
#[must_use]
pub fn chips(rows: &[ReactionRow], viewer_id: i64) -> Vec<ReactionChip> {
    EMOJI
        .iter()
        .filter_map(|emoji| {
            let of: Vec<&ReactionRow> = rows.iter().filter(|r| r.emoji == *emoji).collect();
            if of.is_empty() {
                return None;
            }
            let reactor = |r: &ReactionRow, name: &str| Reactor {
                name: name.to_string(),
                color: avatar_color(&r.username),
                username: r.username.clone(),
            };
            let mut people: Vec<Reactor> = of
                .iter()
                .filter(|r| r.user_id == viewer_id)
                .map(|r| reactor(r, "You"))
                .collect();
            let mine = !people.is_empty();
            people.extend(
                of.iter()
                    .filter(|r| r.user_id != viewer_id)
                    .map(|r| reactor(r, &r.username)),
            );
            let count = of.len();
            Some(ReactionChip {
                emoji: (*emoji).to_string(),
                count,
                mine,
                names: people
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                people,
                label: if mine {
                    format!("{emoji} {count}, you reacted")
                } else {
                    format!("{emoji} {count}")
                },
            })
        })
        .collect()
}

/// The six choices, marking the ones the viewer has already made.
#[must_use]
pub fn picks(chips: &[ReactionChip]) -> Vec<Pick> {
    EMOJI
        .iter()
        .map(|emoji| Pick {
            emoji,
            mine: chips.iter().any(|c| c.mine && c.emoji == *emoji),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(emoji: &str, user_id: i64, username: &str) -> ReactionRow {
        ReactionRow {
            emoji: emoji.to_string(),
            user_id,
            username: username.to_string(),
        }
    }

    #[test]
    fn chips_keep_the_fixed_order_and_put_you_first() {
        // Reacted out of order: ✅, then 👍 by bob, carol and alice.
        let rows = [
            row("✅", 2, "bob"),
            row("👍", 2, "bob"),
            row("👍", 3, "carol"),
            row("👍", 1, "alice"),
        ];
        let chips = chips(&rows, 1);
        let order: Vec<&str> = chips.iter().map(|c| c.emoji.as_str()).collect();
        assert_eq!(order, vec!["👍", "✅"]);
        assert_eq!(chips[0].count, 3);
        assert!(chips[0].mine);
        assert_eq!(chips[0].names, "You, bob, carol");
        assert_eq!(
            chips[0].people[0].username, "alice",
            "the avatar is still alice's"
        );
        assert_eq!(chips[0].label, "👍 3, you reacted");
        assert!(!chips[1].mine);
        assert_eq!(chips[1].label, "✅ 1");

        // Bob sees the same chips with himself first.
        let bobs = super::chips(&rows, 2);
        assert_eq!(bobs[0].names, "You, carol, alice");
        assert!(bobs[1].mine);
    }

    #[test]
    fn picks_mark_only_your_own_reactions() {
        let rows = [row("❤️", 2, "bob"), row("🎉", 1, "alice")];
        let picks = picks(&chips(&rows, 1));
        assert_eq!(picks.len(), 6);
        let mine: Vec<&str> = picks.iter().filter(|p| p.mine).map(|p| p.emoji).collect();
        assert_eq!(mine, vec!["🎉"]);
        assert!(super::picks(&[]).iter().all(|p| !p.mine));
    }

    #[test]
    fn people_off_the_team_show_as_former_members() {
        let names: HashMap<i64, String> = [(1, "alice".to_string())].into_iter().collect();
        assert_eq!(ReactionRow::named("👍".into(), 1, &names).username, "alice");
        let gone = ReactionRow::named("👍".into(), 9, &names);
        assert_eq!(gone.username, "Former member");
        assert_eq!(gone.user_id, 9);
    }
}
