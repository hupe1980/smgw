//! TAF 6 retrospective selection of a daily billing-register balance.
//!
//! Use one history per commissioned quantity. The application authenticates the
//! meter, resolves local billing-day boundaries, and supplies a qualified balance
//! at the opening. Authorize the recipient and exceptional purpose for the exact
//! quantity and date before lookup. The application owns durable storage, access
//! logs, signing and delivery.

use crate::{taf2::TariffRegister, taf7::Channel};
use core::fmt;

/// Quantity selected within an electricity evaluation profile, in whole Wh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Register {
    Meter,
    DerivedTotal,
    DerivedTariff(TariffRegister),
    DerivedError,
}

/// The scope shared by an opening balance and its authorized lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quantity<'a> {
    pub profile_id: &'a str,
    pub channel: Channel<'a>,
    pub register: Register,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DailyBalance<'a> {
    pub quantity: Quantity<'a>,
    pub opening_utc: i64,
    pub wh: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    EmptyIdentifier,
    InvalidCalendar,
    WrongQuantity,
    OutsideCalendar,
    NotDayOpening,
    DayAlreadyRecorded,
    DayWentBackwards,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::EmptyIdentifier => "supply the evaluation profile and meter identifiers",
            Self::InvalidCalendar => "supply ordered daily openings and a final exclusive boundary",
            Self::WrongQuantity => "profile, channel or register does not match the history",
            Self::OutsideCalendar => "the billing calendar does not cover this instant",
            Self::NotDayOpening => "expected the exact opening of a billing day",
            Self::DayAlreadyRecorded => "the opening balance has already been recorded",
            Self::DayWentBackwards => "cannot move the billing day backwards",
        })
    }
}

impl core::error::Error for Error {}

const DAYS: usize = 42;

/// A rolling in-memory history covering the current billing day and its 41 predecessors.
pub struct History<'a> {
    quantity: Quantity<'a>,
    boundaries: &'a [i64],
    current_day: Option<usize>,
    balances: [Option<u64>; DAYS],
}

impl<'a> History<'a> {
    /// Supply every consecutive local day opening in UTC, followed by the exclusive end.
    /// This electricity profile accepts 23-, 24- and 25-hour billing days.
    pub fn new(quantity: Quantity<'a>, boundaries: &'a [i64]) -> Result<Self, Error> {
        if quantity.profile_id.trim().is_empty() || quantity.channel.meter_id.trim().is_empty() {
            return Err(Error::EmptyIdentifier);
        }
        // German billing days may gain or lose one hour at a clock change.
        if boundaries.len() < 2
            || boundaries
                .windows(2)
                .any(|pair| !matches!(pair[1].checked_sub(pair[0]), Some(82_800 | 86_400 | 90_000)))
        {
            return Err(Error::InvalidCalendar);
        }
        Ok(Self {
            quantity,
            boundaries,
            current_day: None,
            balances: [None; DAYS],
        })
    }

    /// Finalize a qualified daily balance once. Omit days with unavailable readings.
    pub fn record(
        &mut self,
        quantity: Quantity<'_>,
        opening_utc: i64,
        wh: u64,
    ) -> Result<(), Error> {
        if quantity != self.quantity {
            return Err(Error::WrongQuantity);
        }
        let day = self.day_at(opening_utc)?;
        if self.boundaries[day] != opening_utc {
            return Err(Error::NotDayOpening);
        }
        self.advance(day)?;
        let slot = &mut self.balances[day % DAYS];
        if slot.is_some() {
            return Err(Error::DayAlreadyRecorded);
        }
        *slot = Some(wh);
        Ok(())
    }

    /// Advance retention to the trusted current instant, even if capture has stopped.
    pub fn advance_to(&mut self, now_utc: i64) -> Result<(), Error> {
        self.advance(self.day_at(now_utc)?)
    }

    /// Select one exact opening after authorization. Missing, expired, future and
    /// non-opening requests return `None`. A lookup advances retention to `now_utc`.
    pub fn lookup(
        &mut self,
        quantity: Quantity<'_>,
        opening_utc: i64,
        now_utc: i64,
    ) -> Result<Option<DailyBalance<'a>>, Error> {
        if quantity != self.quantity {
            return Err(Error::WrongQuantity);
        }
        let today = self.day_at(now_utc)?;
        self.advance(today)?;
        let Ok(day) = self.boundaries.binary_search(&opening_utc) else {
            return Ok(None);
        };
        if day > today || today - day >= DAYS {
            return Ok(None);
        }
        Ok(self.balances[day % DAYS].map(|wh| DailyBalance {
            quantity: self.quantity,
            opening_utc,
            wh,
        }))
    }

    fn day_at(&self, instant: i64) -> Result<usize, Error> {
        let end = self.boundaries.partition_point(|&start| start <= instant);
        if end == 0 || end == self.boundaries.len() {
            Err(Error::OutsideCalendar)
        } else {
            Ok(end - 1)
        }
    }

    fn advance(&mut self, day: usize) -> Result<(), Error> {
        if let Some(previous) = self.current_day {
            let elapsed = day.checked_sub(previous).ok_or(Error::DayWentBackwards)?;
            // Clear the slots entering the window, including days skipped during an outage.
            for offset in 0..elapsed.min(DAYS) {
                self.balances[(day - offset) % DAYS] = None;
            }
        }
        self.current_day = Some(day);
        Ok(())
    }
}
