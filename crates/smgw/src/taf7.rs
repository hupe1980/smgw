//! Bounded, in-memory TAF 7 series for one commissioned active-energy channel.
//!
//! Each slot retains the original cumulative Wh, source time and caller-classified
//! quality, or an explicit gap. Archive the block before starting the next one.

use crate::taf2;
use core::fmt;

/// Direction of cumulative active energy, normalized to whole watt-hours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Import,
    Export,
}

/// Identity supplied by a caller that has authenticated the meter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Channel<'a> {
    pub meter_id: &'a str,
    pub direction: Direction,
}

/// Registration cadence for this electricity simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    QuarterHour,
    Hour,
}

impl Period {
    pub const fn seconds(self) -> i64 {
        match self {
            Self::QuarterHour => 900,
            Self::Hour => 3600,
        }
    }
}

/// Status assigned by the caller using the source and reception evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    Valid,
    TimeInvalid,
    MeterFault,
    Fatal,
}

/// Original observation for a scheduled instant. Source and scheduled times are Unix seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    Missing,
    Received {
        source_utc: i64,
        cumulative_wh: u64,
        quality: Quality,
    },
}

impl From<Observation> for taf2::Reading {
    /// Project a slot's quality and value for register arithmetic. The caller
    /// supplies the commissioned tariff and enforces the TAF 2 quarter-hour grid.
    fn from(value: Observation) -> Self {
        match value {
            Observation::Missing => Self::Missing,
            Observation::Received {
                cumulative_wh,
                quality,
                ..
            } => match quality {
                Quality::Valid => Self::Valid(cumulative_wh),
                Quality::TimeInvalid => Self::TimeInvalid,
                Quality::MeterFault => Self::MeterFault(cumulative_wh),
                Quality::Fatal => Self::Fatal,
            },
        }
    }
}

/// One original observation, indexed by its scheduled registration instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub scheduled_utc: i64,
    pub observation: Observation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    EmptyMeterId,
    EmptyBlock,
    UnalignedStart,
    TimestampOverflow,
    WrongChannel,
    UnexpectedSlot,
    Full,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::EmptyMeterId => "supply a meter identifier",
            Self::EmptyBlock => "the block must hold at least one slot",
            Self::UnalignedStart => "first slot must align to the UTC registration grid",
            Self::TimestampOverflow => "last slot is outside the representable UTC range",
            Self::WrongChannel => "observation belongs to a different meter or energy direction",
            Self::UnexpectedSlot => {
                "submit the next scheduled slot, including missing observations"
            }
            Self::Full => "series is full; archive it and start a new block",
        })
    }
}
impl core::error::Error for Error {}

/// Append-only block of at most N entries. Every slot uses fixed storage, including gaps.
pub struct Series<'a, const N: usize> {
    channel: Channel<'a>,
    period: Period,
    next: i64,
    entries: [Entry; N],
    len: usize,
}

impl<'a, const N: usize> Series<'a, N> {
    /// Start an empty block at its first reportable instant.
    pub fn new(channel: Channel<'a>, period: Period, first_utc: i64) -> Result<Self, Error> {
        if channel.meter_id.trim().is_empty() {
            return Err(Error::EmptyMeterId);
        }
        let last = N.checked_sub(1).ok_or(Error::EmptyBlock)?;
        if first_utc % period.seconds() != 0 {
            return Err(Error::UnalignedStart);
        }
        let last_utc = i128::from(first_utc) + last as i128 * i128::from(period.seconds());
        i64::try_from(last_utc).map_err(|_| Error::TimestampOverflow)?;
        Ok(Self {
            channel,
            period,
            next: first_utc,
            entries: [Entry {
                scheduled_utc: 0,
                observation: Observation::Missing,
            }; N],
            len: 0,
        })
    }

    pub fn channel(&self) -> Channel<'a> {
        self.channel
    }
    pub fn period(&self) -> Period {
        self.period
    }
    pub fn next_slot(&self) -> Option<i64> {
        (self.len < N).then_some(self.next)
    }
    /// Accepted entries in chronological order. A refused append leaves this slice unchanged.
    pub fn entries(&self) -> &[Entry] {
        &self.entries[..self.len]
    }

    /// Append exactly one slot. Submit a missing observation explicitly when no sample arrives.
    pub fn record(
        &mut self,
        channel: Channel<'_>,
        scheduled_utc: i64,
        observation: Observation,
    ) -> Result<(), Error> {
        if channel != self.channel {
            return Err(Error::WrongChannel);
        }
        let next = self.next_slot().ok_or(Error::Full)?;
        if scheduled_utc != next {
            return Err(Error::UnexpectedSlot);
        }
        self.entries[self.len] = Entry {
            scheduled_utc,
            observation,
        };
        self.len += 1;
        if self.len < N {
            self.next += self.period.seconds();
        }
        Ok(())
    }
}
