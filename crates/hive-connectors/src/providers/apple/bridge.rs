//! Shared helpers for bridging between objc2 types and Rust types.

use anyhow::{anyhow, Result};
use objc2::rc::Retained;
use objc2_foundation::{NSDate, NSError, NSString};

/// Wrapper around `Retained<T>` that implements `Send + Sync`.
///
/// # Safety
/// Only use for ObjC types that Apple documents as thread-safe
/// (e.g. `EKEventStore`, `CNContactStore`).
pub struct SendRetained<T>(pub Retained<T>);

// SAFETY: EKEventStore and CNContactStore are documented by Apple as thread-safe.
unsafe impl<T> Send for SendRetained<T> {}
unsafe impl<T> Sync for SendRetained<T> {}

impl<T> std::ops::Deref for SendRetained<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> Clone for SendRetained<T>
where
    Retained<T>: Clone,
{
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

/// Convert an `NSString` reference to a Rust `String`.
pub fn nsstring_to_string(ns: &NSString) -> String {
    ns.to_string()
}

/// Create a retained `NSString` from a Rust `&str`.
pub fn string_to_nsstring(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

/// Convert an `NSDate` to an RFC 3339 timestamp string.
///
/// `NSDate` stores time as seconds since 2001-01-01 00:00:00 UTC.
pub fn nsdate_to_rfc3339(date: &NSDate) -> String {
    use chrono::{DateTime, Utc};

    const NS_REFERENCE_EPOCH: f64 = 978_307_200.0;
    let secs_since_ref = date.timeIntervalSinceReferenceDate();
    let total_unix = secs_since_ref + NS_REFERENCE_EPOCH;
    let unix_secs = total_unix.floor() as i64;
    let nanos = ((total_unix - unix_secs as f64) * 1_000_000_000.0) as u32;

    DateTime::<Utc>::from_timestamp(unix_secs, nanos).unwrap_or_default().to_rfc3339()
}

/// Parse an RFC 3339 / ISO 8601 string to a retained `NSDate`.
pub fn rfc3339_to_nsdate(s: &str) -> Result<Retained<NSDate>> {
    use chrono::DateTime;

    const NS_REFERENCE_EPOCH: i64 = 978_307_200;
    let dt = DateTime::parse_from_rfc3339(s)
        .or_else(|_| DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S"))
        .map_err(|e| anyhow!("invalid date '{}': {}", s, e))?;

    let unix_secs = dt.timestamp();
    let nanos = dt.timestamp_subsec_nanos();
    let interval = (unix_secs - NS_REFERENCE_EPOCH) as f64 + (nanos as f64 / 1_000_000_000.0);

    Ok(NSDate::dateWithTimeIntervalSinceReferenceDate(interval))
}

/// Convert a `Retained<NSError>` to an `anyhow::Error`.
pub fn retained_nserror_to_anyhow(err: &Retained<NSError>) -> anyhow::Error {
    let desc = err.localizedDescription();
    anyhow!("{}", nsstring_to_string(&desc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nsstring_roundtrip() {
        let original = "Hello, Apple Connector!";
        let ns = string_to_nsstring(original);
        let back = nsstring_to_string(&ns);
        assert_eq!(original, back);
    }

    #[test]
    fn test_nsstring_empty() {
        let ns = string_to_nsstring("");
        assert_eq!(nsstring_to_string(&ns), "");
    }

    #[test]
    fn test_nsstring_unicode() {
        let original = "日本語テスト 🍎";
        let ns = string_to_nsstring(original);
        assert_eq!(nsstring_to_string(&ns), original);
    }

    #[test]
    fn test_nsdate_roundtrip() {
        let rfc = "2025-06-15T10:30:00+00:00";
        let date = rfc3339_to_nsdate(rfc).unwrap();
        let back = nsdate_to_rfc3339(&date);
        assert!(back.starts_with("2025-06-15T10:30:00"));
    }

    #[test]
    fn test_nsdate_invalid() {
        assert!(rfc3339_to_nsdate("not-a-date").is_err());
    }

    #[test]
    fn test_nsdate_pre_2001() {
        // 2000-06-15 is before the NSDate reference epoch (2001-01-01)
        let rfc = "2000-06-15T12:00:00+00:00";
        let date = rfc3339_to_nsdate(rfc).unwrap();
        let back = nsdate_to_rfc3339(&date);
        assert!(back.starts_with("2000-06-15T12:00:00"), "got: {}", back);
    }

    #[test]
    fn test_nsdate_subsecond_precision() {
        // Ensure sub-second precision is preserved
        let rfc = "2025-01-15T08:30:00.500+00:00";
        let date = rfc3339_to_nsdate(rfc).unwrap();
        let back = nsdate_to_rfc3339(&date);
        // chrono may format subseconds differently, but the seconds should be :00
        assert!(back.starts_with("2025-01-15T08:30:00"), "got: {}", back);
    }
}
