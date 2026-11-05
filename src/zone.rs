//! IANA time zones for usage days and window-starter hours.
//!
//! The data is the tz database bundled into the binary (jiff's copy), so the
//! host's zoneinfo is never read and the binary stays self-contained. It is kept
//! as compact TZif data and read on demand: the tables `chrono-tz` compiled in
//! held a pointer per transition, which the loader relocated at every start
//! (about 1.2 MB of the binary and of each process's private memory).

use chrono::{DateTime, Datelike, LocalResult, NaiveDate, NaiveDateTime, Utc};
use jiff::Timestamp;
use jiff::tz::{AmbiguousOffset, TimeZone, TimeZoneDatabase};
use std::sync::OnceLock;

/// The bundled database itself, not `jiff::tz::db()`: that one would read the host's
/// zoneinfo first should a dependency ever turn on jiff's system database features.
fn database() -> &'static TimeZoneDatabase {
    static DATABASE: OnceLock<TimeZoneDatabase> = OnceLock::new();
    DATABASE.get_or_init(TimeZoneDatabase::bundled)
}

/// A name the database doesn't have. Reads as chrono-tz's parse error did.
#[derive(Debug)]
pub struct UnknownZone;

impl std::fmt::Display for UnknownZone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("failed to parse timezone")
    }
}

impl std::error::Error for UnknownZone {}

/// An IANA time zone, named exactly as the database spells it
/// (`Europe/Amsterdam`, `UTC`).
#[derive(Clone, Debug)]
pub struct Zone(TimeZone);

impl Zone {
    #[cfg(test)]
    pub fn utc() -> Self {
        Self(TimeZone::UTC)
    }

    /// The zone of that name. Case counts, as it always did here: `europe/amsterdam` is not a zone.
    /// Neither is the database's `Factory` placeholder, which chrono-tz never offered.
    pub fn parse(name: &str) -> Result<Self, UnknownZone> {
        let tz = database().get(name).map_err(|_| UnknownZone)?;
        (name != "Factory" && tz.iana_name() == Some(name)).then_some(Self(tz)).ok_or(UnknownZone)
    }

    /// Milliseconds the zone is ahead of UTC at the instant `ms`.
    fn offset_ms(&self, ms: i64) -> i64 {
        i64::from(self.0.to_offset(timestamp(ms)).seconds()) * 1000
    }

    /// The wall-clock time here at an instant.
    pub fn local(&self, at: DateTime<Utc>) -> Option<NaiveDateTime> {
        at.naive_utc().checked_add_signed(chrono::Duration::milliseconds(self.offset_ms(at.timestamp_millis())))
    }

    /// The instant (ms since the epoch) local midnight starts `date`: two when the clocks
    /// fall back across it, none when they spring forward over it.
    pub fn midnight(&self, date: NaiveDate) -> LocalResult<i64> {
        let wall = date.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis();
        let civil = i16::try_from(date.year_ce().1)
            .ok()
            .filter(|_| date.year_ce().0)
            .and_then(|year| jiff::civil::Date::new(year, date.month0() as i8 + 1, date.day0() as i8 + 1).ok());
        let Some(civil) = civil else {
            // Beyond the database's years, the offset at its edge holds.
            return LocalResult::Single(wall - self.offset_ms(wall));
        };
        let at = |offset: jiff::tz::Offset| wall - i64::from(offset.seconds()) * 1000;
        match self.0.to_ambiguous_timestamp(civil.at(0, 0, 0, 0)).offset() {
            AmbiguousOffset::Unambiguous { offset } => LocalResult::Single(at(offset)),
            AmbiguousOffset::Gap { .. } => LocalResult::None,
            AmbiguousOffset::Fold { before, after } => {
                let (a, b) = (at(before), at(after));
                LocalResult::Ambiguous(a.min(b), a.max(b))
            }
        }
    }
}

/// The name, as the database spells it.
impl std::fmt::Display for Zone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.iana_name().unwrap_or("UTC"))
    }
}

/// The database covers years -9999 to 9999; an instant beyond reads the offset at the edge.
fn timestamp(ms: i64) -> Timestamp {
    let ms = ms.clamp(Timestamp::MIN.as_millisecond(), Timestamp::MAX.as_millisecond());
    Timestamp::from_millisecond(ms).unwrap_or(Timestamp::UNIX_EPOCH)
}

/// One stretch between two transitions: the offset holds for instants in `from..until` (ms).
#[derive(Clone, Copy)]
struct Period {
    from: i64,
    until: i64,
    offset: i64,
}

impl Period {
    fn contains(&self, ms: i64) -> bool {
        (self.from..self.until).contains(&ms)
    }
}

/// Bounds the stretches one reader keeps; a decade has about twenty.
const MAX_PERIODS: usize = 4096;

/// Local calendar dates for many instants, in any order. A zone's offset holds from one
/// transition to the next, months apart, so each stretch is looked up once and every
/// other instant in it costs a comparison or a short binary search.
///
/// A stretch is known from the earliest instant seen in it to the next transition. Only
/// forward lookups are used: past a zone's last listed transition, where its standing
/// rule takes over, jiff's backward iterator can name a rule transition from before the
/// last listed one.
pub struct LocalDates {
    zone: Zone,
    /// The stretches seen so far, sorted and disjoint; `last` is the one used last.
    periods: Vec<Period>,
    last: usize,
}

impl LocalDates {
    pub fn new(zone: Zone) -> Self {
        Self { zone, periods: Vec::new(), last: 0 }
    }

    /// The local date at `ms`, or none where chrono has no date for the instant.
    pub fn date(&mut self, ms: i64) -> Option<NaiveDate> {
        DateTime::from_timestamp_millis(ms)?;
        let offset = self.offset(ms);
        Some(DateTime::from_timestamp_millis(ms.checked_add(offset)?)?.date_naive())
    }

    /// Seconds the zone is ahead of UTC at `ms`, or none where chrono has no date for the instant.
    #[cfg(test)]
    pub fn offset_seconds(&mut self, ms: i64) -> Option<i32> {
        DateTime::from_timestamp_millis(ms)?;
        i32::try_from(self.offset(ms) / 1000).ok()
    }

    fn offset(&mut self, ms: i64) -> i64 {
        if let Some(p) = self.periods.get(self.last)
            && p.contains(ms)
        {
            return p.offset;
        }
        // The first known stretch that ends after `ms`.
        let i = self.periods.partition_point(|p| p.until <= ms);
        if let Some(p) = self.periods.get(i)
            && p.from <= ms
        {
            self.last = i;
            return p.offset;
        }
        let Some((until, offset)) = self.stretch(ms) else { return self.zone.offset_ms(ms) };
        if let Some(p) = self.periods.get_mut(i)
            && p.until <= until
        {
            // Earlier in a stretch already known: it now reaches back to `ms`. Anything
            // else would mean the lookups disagree, and the answer is just not kept.
            if p.until == until && p.offset == offset {
                p.from = ms;
                self.last = i;
            }
            return offset;
        }
        if self.periods.len() >= MAX_PERIODS {
            self.periods.clear();
        }
        let i = self.periods.partition_point(|p| p.until <= ms);
        self.periods.insert(i, Period { from: ms, until, offset });
        self.last = i;
        offset
    }

    /// The offset at `ms` and the next transition after it, unless `ms` is beyond the
    /// database's years.
    fn stretch(&self, ms: i64) -> Option<(i64, i64)> {
        let at = timestamp(ms);
        if at.as_millisecond() != ms {
            return None;
        }
        let offset = i64::from(self.zone.0.to_offset(at).seconds()) * 1000;
        // The first transition after `ms`, whether or not the iterator starts with one at
        // `ms` itself. Two at most: past the last listed transition of a zone without a
        // standing rule, jiff repeats that past transition, and the offset holds for good.
        let until = self
            .zone
            .0
            .following(at)
            .take(2)
            .map(|t| t.timestamp().as_millisecond())
            .find(|&until| until > ms)
            .unwrap_or(i64::MAX);
        Some((until, offset))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(s: &str) -> i64 {
        DateTime::parse_from_rfc3339(s).unwrap().timestamp_millis()
    }

    #[test]
    fn names_must_match_the_database_exactly() {
        assert_eq!(Zone::utc().to_string(), "UTC");
        assert_eq!(Zone::parse("UTC").unwrap().to_string(), "UTC");
        assert_eq!(Zone::parse("US/Eastern").unwrap().to_string(), "US/Eastern");
        assert!(Zone::parse("Europe/Amsterdam").is_ok());
        assert!(Zone::parse("UTC").is_ok());
        assert!(Zone::parse("US/Eastern").is_ok());
        assert!(Zone::parse("europe/amsterdam").is_err());
        assert!(Zone::parse("Mars/Olympus_Mons").is_err());
        assert!(Zone::parse("Factory").is_err());
        assert_eq!(Zone::parse("utc").unwrap_err().to_string(), "failed to parse timezone");
        assert!(Zone::parse("").is_err());
    }

    #[test]
    fn local_dates_follow_daylight_saving() {
        let mut dates = LocalDates::new(Zone::parse("Europe/Amsterdam").unwrap());
        let date = |d: &mut LocalDates, s: &str| d.date(ms(s)).unwrap().to_string();
        // CET is UTC+1, CEST UTC+2; the clocks went forward on 30 March 2025 at 01:00 UTC.
        assert_eq!(date(&mut dates, "2025-03-29T22:59:59Z"), "2025-03-29");
        assert_eq!(date(&mut dates, "2025-03-29T23:00:00Z"), "2025-03-30");
        assert_eq!(date(&mut dates, "2025-07-01T21:59:59Z"), "2025-07-01");
        assert_eq!(date(&mut dates, "2025-07-01T22:00:00Z"), "2025-07-02");
        // Back across the transition, out of order.
        assert_eq!(date(&mut dates, "2025-03-30T00:59:59Z"), "2025-03-30");
        assert_eq!(date(&mut dates, "2025-01-15T23:00:00Z"), "2025-01-16");
        assert_eq!(date(&mut dates, "2025-03-30T00:59:59Z"), "2025-03-30");
        assert_eq!(dates.offset_seconds(ms("2025-03-30T00:59:59Z")), Some(3600));
        assert_eq!(dates.offset_seconds(ms("2025-03-30T01:00:00Z")), Some(7200));
        // Past Ciudad Juárez's last listed transition (to MST on 30 November 2022) its
        // standing US rule applies; the CST weeks before it must not be read as MST.
        let mut juarez = LocalDates::new(Zone::parse("America/Ciudad_Juarez").unwrap());
        assert_eq!(juarez.offset_seconds(ms("2022-12-15T10:00:00Z")), Some(-7 * 3600));
        assert_eq!(juarez.offset_seconds(ms("2022-11-12T06:00:00Z")), Some(-6 * 3600));
        assert_eq!(juarez.offset_seconds(ms("2022-11-30T05:59:59Z")), Some(-6 * 3600));
        assert_eq!(juarez.offset_seconds(ms("2022-11-30T06:00:00Z")), Some(-7 * 3600));
        assert_eq!(LocalDates::new(Zone::utc()).date(ms("2025-01-15T23:59:59Z")).unwrap().to_string(), "2025-01-15");
    }

    /// The cache must answer as an uncached lookup does, in any order: around transitions,
    /// past a zone's last listed one (Ciudad Juárez, Metlakatla), and far ahead.
    #[test]
    fn local_dates_agree_with_direct_lookups() {
        let names = ["Europe/Amsterdam", "America/New_York", "America/Ciudad_Juarez", "America/Metlakatla"];
        let names = names.into_iter().chain(["Australia/Lord_Howe", "America/Havana", "Asia/Kolkata", "UTC"]);
        let (low, high) = (ms("1990-01-01T00:00:00Z"), ms("2140-01-01T00:00:00Z"));
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        for name in names {
            let zone = Zone::parse(name).unwrap();
            let mut dates = LocalDates::new(zone.clone());
            for _ in 0..20_000 {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let at = low + (state % (high - low) as u64) as i64;
                let direct = zone.local(DateTime::from_timestamp_millis(at).unwrap()).unwrap();
                assert_eq!(dates.date(at), Some(direct.date()), "{name} at {at}");
                assert_eq!(
                    dates.offset_seconds(at).map(i64::from),
                    Some((direct.and_utc().timestamp_millis() - at) / 1000),
                    "{name} at {at}"
                );
            }
        }
    }

    #[test]
    fn midnight_handles_gaps_and_folds() {
        let day = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        let amsterdam = Zone::parse("Europe/Amsterdam").unwrap();
        assert_eq!(amsterdam.midnight(day("2025-07-01")), LocalResult::Single(ms("2025-06-30T22:00:00Z")));
        // Havana springs forward at midnight: 2025-03-09 00:00 never happened there.
        assert_eq!(Zone::parse("America/Havana").unwrap().midnight(day("2025-03-09")), LocalResult::None);
        // ... and falls back at 01:00 to midnight, so 2025-11-02 00:00 happened twice.
        assert_eq!(
            Zone::parse("America/Havana").unwrap().midnight(day("2025-11-02")),
            LocalResult::Ambiguous(ms("2025-11-02T04:00:00Z"), ms("2025-11-02T05:00:00Z"))
        );
    }

    #[test]
    fn local_wall_time() {
        let tokyo = Zone::parse("Asia/Tokyo").unwrap();
        let at = DateTime::parse_from_rfc3339("2025-12-31T15:30:00Z").unwrap().with_timezone(&Utc);
        assert_eq!(tokyo.local(at).unwrap().to_string(), "2026-01-01 00:30:00");
    }
}
