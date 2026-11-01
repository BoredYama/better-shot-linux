//! `Date` as Foundation's `JSONEncoder` writes it by default: seconds since
//! 2001-01-01T00:00:00Z as a floating-point number. Files shared with the macOS
//! app (history, projects) use this encoding.

use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Deserializer, Serializer};

/// Seconds between the Unix epoch and Foundation's reference date.
pub const REFERENCE_DATE_UNIX: i64 = 978_307_200;

pub fn to_reference_seconds(date: &DateTime<Utc>) -> f64 {
    (date.timestamp() - REFERENCE_DATE_UNIX) as f64 + f64::from(date.timestamp_subsec_nanos()) / 1e9
}

pub fn from_reference_seconds(seconds: f64) -> Option<DateTime<Utc>> {
    if !seconds.is_finite() {
        return None;
    }
    let whole = seconds.floor();
    let nanos = ((seconds - whole) * 1e9).round().min(999_999_999.0) as u32;
    Utc.timestamp_opt(whole as i64 + REFERENCE_DATE_UNIX, nanos).single()
}

pub fn serialize<S: Serializer>(date: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_f64(to_reference_seconds(date))
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<DateTime<Utc>, D::Error> {
    let seconds = f64::deserialize(deserializer)?;
    from_reference_seconds(seconds).ok_or_else(|| serde::de::Error::custom("date out of range"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_reference_dates() {
        let date = from_reference_seconds(0.0).unwrap();
        assert_eq!(date.to_rfc3339(), "2001-01-01T00:00:00+00:00");
        let later = from_reference_seconds(780_000_000.25).unwrap();
        assert_eq!(to_reference_seconds(&later), 780_000_000.25);
        assert!(from_reference_seconds(f64::NAN).is_none());
    }
}
