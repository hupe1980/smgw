//! TAF 1 selection of one cumulative electricity reading per billing close.
//!
//! Commission the monthly or multi-month billing calendar in local time and
//! pass its resolved closing instants in UTC. The caller authenticates the
//! meter, archives original observations, and authorizes and signs delivery.

use crate::taf7::{Channel, Entry, Observation, Quality};
use core::fmt;

/// Local report for one commissioned quantity at the end of a billing period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClosingReading<'a> {
    pub profile_id: &'a str,
    pub channel: Channel<'a>,
    /// Scheduled end of the billing period, in Unix seconds.
    pub end_utc: i64,
    /// Absolute register reading, normalized to whole watt-hours.
    pub cumulative_wh: u64,
}

/// Rejection during commissioning or finalization. A rejected close can be retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    EmptyProfileId,
    EmptyMeterId,
    InvalidSchedule,
    WrongChannel,
    UnexpectedClose,
    ScheduleComplete,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::EmptyProfileId => "supply an evaluation profile identifier",
            Self::EmptyMeterId => "supply a meter identifier",
            Self::InvalidSchedule => {
                "supply a nonempty, strictly increasing list of billing closes"
            }
            Self::WrongChannel => "closing observation belongs to a different meter or direction",
            Self::UnexpectedClose => "expected the next commissioned billing close",
            Self::ScheduleComplete => "all commissioned billing periods have been finalized",
        })
    }
}

impl core::error::Error for Error {}

/// Single-channel evaluator. Each accepted close is consumed once per instance.
pub struct Profile<'a> {
    id: &'a str,
    channel: Channel<'a>,
    remaining: &'a [i64],
}

impl<'a> Profile<'a> {
    /// Commission explicit UTC closes. Calendar and profile validity are resolved upstream.
    pub fn new(id: &'a str, channel: Channel<'a>, closes_utc: &'a [i64]) -> Result<Self, Error> {
        if id.trim().is_empty() {
            return Err(Error::EmptyProfileId);
        }
        if channel.meter_id.trim().is_empty() {
            return Err(Error::EmptyMeterId);
        }
        if closes_utc.is_empty() || closes_utc.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(Error::InvalidSchedule);
        }
        Ok(Self {
            id,
            channel,
            remaining: closes_utc,
        })
    }

    pub fn next_close(&self) -> Option<i64> {
        self.remaining.first().copied()
    }

    /// Finalize an exact scheduled close. A valid reading produces one report.
    /// A missing or invalid observation consumes the period and returns `Ok(None)`.
    /// An error leaves the period open. Finalize only after reception is complete.
    pub fn close(
        &mut self,
        channel: Channel<'_>,
        entry: Entry,
    ) -> Result<Option<ClosingReading<'a>>, Error> {
        let (&end_utc, rest) = self
            .remaining
            .split_first()
            .ok_or(Error::ScheduleComplete)?;
        if channel != self.channel {
            return Err(Error::WrongChannel);
        }
        if entry.scheduled_utc != end_utc {
            return Err(Error::UnexpectedClose);
        }
        self.remaining = rest;
        Ok(match entry.observation {
            Observation::Received {
                quality: Quality::Valid,
                cumulative_wh,
                ..
            } => Some(ClosingReading {
                profile_id: self.id,
                channel: self.channel,
                end_utc,
                cumulative_wh,
            }),
            _ => None,
        })
    }
}
