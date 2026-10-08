//! Showing times in an organisation's time zone. Everything is stored in UTC;
//! these turn it into what people read, with BST and GMT handled by the zone.
use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use chrono_tz::Tz;

/// The zone for an IANA name, or London when the name isn't known.
#[must_use]
pub fn zone(name: &str) -> Tz {
    name.parse().unwrap_or(chrono_tz::Europe::London)
}

/// A stored time in the organisation's zone.
#[must_use]
pub fn local(at: DateTime<FixedOffset>, tz: Tz) -> DateTime<Tz> {
    at.with_timezone(&tz)
}

/// Today's date where the organisation is.
#[must_use]
pub fn today(tz: Tz) -> NaiveDate {
    Utc::now().with_timezone(&tz).date_naive()
}

/// Every zone for the settings picker, e.g. "Europe/London".
#[must_use]
pub fn all_names() -> Vec<&'static str> {
    chrono_tz::TZ_VARIANTS.iter().map(|tz| tz.name()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn london_follows_summer_time() {
        let summer = DateTime::parse_from_rfc3339("2026-10-08T17:00:00Z").unwrap();
        let winter = DateTime::parse_from_rfc3339("2026-12-08T17:00:00Z").unwrap();
        let london = zone("Europe/London");
        assert_eq!(
            local(summer, london).format("%H:%M %Z").to_string(),
            "18:00 BST"
        );
        assert_eq!(
            local(winter, london).format("%H:%M %Z").to_string(),
            "17:00 GMT"
        );
        assert_eq!(zone("Not/AZone"), london);
        assert!(all_names().contains(&"America/New_York"));
    }
}
