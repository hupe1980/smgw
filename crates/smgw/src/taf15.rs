//! TAF 15 capture of meter-registered values for one commissioned OBIS channel.
//!
//! Import a bounded reference-time window in any order. Exact retries are
//! idempotent. Records retain their meter timestamp, decimal value and status.

pub use crate::registration::Period;
use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Channel<'a> {
    pub meter_id: &'a str,
    /// Canonical A–F groups from the evaluation profile, with B=0 and F=255.
    pub obis: [u8; 6],
    /// Commissioned unit of the meter values, for example Wh or m³.
    pub unit: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    /// Scheduled registration instant determined from the meter's registration grid.
    pub reference_utc: i64,
    /// Original measurement timestamp supplied by the meter.
    pub measured_utc: i64,
    /// Original value is coefficient × 10^scale in the commissioned unit.
    pub coefficient: i128,
    pub scale: i8,
    pub meter_status: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capture {
    Inserted,
    Duplicate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    EmptyIdentifier,
    InvalidChannel,
    InvalidWindow,
    WrongChannel,
    UnexpectedSlot,
    Conflict,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::EmptyIdentifier => "supply profile and meter identifiers",
            Self::InvalidChannel => {
                "commission an electricity or gas OBIS channel with its unit, B=0 and F=255"
            }
            Self::InvalidWindow => {
                "supply a nonempty representable registration window and an allowed period"
            }
            Self::WrongChannel => "meter, OBIS or unit does not match",
            Self::UnexpectedSlot => "reference timestamp is outside the commissioned slot grid",
            Self::Conflict => {
                "a different record already uses this reference or measurement timestamp"
            }
        })
    }
}
impl core::error::Error for Error {}

/// A single-channel registered value list. The host owns meter authentication,
/// persistence and recipient authorization.
pub struct Profile<'a, const N: usize> {
    id: &'a str,
    channel: Channel<'a>,
    period: Period,
    first: i64,
    slots: [Option<Record>; N],
}

impl<'a, const N: usize> Profile<'a, N> {
    /// The first reference timestamp defines the phase of the meter's grid.
    pub fn new(
        id: &'a str,
        channel: Channel<'a>,
        period: Period,
        first_utc: i64,
    ) -> Result<Self, Error> {
        if id.trim().is_empty() || channel.meter_id.trim().is_empty() {
            return Err(Error::EmptyIdentifier);
        }
        if !matches!(channel.obis, [1 | 7, 0, _, _, _, 255]) || channel.unit.trim().is_empty() {
            return Err(Error::InvalidChannel);
        }
        let last = N.checked_sub(1).ok_or(Error::InvalidWindow)?;
        if (channel.obis[0] == 1 && period == Period::Hour)
            || i128::from(first_utc) + last as i128 * i128::from(period.seconds())
                > i128::from(i64::MAX)
        {
            return Err(Error::InvalidWindow);
        }
        Ok(Self {
            id,
            channel,
            period,
            first: first_utc,
            slots: [None; N],
        })
    }

    pub fn profile_id(&self) -> &'a str {
        self.id
    }
    pub fn channel(&self) -> Channel<'a> {
        self.channel
    }

    /// Insert the first record for this slot. A conflicting replay leaves it intact.
    pub fn capture(&mut self, channel: Channel<'_>, record: Record) -> Result<Capture, Error> {
        if channel != self.channel {
            return Err(Error::WrongChannel);
        }
        let offset = i128::from(record.reference_utc) - i128::from(self.first);
        let seconds = i128::from(self.period.seconds());
        if offset < 0 || offset % seconds != 0 {
            return Err(Error::UnexpectedSlot);
        }
        let index = usize::try_from(offset / seconds).map_err(|_| Error::UnexpectedSlot)?;
        match self.slots.get(index).ok_or(Error::UnexpectedSlot)? {
            Some(original) if *original == record => return Ok(Capture::Duplicate),
            Some(_) => return Err(Error::Conflict),
            None => {}
        }
        if self
            .records()
            .any(|r| r.measured_utc == record.measured_utc)
        {
            return Err(Error::Conflict);
        }
        self.slots[index] = Some(record);
        Ok(Capture::Inserted)
    }

    /// Iterate in reference-time order, including late backfills.
    pub fn records(&self) -> impl Iterator<Item = &Record> {
        self.slots.iter().flatten()
    }

    /// Reference instants to request from the meter on a subsequent read.
    pub fn missing(&self) -> impl Iterator<Item = i64> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, r)| r.is_none())
            .map(|(i, _)| {
                (i128::from(self.first) + i as i128 * i128::from(self.period.seconds())) as i64
            })
    }
}
