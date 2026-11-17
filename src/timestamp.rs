//! Session timestamps in logs, exports, and the local report.

use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcDateTime, UtcOffset};

/// Read a record's RFC 3339 timestamp and apply its offset.
pub(crate) fn parse(text: &str) -> Option<UtcDateTime> {
    // `time` accepts any separator; keep chrono's T, t, or space.
    if !matches!(text.as_bytes().get(10), Some(b'T' | b't' | b' ')) {
        return None;
    }
    OffsetDateTime::parse(text, &Rfc3339).ok()?.checked_to_utc()
}

/// The UTC spelling used by CSV, JSON, and the label cache. Fractions use 3, 6, or 9 digits.
pub(crate) fn format(at: UtcDateTime) -> String {
    let (hour, minute, second, nanos) = at.as_hms_nano();
    let fraction = match nanos {
        0 => String::new(),
        n if n.is_multiple_of(1_000_000) => format!(".{:03}", n / 1_000_000),
        n if n.is_multiple_of(1_000) => format!(".{:06}", n / 1_000),
        n => format!(".{n:09}"),
    };
    format!(
        "{}T{hour:02}:{minute:02}:{second:02}{fraction}+00:00",
        at.date()
    )
}

/// Use the local offset at the recorded instant, including daylight saving time.
pub(crate) fn local(at: UtcDateTime) -> OffsetDateTime {
    let utc = at.to_offset(UtcOffset::UTC);
    UtcOffset::local_offset_at(utc)
        .ok()
        .and_then(|offset| at.checked_to_offset(offset))
        .unwrap_or(utc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_offsets_and_preserves_export_precision() {
        for (record, exported) in [
            ("2026-01-02T00:15:00+01:00", "2026-01-01T23:15:00+00:00"),
            ("2026-01-01 23:15:00.1Z", "2026-01-01T23:15:00.100+00:00"),
            (
                "2026-01-01t22:45:00.00001-00:30",
                "2026-01-01T23:15:00.000010+00:00",
            ),
            (
                "2026-01-01T23:15:00.00000001z",
                "2026-01-01T23:15:00.000000010+00:00",
            ),
        ] {
            assert_eq!(format(parse(record).unwrap()), exported);
        }
        assert!(parse("2026-01-01T23:15:00").is_none());
        assert!(parse("2026-01-01x23:15:00Z").is_none());
    }
}
