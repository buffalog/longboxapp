//! Serde for every timestamp the API sends: `2026-10-28T15:59:00Z`.
//!
//! `time`'s own serde writes `PrimitiveDateTime` as `2026-10-28 15:59:00.0`
//! (no `T`, no zone: Safari rejects it, Chrome reads it as local time) and
//! `time::serde::iso8601` writes `+002026-10-28T…` (six-digit year). This
//! writes the subset of RFC 3339 that ECMA-262's Date Time String Format
//! guarantees every browser parses: four-digit year, whole seconds, `Z`.
//!
//! `PrimitiveDateTime` values are UTC by convention (SQLite
//! `CURRENT_TIMESTAMP` and `now_utc()` everywhere), so they get `Z`.
//!
//! Use on any `OffsetDateTime`/`PrimitiveDateTime` field, or an `Option`
//! of one: `#[serde(with = "longbox_core::rfc3339")]`. A workspace test
//! fails if a serialized datetime field doesn't.

use serde::{Deserialize, Deserializer, Serializer};
use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::{OffsetDateTime, PrimitiveDateTime, UtcOffset};

/// A timestamp type (or `Option` of one) this module can write and read.
pub trait Timestamp: Sized {
    fn to_utc(&self) -> Option<OffsetDateTime>;
    fn from_utc(t: Option<OffsetDateTime>) -> Option<Self>;
}

impl Timestamp for OffsetDateTime {
    fn to_utc(&self) -> Option<OffsetDateTime> {
        Some(self.to_offset(UtcOffset::UTC))
    }
    fn from_utc(t: Option<OffsetDateTime>) -> Option<Self> {
        t
    }
}

impl Timestamp for PrimitiveDateTime {
    fn to_utc(&self) -> Option<OffsetDateTime> {
        Some(self.assume_utc())
    }
    fn from_utc(t: Option<OffsetDateTime>) -> Option<Self> {
        t.map(|t| PrimitiveDateTime::new(t.date(), t.time()))
    }
}

impl<T: Timestamp> Timestamp for Option<T> {
    fn to_utc(&self) -> Option<OffsetDateTime> {
        self.as_ref().and_then(T::to_utc)
    }
    fn from_utc(t: Option<OffsetDateTime>) -> Option<Self> {
        Some(t.and_then(|t| T::from_utc(Some(t))))
    }
}

/// `2026-10-28T15:59:00Z`.
pub fn format(t: OffsetDateTime) -> String {
    t.to_offset(UtcOffset::UTC)
        .format(format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second]Z"
        ))
        .expect("a UTC datetime with a four-digit year always formats")
}

pub fn serialize<T: Timestamp, S: Serializer>(v: &T, s: S) -> Result<S::Ok, S::Error> {
    match v.to_utc() {
        Some(t) => s.serialize_str(&format(t)),
        None => s.serialize_none(),
    }
}

pub fn deserialize<'de, T: Timestamp, D: Deserializer<'de>>(d: D) -> Result<T, D::Error> {
    let parsed = Option::<String>::deserialize(d)?
        .map(|s| OffsetDateTime::parse(&s, &Rfc3339).map_err(serde::de::Error::custom))
        .transpose()?;
    T::from_utc(parsed).ok_or_else(|| serde::de::Error::custom("expected a timestamp, got null"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct Row {
        #[serde(with = "crate::rfc3339")]
        p: PrimitiveDateTime,
        #[serde(with = "crate::rfc3339")]
        o: OffsetDateTime,
        #[serde(with = "crate::rfc3339")]
        none: Option<PrimitiveDateTime>,
        #[serde(with = "crate::rfc3339")]
        some: Option<OffsetDateTime>,
    }

    #[test]
    fn writes_browser_safe_rfc3339_and_reads_it_back() {
        let row = Row {
            p: datetime!(2026-10-28 15:59:00.123456789),
            // A non-UTC offset is converted, not relabelled.
            o: datetime!(2026-10-28 08:59:00 -07:00),
            none: None,
            some: Some(datetime!(2026-10-28 15:59:00 UTC)),
        };
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "p": "2026-10-28T15:59:00Z",
                "o": "2026-10-28T15:59:00Z",
                "none": null,
                "some": "2026-10-28T15:59:00Z",
            })
        );
        let back: Row = serde_json::from_value(json).unwrap();
        assert_eq!(back.p, datetime!(2026-10-28 15:59:00));
        assert_eq!(back.o, datetime!(2026-10-28 15:59:00 UTC));
        assert_eq!(back.none, None);
    }
}
