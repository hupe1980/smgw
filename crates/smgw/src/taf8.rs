//! TAF 8 load curves and maximum demand for one electricity billing window.
//!
//! Submit qualified cumulative readings at the opening and each following
//! registration boundary. Archive the returned intervals in the host.

pub use crate::{registration::Period, taf7::Direction, taf14::Quadrant};
use core::fmt;

/// Electrical register families in TR-03109-1 v2.0, table 3.43.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Register {
    Active(Direction),
    Reactive(Quadrant),
}

impl Register {
    const fn group_c(self) -> u8 {
        match self {
            Self::Active(Direction::Import) => 1,
            Self::Active(Direction::Export) => 2,
            Self::Reactive(Quadrant::R1) => 5,
            Self::Reactive(Quadrant::R2) => 6,
            Self::Reactive(Quadrant::R3) => 7,
            Self::Reactive(Quadrant::R4) => 8,
        }
    }

    pub const fn energy_obis(self) -> [u8; 6] {
        [1, 0, self.group_c(), 8, 0, 255]
    }
    pub const fn demand_obis(self) -> [u8; 6] {
        [1, 0, self.group_c(), 29, 0, 255]
    }
    pub const fn maximum_obis(self) -> [u8; 6] {
        [1, 0, self.group_c(), 6, 0, 255]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Channel<'a> {
    pub meter_id: &'a str,
    pub register: Register,
    /// Decimal exponent relative to Wh or varh. Derived W or var use the same exponent.
    pub scale: i8,
}

/// Caller-classified combined status in increasing severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Valid,
    Warning,
    Missing,
    TemporaryError,
    PermanentError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    pub count: u64,
    pub status: Status,
    pub validated_for_billing: bool,
}

impl Reading {
    pub const fn valid(count: u64) -> Self {
        Self {
            count,
            status: Status::Valid,
            validated_for_billing: true,
        }
    }

    pub const MISSING: Self = Self {
        count: 0,
        status: Status::Missing,
        validated_for_billing: false,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Demand {
    /// Mean W or var in the channel's scale. Faulty intervals carry zero.
    pub count: u128,
    pub status: Status,
    pub validated_for_billing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peak {
    pub end_utc: i64,
    pub demand: Demand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interval<'a> {
    pub profile_id: &'a str,
    pub channel: Channel<'a>,
    pub end_utc: i64,
    /// Registered closing count with its effective status for this billing window.
    pub closing: Reading,
    pub mean: Demand,
    /// None while the maximum register is zero and has no associated time.
    pub peak: Option<Peak>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    EmptyIdentifier,
    InvalidWindow,
    WrongChannel,
    UnexpectedSlot,
    CounterDecreased,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::EmptyIdentifier => "supply profile and meter identifiers",
            Self::InvalidWindow => {
                "supply ordered boundaries aligned to a 5- or 15-minute electricity interval"
            }
            Self::WrongChannel => "meter, register or scale does not match",
            Self::UnexpectedSlot => "time lies outside the expected registration interval",
            Self::CounterDecreased => {
                "qualify the register reset or correct the decreasing reading"
            }
        })
    }
}
impl core::error::Error for Error {}

/// One daily or monthly billing window with host-resolved UTC boundaries.
pub struct Profile<'a> {
    id: &'a str,
    channel: Channel<'a>,
    period: Period,
    close: i64,
    next: Option<i64>,
    previous: Option<Reading>,
    peak: Option<Peak>,
    permanent_fault: bool,
}

impl<'a> Profile<'a> {
    pub fn new(
        id: &'a str,
        channel: Channel<'a>,
        period: Period,
        open_utc: i64,
        close_utc: i64,
    ) -> Result<Self, Error> {
        if id.trim().is_empty() || channel.meter_id.trim().is_empty() {
            return Err(Error::EmptyIdentifier);
        }
        let seconds = period.seconds();
        if period == Period::Hour
            || open_utc >= close_utc
            || open_utc % seconds != 0
            || close_utc % seconds != 0
        {
            return Err(Error::InvalidWindow);
        }
        Ok(Self {
            id,
            channel,
            period,
            close: close_utc,
            next: Some(open_utc),
            previous: None,
            peak: None,
            permanent_fault: false,
        })
    }

    /// The opening establishes a baseline. Each subsequent accepted slot emits
    /// an interval through the commissioned close, inclusive.
    pub fn record(
        &mut self,
        channel: Channel<'_>,
        utc: i64,
        reading: Reading,
    ) -> Result<Option<Interval<'a>>, Error> {
        if channel != self.channel {
            return Err(Error::WrongChannel);
        }
        if self.next != Some(utc) {
            return Err(Error::UnexpectedSlot);
        }
        let mut reading = reading;
        if self.permanent_fault && reading.status != Status::Missing {
            reading.status = Status::PermanentError;
        }
        reading.validated_for_billing &= reading.status <= Status::Warning;
        let mean = self
            .previous
            .map(|previous| self.derive(previous, reading, self.period.seconds()))
            .transpose()?;
        self.permanent_fault |= reading.status == Status::PermanentError;
        self.previous = Some(reading);
        self.next = if utc == self.close {
            None
        } else {
            utc.checked_add(self.period.seconds())
        };
        Ok(mean.map(|mean| {
            if mean.count > self.peak.map_or(0, |p| p.demand.count) {
                self.peak = Some(Peak {
                    end_utc: utc,
                    demand: mean,
                });
            }
            Interval {
                profile_id: self.id,
                channel: self.channel,
                end_utc: utc,
                closing: reading,
                mean,
                peak: self.peak,
            }
        }))
    }

    /// Local average since the last boundary. Query strictly inside the current interval.
    pub fn provisional(
        &self,
        channel: Channel<'_>,
        utc: i64,
        reading: Reading,
    ) -> Result<Demand, Error> {
        if channel != self.channel {
            return Err(Error::WrongChannel);
        }
        let previous = self.previous.ok_or(Error::UnexpectedSlot)?;
        let next = self.next.ok_or(Error::UnexpectedSlot)?;
        let start = next - self.period.seconds();
        if utc <= start || utc >= next {
            return Err(Error::UnexpectedSlot);
        }
        self.derive(previous, reading, utc - start)
    }

    fn derive(&self, previous: Reading, current: Reading, seconds: i64) -> Result<Demand, Error> {
        let status = if self.permanent_fault {
            Status::PermanentError
        } else {
            previous.status.max(current.status)
        };
        let usable = status <= Status::Warning;
        let count = if usable {
            let delta = current
                .count
                .checked_sub(previous.count)
                .ok_or(Error::CounterDecreased)?;
            let seconds = seconds as u128;
            (u128::from(delta) * 3600 + seconds / 2) / seconds
        } else {
            0
        };
        Ok(Demand {
            count,
            status,
            validated_for_billing: usable
                && previous.validated_for_billing
                && current.validated_for_billing,
        })
    }
}
