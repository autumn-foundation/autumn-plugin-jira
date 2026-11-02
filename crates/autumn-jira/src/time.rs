//! Jira timestamps.
//!
//! Jira REST replies use `2021-01-17T12:34:00.000+0000`: the offset has no
//! colon, so a plain RFC 3339 parser rejects it. Webhooks use epoch
//! milliseconds.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{JiraError, Result};

/// A point in time from Jira, in UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JiraTimestamp(DateTime<Utc>);

impl JiraTimestamp {
    /// Parses `%Y-%m-%dT%H:%M:%S%.f%z`, RFC 3339 or epoch milliseconds.
    ///
    /// # Errors
    ///
    /// `JiraError::InvalidInput` when no format matches.
    pub fn parse(text: &str) -> Result<Self> {
        let bad = || {
            JiraError::invalid(
                "timestamp",
                "must be %Y-%m-%dT%H:%M:%S%.f%z, RFC 3339 or epoch ms",
            )
        };
        if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
            return text
                .parse::<i64>()
                .ok()
                .and_then(Self::from_epoch_ms)
                .ok_or_else(bad);
        }
        DateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f%z")
            .or_else(|_| DateTime::parse_from_rfc3339(text))
            .map(|t| Self(t.with_timezone(&Utc)))
            .map_err(|_| bad())
    }

    /// Makes a timestamp from epoch milliseconds. `None` when out of range.
    #[must_use]
    pub const fn from_epoch_ms(ms: i64) -> Option<Self> {
        match DateTime::from_timestamp_millis(ms) {
            Some(time) => Some(Self(time)),
            None => None,
        }
    }

    /// Returns the epoch milliseconds.
    #[must_use]
    pub const fn epoch_ms(&self) -> i64 {
        self.0.timestamp_millis()
    }

    /// Returns the time.
    #[must_use]
    pub const fn as_datetime(&self) -> &DateTime<Utc> {
        &self.0
    }
}

impl From<DateTime<Utc>> for JiraTimestamp {
    fn from(time: DateTime<Utc>) -> Self {
        Self(time)
    }
}

impl fmt::Display for JiraTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            self.0.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        )
    }
}

impl Serialize for JiraTimestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for JiraTimestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        // Jira sends text in REST replies and epoch ms in webhooks.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Ms(i64),
            Text(String),
        }
        match Wire::deserialize(deserializer)? {
            Wire::Ms(ms) => {
                Self::from_epoch_ms(ms).ok_or_else(|| serde::de::Error::custom("bad epoch ms"))
            }
            Wire::Text(text) => Self::parse(&text).map_err(serde::de::Error::custom),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;

    #[test]
    fn parse_table() {
        // 2021-01-17T12:34:00Z.
        let base = 1_610_886_840_000_i64;
        let rows: [(&str, i64); 9] = [
            ("2021-01-17T12:34:00.000+0000", base),
            ("2021-01-17T12:34:00.123+0000", base + 123),
            ("2021-01-17T18:04:00.000+0530", base),
            ("2021-01-17T18:04:00.000+05:30", base),
            ("2021-01-17T12:34:00Z", base),
            ("2021-01-17T12:34:00.5Z", base + 500),
            ("2021-01-17T12:34:00+0000", base),
            ("2021-01-17T07:34:00-0500", base),
            ("1610886840000", base),
        ];
        for (text, ms) in rows {
            let t = JiraTimestamp::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(t.epoch_ms(), ms, "{text}");
        }
        for bad in [
            "",
            "yesterday",
            "2021-01-17",
            "2021-01-17T12:34:00",
            "12a",
            "-",
        ] {
            assert!(JiraTimestamp::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn epoch_round_trip_and_serde() {
        let t = JiraTimestamp::from_epoch_ms(1_606_480_436_302).unwrap();
        assert_eq!(t.epoch_ms(), 1_606_480_436_302);
        assert_eq!(t.to_string(), "2020-11-27T12:33:56.302Z");
        assert_eq!(t.as_datetime().timestamp_millis(), 1_606_480_436_302);
        assert!(JiraTimestamp::from_epoch_ms(i64::MAX).is_none());
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, "\"2020-11-27T12:33:56.302Z\"");
        assert_eq!(serde_json::from_str::<JiraTimestamp>(&json).unwrap(), t);
        assert_eq!(
            serde_json::from_str::<JiraTimestamp>("1606480436302").unwrap(),
            t
        );
        assert_eq!(
            serde_json::from_str::<JiraTimestamp>("\"2020-11-27T12:33:56.302+0000\"").unwrap(),
            t
        );
        assert!(serde_json::from_str::<JiraTimestamp>("true").is_err());
        assert!(serde_json::from_str::<JiraTimestamp>("1.5").is_err());
        assert_eq!(JiraTimestamp::from(*t.as_datetime()), t);
    }
}
