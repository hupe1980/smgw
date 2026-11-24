//! TAF 10 grid-state measurements for one commissioned meter and OBIS channel.
//!
//! Supply qualified instantaneous samples every 60 seconds. Each notification is
//! one value. The host enforces profile validity, authenticates inputs, authorizes
//! recipients and handles access logging and delivery.

use crate::minute::Samples;
pub use crate::{minute::Crossing, taf9::ActivePower};
use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    L1,
    L2,
    L3,
}

/// Permitted phase differences. The named reference voltage is the second operand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseAngle {
    VoltageL2ToL1,
    VoltageL3ToL1,
    CurrentToVoltage(Phase),
}

/// BSI TR-03109-1 v1.1, table 4.9.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantity {
    ActivePower(ActivePower),
    Current(Phase),
    Frequency,
    PhaseAngle(PhaseAngle),
    Voltage(Phase),
}

impl Quantity {
    pub const fn obis(self) -> &'static str {
        match self {
            Self::ActivePower(power) => power.obis(),
            Self::Current(Phase::L1) => "1-0:31.7.0.255",
            Self::Current(Phase::L2) => "1-0:51.7.0.255",
            Self::Current(Phase::L3) => "1-0:71.7.0.255",
            Self::Frequency => "1-0:14.7.0.255",
            Self::PhaseAngle(PhaseAngle::VoltageL2ToL1) => "1-0:81.7.1.255",
            Self::PhaseAngle(PhaseAngle::VoltageL3ToL1) => "1-0:81.7.2.255",
            Self::PhaseAngle(PhaseAngle::CurrentToVoltage(Phase::L1)) => "1-0:81.7.4.255",
            Self::PhaseAngle(PhaseAngle::CurrentToVoltage(Phase::L2)) => "1-0:81.7.15.255",
            Self::PhaseAngle(PhaseAngle::CurrentToVoltage(Phase::L3)) => "1-0:81.7.26.255",
            Self::Voltage(Phase::L1) => "1-0:32.7.0.255",
            Self::Voltage(Phase::L2) => "1-0:52.7.0.255",
            Self::Voltage(Phase::L3) => "1-0:72.7.0.255",
        }
    }

    fn accepts(self, value: Value) -> bool {
        matches!(
            (self, value),
            (Self::ActivePower(_), Value::Watts(_))
                | (Self::Current(_), Value::Milliamps(_))
                | (Self::Frequency, Value::Millihertz(_))
                | (Self::PhaseAngle(_), Value::Millidegrees(_))
                | (Self::Voltage(_), Value::Millivolts(_))
        )
    }
}

/// Exact simulator units. Normalize meter scalers before submitting a value.
/// Angle thresholds use the same signed linear representation as the samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    Watts(i64),
    Milliamps(u32),
    Millihertz(u32),
    Millidegrees(i32),
    Millivolts(u32),
}

impl Value {
    fn amount(self) -> i64 {
        match self {
            Self::Watts(n) => n,
            Self::Millivolts(n) | Self::Milliamps(n) | Self::Millihertz(n) => n.into(),
            Self::Millidegrees(n) => n.into(),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let unit = match self {
            Self::Watts(_) => "W",
            Self::Milliamps(_) => "mA",
            Self::Millihertz(_) => "mHz",
            Self::Millidegrees(_) => "millidegrees",
            Self::Millivolts(_) => "mV",
        };
        write!(f, "{} {unit}", self.amount())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Channel<'a> {
    pub meter_id: &'a str,
    pub quantity: Quantity,
}

/// Configuration for a single-value block at each commissioned minute.
pub struct Config<'a> {
    pub profile_id: &'a str,
    pub channel: Channel<'a>,
    /// When set, all reports substitute this reference for the physical meter ID.
    /// Use a non-identifying profile ID and a separately commissioned pseudonym.
    pub pseudonym: Option<&'a str>,
    pub first_utc: i64,
    pub periodic: bool,
    pub threshold: Option<Value>,
}

/// An outward reading. Only the configured reporting reference is carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading<'a> {
    pub profile_id: &'a str,
    pub meter_reference: &'a str,
    pub quantity: Quantity,
    pub sampled_utc: i64,
    pub value: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Notification<'a> {
    pub reading: Reading<'a>,
    pub periodic: bool,
    pub crossing: Option<Crossing>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    EmptyIdentifier,
    IdentifyingReport,
    WrongChannel,
    WrongUnit,
    UnexpectedSlot,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::EmptyIdentifier => {
                "supply nonempty profile, meter and configured pseudonym identifiers"
            }
            Self::IdentifyingReport => {
                "the pseudonymous report must use identifiers distinct from the physical meter ID"
            }
            Self::WrongChannel => "meter or grid-state quantity does not match",
            Self::WrongUnit => "use the commissioned quantity's unit",
            Self::UnexpectedSlot => "submit the next commissioned 60-second slot",
        })
    }
}
impl core::error::Error for Error {}

/// Fixed-storage evaluator. Input rejection leaves the current reading and cursor intact.
pub struct Profile<'a> {
    profile_id: &'a str,
    channel: Channel<'a>,
    meter_reference: &'a str,
    samples: Samples<Value>,
}

impl<'a> Profile<'a> {
    pub fn new(config: Config<'a>) -> Result<Self, Error> {
        let meter = config.channel.meter_id.trim();
        let reference = config.pseudonym.unwrap_or(config.channel.meter_id);
        if meter.is_empty() || config.profile_id.trim().is_empty() || reference.trim().is_empty() {
            return Err(Error::EmptyIdentifier);
        }
        if config.pseudonym.is_some()
            && (meter == reference.trim() || meter == config.profile_id.trim())
        {
            return Err(Error::IdentifyingReport);
        }
        if config
            .threshold
            .is_some_and(|v| !config.channel.quantity.accepts(v))
        {
            return Err(Error::WrongUnit);
        }
        Ok(Self {
            profile_id: config.profile_id,
            channel: config.channel,
            meter_reference: reference,
            samples: Samples::new(
                config.first_utc,
                config.periodic,
                config.threshold.map(Value::amount),
            ),
        })
    }

    /// Submit an authenticated physical channel and its qualified sample.
    /// A missing or invalid observation is represented by `None`.
    pub fn record(
        &mut self,
        channel: Channel<'_>,
        sampled_utc: i64,
        value: Option<Value>,
    ) -> Result<Option<Notification<'a>>, Error> {
        if self.channel != channel {
            return Err(Error::WrongChannel);
        }
        if value.is_some_and(|v| !channel.quantity.accepts(v)) {
            return Err(Error::WrongUnit);
        }
        let event = self
            .samples
            .record(sampled_utc, value, Value::amount)
            .map_err(|_| Error::UnexpectedSlot)?;
        Ok(event.map(|(value, crossing)| Notification {
            reading: self.reading(sampled_utc, value),
            periodic: self.samples.periodic,
            crossing,
        }))
    }

    /// Select a fresh sample after the host has authorized the on-demand request.
    pub fn on_demand(&self, now_utc: i64) -> Option<Reading<'a>> {
        self.samples
            .current(now_utc)
            .map(|(utc, v)| self.reading(utc, v))
    }

    fn reading(&self, sampled_utc: i64, value: Value) -> Reading<'a> {
        Reading {
            profile_id: self.profile_id,
            meter_reference: self.meter_reference,
            quantity: self.channel.quantity,
            sampled_utc,
            value,
        }
    }
}
