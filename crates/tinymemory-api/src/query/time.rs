//! [`TimeHint`]: the calendar days a question is about.
//!
//! A hint ranks, it never filters. "What did Arjun say yesterday?" should put
//! yesterday's memories first, but a memory recorded on Monday about Tuesday,
//! or one whose day the host misread, must still be reachable. Engines that
//! can boost by date server-side do; any engine can fall back to
//! [`TimeHint::rank`], a stable partition of hits it already holds.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use super::Hit;
use crate::error::{Error, Result};

/// Local calendar days, inclusive, in an IANA time zone.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TimeHint {
    /// First day.
    pub from: NaiveDate,
    /// Last day; equal to `from` for a single day.
    pub to: NaiveDate,
    /// The zone the days are read in (`Asia/Kolkata`); UTC when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone: Option<String>,
}

impl TimeHint {
    /// The days `from..=to` in `zone`.
    ///
    /// # Errors
    ///
    /// As [`TimeHint::validate`].
    pub fn new(from: NaiveDate, to: NaiveDate, zone: Option<String>) -> Result<Self> {
        let hint = Self { from, to, zone };
        hint.validate()?;
        Ok(hint)
    }

    /// Checks the range is ordered and the zone is a real IANA name, so an
    /// engine never sends its server a hint it would refuse.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] for `from > to` or an unknown zone.
    pub fn validate(&self) -> Result<()> {
        if self.from > self.to {
            return Err(Error::InvalidRequest(format!(
                "time hint runs backwards: {} > {}",
                self.from, self.to
            )));
        }
        self.tz()?;
        Ok(())
    }

    /// Whether `at` falls on one of the hinted days, read in the hint's zone.
    #[must_use]
    pub fn covers(&self, at: DateTime<Utc>) -> bool {
        let Ok(tz) = self.tz() else {
            return false;
        };
        let day = at.with_timezone(&tz).date_naive();
        self.from <= day && day <= self.to
    }

    /// Moves the hits observed on a hinted day ahead of the rest, keeping the
    /// order within each group: the engine's ranking is kept, only lifted by
    /// date. A hit without `observed_at` stays with the rest.
    pub fn rank(&self, hits: &mut Vec<Hit>) {
        let (mut on, off): (Vec<Hit>, Vec<Hit>) = std::mem::take(hits)
            .into_iter()
            .partition(|hit| hit.meta.observed_at.is_some_and(|at| self.covers(at)));
        on.extend(off);
        *hits = on;
    }

    fn tz(&self) -> Result<chrono_tz::Tz> {
        match self.zone.as_deref() {
            None => Ok(chrono_tz::UTC),
            Some(zone) => zone.parse().map_err(|_| {
                Error::InvalidRequest(format!("time hint zone {zone:?} is not an IANA zone"))
            }),
        }
    }
}

#[cfg(test)]
#[path = "time_tests.rs"]
mod tests;
