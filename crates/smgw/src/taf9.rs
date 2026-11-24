//! TAF 9 single-channel, instantaneous active-power simulation.
//!
//! Supply qualified meter measurements in signed whole watts, using the commissioned
//! meter's sign convention for both samples and threshold. The caller authenticates
//! the meter, authorizes requests and handles recipient logging and delivery.

pub use crate::minute::Crossing;
use crate::minute::Samples;
use core::fmt;

/// Active-power quantities in BSI TR-03109-1 v1.1, table 4.7.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePower {
    Total,
    L1,
    L2,
    L3,
}

impl ActivePower {
    pub const fn obis(self) -> &'static str {
        match self {
            Self::Total => "1-0:16.7.0.255",
            Self::L1 => "1-0:36.7.0.255",
            Self::L2 => "1-0:56.7.0.255",
            Self::L3 => "1-0:76.7.0.255",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Channel<'a> {
    pub meter_id: &'a str,
    pub quantity: ActivePower,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading<'a> {
    pub profile_id: &'a str,
    pub channel: Channel<'a>,
    pub sampled_utc: i64,
    pub watts: i64,
}

/// One measurement may satisfy both configured triggers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Notification<'a> {
    pub reading: Reading<'a>,
    pub periodic: bool,
    pub crossing: Option<Crossing>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    EmptyIdentifier,
    WrongChannel,
    UnexpectedSlot,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::EmptyIdentifier => "supply profile and meter identifiers",
            Self::WrongChannel => "meter or active-power quantity does not match",
            Self::UnexpectedSlot => "submit the next commissioned 60-second slot",
        })
    }
}
impl core::error::Error for Error {}

/// Single-value profile with a 60-second reception and optional 60-second reporting period.
pub struct Profile<'a> {
    id: &'a str,
    channel: Channel<'a>,
    samples: Samples<i64>,
}

impl<'a> Profile<'a> {
    /// Commission the first sample instant, periodic reporting and an optional
    /// threshold in the same signed watts as the meter. The first valid sample
    /// sets the baseline. A crossing occurs when a later sample moves strictly
    /// above or below the threshold from the other side or from equality.
    pub fn new(
        id: &'a str,
        channel: Channel<'a>,
        first_utc: i64,
        periodic: bool,
        threshold_w: Option<i64>,
    ) -> Result<Self, Error> {
        if id.trim().is_empty() || channel.meter_id.trim().is_empty() {
            return Err(Error::EmptyIdentifier);
        }
        Ok(Self {
            id,
            channel,
            samples: Samples::new(first_utc, periodic, threshold_w),
        })
    }

    /// Submit each scheduled sample, including gaps as `None`. A gap invalidates
    /// on-demand output and breaks the comparison with the previous sample.
    pub fn record(
        &mut self,
        channel: Channel<'_>,
        sampled_utc: i64,
        watts: Option<i64>,
    ) -> Result<Option<Notification<'a>>, Error> {
        if channel != self.channel {
            return Err(Error::WrongChannel);
        }
        let event = self
            .samples
            .record(sampled_utc, watts, |w| w)
            .map_err(|_| Error::UnexpectedSlot)?;
        Ok(event.map(|(watts, crossing)| Notification {
            reading: self.reading(sampled_utc, watts),
            periodic: self.samples.periodic,
            crossing,
        }))
    }

    fn reading(&self, sampled_utc: i64, watts: i64) -> Reading<'a> {
        Reading {
            profile_id: self.id,
            channel: self.channel,
            sampled_utc,
            watts,
        }
    }

    /// Select the latest valid reading for an authorized request. It expires at
    /// the next reception slot even when the caller has stopped submitting data.
    pub fn on_demand(&self, now_utc: i64) -> Option<Reading<'a>> {
        self.samples
            .current(now_utc)
            .map(|(utc, watts)| self.reading(utc, watts))
    }
}
