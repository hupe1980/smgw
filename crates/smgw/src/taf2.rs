//! Single-meter TAF 2 register simulation for quarter-hour electricity readings.
//!
//! Use one instance per meter, energy direction and evaluation profile. The caller
//! authenticates the source, classifies its status and supplies the commissioned
//! register for the entire elapsed period. Store original observations separately.

use core::fmt;

/// A commissioned tariff register (1 through 62 in this simulator).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TariffRegister(u8);

impl TariffRegister {
    /// Reserve 0 for the total and 63 for energy with uncertain attribution.
    pub fn new(number: u8) -> Result<Self, Error> {
        match number {
            1..=62 => Ok(Self(number)),
            _ => Err(Error::InvalidRegister),
        }
    }

    /// Numeric identifier within the evaluation profile.
    pub const fn number(self) -> u8 {
        self.0
    }
}

/// Meter status at a scheduled registration instant, supplied by the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reading {
    /// Timely, validated cumulative energy in Wh.
    Valid(u64),
    /// No reading arrived within the receiving window.
    Missing,
    /// Clock quality or reception timing precludes using the value.
    TimeInvalid,
    /// Timely cumulative Wh carrying a nonfatal meter fault.
    MeterFault(u64),
    /// End the trace for this meter under the simulator's stop-on-fatal policy.
    Fatal,
}

/// Where to book the measured energy delta, in addition to the total.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination {
    Tariff(TariffRegister),
    ErrorRegister,
}

/// Result of consuming one slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Booked { wh: u64, destination: Destination },
    Deferred,
    Stopped,
}

/// A rejected operation leaves all accumulator state intact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidRegister,
    UnalignedBaseline,
    UnexpectedSlot,
    DecreasingReading,
    MeterStopped,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidRegister => "tariff register must be in 1..=62",
            Self::UnalignedBaseline => "baseline must be on a UTC quarter-hour",
            Self::UnexpectedSlot => "expected the next UTC quarter-hour",
            Self::DecreasingReading => "cumulative Wh decreased",
            Self::MeterStopped => "meter stopped after fatal fault",
        })
    }
}

impl core::error::Error for Error {}

/// Fixed-size derived registers for one continuous, nondecreasing register trace.
#[derive(Debug)]
pub struct Registers {
    wh: [u64; 64],
    last_slot: i64,
    last_wh: u64,
    // Attribution for energy accrued but not yet booked, including fault recovery.
    pending: Option<Destination>,
    stopped: bool,
}

impl Registers {
    /// Start at a validated cumulative reading. All derived registers start at zero.
    pub fn new(start_utc: i64, baseline_wh: u64) -> Result<Self, Error> {
        if start_utc % 900 != 0 {
            return Err(Error::UnalignedBaseline);
        }
        Ok(Self {
            wh: [0; 64],
            last_slot: start_utc,
            last_wh: baseline_wh,
            pending: None,
            stopped: false,
        })
    }

    pub fn total_wh(&self) -> u64 {
        self.wh[0]
    }
    pub fn error_wh(&self) -> u64 {
        self.wh[63]
    }
    pub fn tariff_wh(&self, tariff: TariffRegister) -> u64 {
        self.wh[usize::from(tariff.0)]
    }

    /// Submit one elapsed period, even if its reading is missing. At a switch on
    /// the closing boundary, supply the outgoing register. After `Fatal`, start
    /// a new instance for the replacement meter.
    pub fn record(
        &mut self,
        end_utc: i64,
        tariff: TariffRegister,
        reading: Reading,
    ) -> Result<Outcome, Error> {
        if self.stopped {
            return Err(Error::MeterStopped);
        }
        if self.last_slot.checked_add(900) != Some(end_utc) {
            return Err(Error::UnexpectedSlot);
        }
        let period = Destination::Tariff(tariff);
        let mut destination = match self.pending {
            Some(previous) if previous != period => Destination::ErrorRegister,
            _ => period,
        };
        let outcome = match reading {
            Reading::Missing | Reading::TimeInvalid => {
                self.pending = Some(destination);
                Outcome::Deferred
            }
            Reading::Fatal => {
                self.stopped = true;
                Outcome::Stopped
            }
            Reading::Valid(value) | Reading::MeterFault(value) => {
                let delta = value
                    .checked_sub(self.last_wh)
                    .ok_or(Error::DecreasingReading)?;
                let fault = matches!(reading, Reading::MeterFault(_));
                if fault {
                    destination = Destination::ErrorRegister;
                }
                let index = match destination {
                    Destination::Tariff(id) => usize::from(id.0),
                    Destination::ErrorRegister => 63,
                };
                // All deltas telescope to value - baseline_wh, which fits in u64.
                self.wh[0] += delta;
                self.wh[index] += delta;
                self.last_wh = value;
                self.pending = fault.then_some(Destination::ErrorRegister);
                Outcome::Booked {
                    wh: delta,
                    destination,
                }
            }
        };
        self.last_slot = end_utc;
        Ok(outcome)
    }
}
