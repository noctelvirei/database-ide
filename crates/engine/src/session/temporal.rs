//! Validated ISO 8601 text for the common calendar and clock range.
//!
//! shortcut: four-digit years and ordinary clock times only, replace with date/time types
//! when temporal arithmetic is needed; engines preserve infinity, BC dates, leap seconds,
//! MySQL zero dates and duration-valued TIME as Value::Other instead of losing them.

/// A temporal string outside the documented ISO 8601 representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// Invalid syntax, calendar date, clock time or UTC offset; input is not retained.
    #[error("Invalid ISO 8601 date or time")]
    InvalidIso8601,
}

macro_rules! temporal_text {
    ($name:ident, $validate:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $name(String);

        impl $name {
            /// Validates and copies text, preserving fractional precision and UTC offset.
            /// Returns InvalidIso8601 without retaining the input if validation fails.
            pub fn parse(text: &str) -> Result<Self, Error> {
                if $validate(text) {
                    Ok(Self(text.to_owned()))
                } else {
                    Err(Error::InvalidIso8601)
                }
            }

            /// The original validated ISO 8601 text, without normalization.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

temporal_text!(
    Date,
    date,
    "A Gregorian date: YYYY-MM-DD, years 0001 through 9999."
);
temporal_text!(
    Time,
    time,
    "A clock time: HH:MM:SS with optional decimal fraction; seconds 00 through 59."
);
temporal_text!(
    TimeWithZone,
    time_with_zone,
    "A clock time followed by Z or a signed HH:MM offset, up to 23:59."
);
temporal_text!(
    Timestamp,
    timestamp,
    "A date and clock time separated by T, without a UTC offset."
);
temporal_text!(
    TimestampWithZone,
    timestamp_with_zone,
    "A date and clock time separated by T, followed by Z or a signed HH:MM offset."
);

fn number(text: &str) -> Option<u32> {
    text.bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| text.parse().ok())
        .flatten()
}

fn date(text: &str) -> bool {
    if !text.is_ascii() || text.len() != 10 || &text[4..5] != "-" || &text[7..8] != "-" {
        return false;
    }
    let (Some(year), Some(month), Some(day)) =
        (number(&text[..4]), number(&text[5..7]), number(&text[8..]))
    else {
        return false;
    };
    let days = match month {
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => return false,
    };
    year != 0 && (1..=days).contains(&day)
}

fn time(text: &str) -> bool {
    if !text.is_ascii() || text.len() < 8 || &text[2..3] != ":" || &text[5..6] != ":" {
        return false;
    }
    let (Some(hour), Some(minute), Some(second)) =
        (number(&text[..2]), number(&text[3..5]), number(&text[6..8]))
    else {
        return false;
    };
    let fraction = &text[8..];
    hour < 24
        && minute < 60
        && second < 60
        && (fraction.is_empty()
            || (fraction.starts_with('.')
                && fraction.len() > 1
                && fraction[1..].bytes().all(|byte| byte.is_ascii_digit())))
}

fn time_with_zone(text: &str) -> bool {
    if let Some(clock) = text.strip_suffix('Z') {
        return time(clock);
    }
    if !text.is_ascii() || text.len() < 14 {
        return false;
    }
    let (clock, offset) = text.split_at(text.len() - 6);
    if !matches!(offset.as_bytes()[0], b'+' | b'-') || &offset[3..4] != ":" {
        return false;
    }
    time(clock)
        && number(&offset[1..3]).is_some_and(|hour| hour < 24)
        && number(&offset[4..]).is_some_and(|minute| minute < 60)
}

fn timestamp(text: &str) -> bool {
    text.split_once('T')
        .is_some_and(|(day, clock)| date(day) && time(clock))
}

fn timestamp_with_zone(text: &str) -> bool {
    text.split_once('T')
        .is_some_and(|(day, clock)| date(day) && time_with_zone(clock))
}
