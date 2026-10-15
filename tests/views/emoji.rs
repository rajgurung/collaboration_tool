//! Guards the emoji picker's list in `frontend/emoji.ts`, so a later edit
//! cannot add an emoji that draws as a box or as text on some phones.
use std::collections::HashSet;

use collab::models::reactions::EMOJI;

const SOURCE: &str = include_str!("../../frontend/emoji.ts");

/// Characters below U+3300 that draw as emoji without U+FE0F.
const DEFAULT_EMOJI: &str = "✅❌❓❗⭐⚡⚽⚾⏰⌛⏳✨➕➖⛔☕⛄⛳⛺⛵✊✋⭕";

/// The first quoted string of each `["😀", "name"]` entry.
fn entries() -> Vec<&'static str> {
    SOURCE
        .split("[\"")
        .skip(1)
        .map(|rest| rest.split('"').next().unwrap())
        .collect()
}

#[test]
fn the_picker_has_about_two_hundred_emoji_with_no_repeats() {
    let list = entries();
    assert!((180..=220).contains(&list.len()), "{} emoji", list.len());
    let unique: HashSet<&str> = list.iter().copied().collect();
    assert_eq!(unique.len(), list.len(), "an emoji is listed twice");
    for emoji in EMOJI {
        assert!(
            unique.contains(emoji),
            "{emoji} is one of the six reactions"
        );
    }
}

#[test]
fn every_emoji_draws_as_one_emoji_on_any_recent_phone() {
    for emoji in entries() {
        let chars: Vec<char> = emoji.chars().collect();
        for (i, c) in chars.iter().enumerate() {
            let code = u32::from(*c);
            assert_ne!(code, 0x200D, "{emoji}: no ZWJ sequences");
            assert!(
                !(0x1F3FB..=0x1F3FF).contains(&code),
                "{emoji}: no skin tones"
            );
            assert!(!(0x1F1E6..=0x1F1FF).contains(&code), "{emoji}: no flags");
            if code < 0x3300 && !DEFAULT_EMOJI.contains(*c) {
                assert_eq!(
                    chars.get(i + 1),
                    Some(&'\u{FE0F}'),
                    "{emoji} (U+{code:04X}) draws as text without U+FE0F"
                );
            }
        }
    }
}
