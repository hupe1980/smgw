//! Bounded TAF 14 measurement blocks for value-added services (BSI TR-03109-1 v1.1).
//!
//! Commission one channel per profile. The host authenticates and qualifies its
//! samples, enforces consent and profile validity, and delivers each emitted block.

pub use crate::{minute::Crossing, taf7::Direction, taf9::ActivePower, taf10::Phase};
use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quadrant {
    R1,
    R2,
    R3,
    R4,
}

/// Electricity channels from table 4.11. An absent phase selects the meter total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantity {
    ActiveEnergy(Direction, Option<Phase>),
    ReactiveEnergy(Quadrant),
    ActivePower(ActivePower),
}

impl Quantity {
    pub const fn obis(self) -> &'static str {
        use Direction::{Export, Import};
        use Phase::{L1, L2, L3};
        match self {
            Self::ActiveEnergy(Import, None) => "1-0:1.8.0.255",
            Self::ActiveEnergy(Import, Some(L1)) => "1-0:21.8.0.255",
            Self::ActiveEnergy(Import, Some(L2)) => "1-0:41.8.0.255",
            Self::ActiveEnergy(Import, Some(L3)) => "1-0:61.8.0.255",
            Self::ActiveEnergy(Export, None) => "1-0:2.8.0.255",
            Self::ActiveEnergy(Export, Some(L1)) => "1-0:22.8.0.255",
            Self::ActiveEnergy(Export, Some(L2)) => "1-0:42.8.0.255",
            Self::ActiveEnergy(Export, Some(L3)) => "1-0:62.8.0.255",
            Self::ReactiveEnergy(Quadrant::R1) => "1-0:5.8.0.255",
            Self::ReactiveEnergy(Quadrant::R2) => "1-0:6.8.0.255",
            Self::ReactiveEnergy(Quadrant::R3) => "1-0:7.8.0.255",
            Self::ReactiveEnergy(Quadrant::R4) => "1-0:8.8.0.255",
            Self::ActivePower(power) => power.obis(),
        }
    }

    fn accepts(self, value: Value) -> bool {
        matches!(
            (self, value),
            (Self::ActiveEnergy(..), Value::WattHours(_))
                | (Self::ReactiveEnergy(_), Value::VarHours(_))
                | (Self::ActivePower(_), Value::Watts(_))
        )
    }
}

/// Exact whole units after the host applies the meter scaler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    WattHours(u64),
    VarHours(u64),
    Watts(i64),
}

impl Value {
    fn amount(self) -> i128 {
        match self {
            Self::WattHours(value) | Self::VarHours(value) => value.into(),
            Self::Watts(value) => value.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Channel<'a> {
    pub meter_id: &'a str,
    pub quantity: Quantity,
}

#[derive(Debug, Clone, Copy)]
pub struct Config<'a> {
    pub profile_id: &'a str,
    pub channel: Channel<'a>,
    /// UTC seconds of the first slot.
    pub first_utc: i64,
    pub reception_seconds: u32,
    /// The first delivery boundary is the last slot of the first full period.
    pub delivery_every_slots: Option<usize>,
    /// Strictly increasing values in the channel's unit.
    pub thresholds: &'a [Value],
}

impl<'a> Config<'a> {
    /// Deliver each qualified sample immediately, with one reception slot per minute.
    pub const fn new(profile_id: &'a str, channel: Channel<'a>, first_utc: i64) -> Self {
        Self {
            profile_id,
            channel,
            first_utc,
            reception_seconds: 60,
            delivery_every_slots: None,
            thresholds: &[],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub sampled_utc: i64,
    pub value: Value,
}

/// Every threshold crossed by the same sample, listed in increasing order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThresholdCrossing<'a> {
    pub sampled_utc: i64,
    pub direction: Crossing,
    pub thresholds: &'a [Value],
}

fn crossed(thresholds: &[Value], previous: Value, sample: Sample) -> Option<ThresholdCrossing<'_>> {
    let previous = previous.amount();
    let current = sample.value.amount();
    let (direction, first, end) = match current.cmp(&previous) {
        core::cmp::Ordering::Greater => (
            Crossing::Above,
            thresholds.partition_point(|t| t.amount() < previous),
            thresholds.partition_point(|t| t.amount() < current),
        ),
        core::cmp::Ordering::Less => (
            Crossing::Below,
            thresholds.partition_point(|t| t.amount() <= current),
            thresholds.partition_point(|t| t.amount() <= previous),
        ),
        core::cmp::Ordering::Equal => return None,
    };
    (first < end).then(|| ThresholdCrossing {
        sampled_utc: sample.sampled_utc,
        direction,
        thresholds: &thresholds[first..end],
    })
}

#[derive(Debug, PartialEq, Eq)]
pub struct Block<'a> {
    pub profile_id: &'a str,
    pub channel: Channel<'a>,
    /// Qualified observations in ascending time order. Always nonempty.
    pub samples: &'a [Sample],
    /// Older qualified samples displaced since the previous block.
    pub omitted: u64,
    pub periodic: bool,
    pub crossing: Option<ThresholdCrossing<'a>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    EmptyIdentifier,
    InvalidCapacity,
    InvalidPeriod,
    UnorderedThresholds,
    WrongUnit,
    WrongChannel,
    UnexpectedSlot,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::EmptyIdentifier => "supply nonempty profile and meter identifiers",
            Self::InvalidCapacity => {
                "provide storage for at least one sample and the entire configured delivery period"
            }
            Self::InvalidPeriod => "reception and delivery periods must be positive",
            Self::UnorderedThresholds => "supply thresholds in strictly increasing order",
            Self::WrongUnit => "use the commissioned channel's unit",
            Self::WrongChannel => "meter or quantity does not match the commissioned channel",
            Self::UnexpectedSlot => "submit the next commissioned reception slot",
        })
    }
}

impl core::error::Error for Error {}

/// Retains the latest N qualified samples since the previous block. Consume or
/// copy each block before recording the next slot.
pub struct Profile<'a, const N: usize> {
    config: Config<'a>,
    next_utc: Option<i64>,
    slots_left: Option<usize>,
    previous: Option<Value>,
    buffer: [Sample; N],
    write: usize,
    len: usize,
    omitted: u64,
}

impl<'a, const N: usize> Profile<'a, N> {
    pub fn new(config: Config<'a>) -> Result<Self, Error> {
        if config.profile_id.trim().is_empty() || config.channel.meter_id.trim().is_empty() {
            return Err(Error::EmptyIdentifier);
        }
        if config.reception_seconds == 0 || config.delivery_every_slots == Some(0) {
            return Err(Error::InvalidPeriod);
        }
        if N == 0 || config.delivery_every_slots.is_some_and(|slots| slots > N) {
            return Err(Error::InvalidCapacity);
        }
        if config
            .thresholds
            .iter()
            .any(|&t| !config.channel.quantity.accepts(t))
        {
            return Err(Error::WrongUnit);
        }
        if config
            .thresholds
            .windows(2)
            .any(|pair| pair[0].amount() >= pair[1].amount())
        {
            return Err(Error::UnorderedThresholds);
        }
        Ok(Self {
            config,
            next_utc: Some(config.first_utc),
            slots_left: config.delivery_every_slots,
            previous: None,
            buffer: [Sample {
                sampled_utc: 0,
                value: Value::Watts(0),
            }; N],
            write: 0,
            len: 0,
            omitted: 0,
        })
    }

    /// Submit each reception slot, passing `None` for a missing or invalid sample.
    /// A gap breaks threshold comparison and still advances the periodic schedule.
    /// Rejected inputs leave the slot and pending block intact.
    pub fn record(
        &mut self,
        channel: Channel<'_>,
        utc: i64,
        value: Option<Value>,
    ) -> Result<Option<Block<'_>>, Error> {
        if channel != self.config.channel {
            return Err(Error::WrongChannel);
        }
        if self.next_utc != Some(utc) {
            return Err(Error::UnexpectedSlot);
        }
        if value.is_some_and(|v| !channel.quantity.accepts(v)) {
            return Err(Error::WrongUnit);
        }

        self.next_utc = utc.checked_add(self.config.reception_seconds.into());
        let periodic = self.slots_left == Some(1);
        self.slots_left = if periodic {
            self.config.delivery_every_slots
        } else {
            self.slots_left.map(|s| s - 1)
        };

        let previous = core::mem::replace(&mut self.previous, value);
        let crossing = if let Some(value) = value {
            let sample = Sample {
                sampled_utc: utc,
                value,
            };
            self.buffer[self.write] = sample;
            self.write = (self.write + 1) % N;
            if self.len == N {
                self.omitted += 1;
            } else {
                self.len += 1;
            }
            previous.and_then(|prev| crossed(self.config.thresholds, prev, sample))
        } else {
            None
        };

        let immediate =
            self.config.delivery_every_slots.is_none() && self.config.thresholds.is_empty();
        if self.len == 0 || !(periodic || immediate || crossing.is_some()) {
            return Ok(None);
        }
        if self.len == N {
            self.buffer.rotate_left(self.write);
        }
        self.write = 0;
        let len = core::mem::take(&mut self.len);
        Ok(Some(Block {
            profile_id: self.config.profile_id,
            channel: self.config.channel,
            samples: &self.buffer[..len],
            omitted: core::mem::take(&mut self.omitted),
            periodic,
            crossing,
        }))
    }
}
