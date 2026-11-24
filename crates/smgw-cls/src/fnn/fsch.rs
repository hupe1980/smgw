//! Import a fixed-start daily FSCH price schedule into the local tariff lookup.
//!
//! Both readers verify the closing header, apply the caller's policy to the original
//! start timestamp, then classify prices in the contracted unit. The provider must
//! keep individual prices stable throughout the read. Replace the active schedule
//! only after the import succeeds.
use std::io::{Read, Write};
use std::ops::{ControlFlow, Range};

use iec61850_kit::client::{Client, TariffSchedule};
use iec61850_kit::proto::data::Value;
use iec61850_kit::{Error, Fc, Result, UtcTime};

pub use iec61850_kit::client::{TariffProfile, TariffRead};

use super::tariff::{TariffLevel, TariffWindow};

/// Read the provider's UTC delivery window on an associated blocking MMS client.
///
/// For a synchronized, working clock, use
/// `|t| !t.quality.clock_not_synchronized && !t.quality.clock_failure`.
/// Returning `None` from `classify` rejects the entire import.
pub fn read_tariff_windows<S: Read + Write>(
    client: &mut Client<S>,
    node: &str,
    profile: TariffProfile<'_>,
    delivery_day: Range<u32>,
    accept_start: impl FnOnce(UtcTime) -> bool,
    classify: impl FnMut(f64) -> Option<TariffLevel>,
) -> Result<Vec<TariffWindow>> {
    let schedule = client.read_tariff_schedule(node, profile, delivery_day)?;
    tariff_windows(schedule, accept_start, classify)
}

/// Drive a configured [`TariffRead`] using an existing asynchronous MMS association.
///
/// For each batch, `read_fields` issues one MMS Read and returns decoded values in
/// request order or the original MMS error. The association driver owns invoke-ID
/// matching, deadlines and in-flight request cleanup on cancellation. Dropping
/// this future discards the unfinished schedule.
///
/// ```
/// use smgw_cls::fnn::fsch::{TariffProfile, TariffRead, read_tariff_windows_async};
/// use smgw_cls::fnn::tariff::TariffLevel;
/// # use iec61850_kit::{Fc, Result, proto::data::Value};
/// # async fn import(read_fields: impl AsyncFnMut(&[(String, Fc)]) -> Result<Vec<Value>>)
/// # -> Result<()> {
/// let profile = TariffProfile { first_price: "ValING1", ..TariffProfile::new("ct/kWh") };
/// let read = TariffRead::new("CLSLD0/FSCH1", profile, 86_400..172_800)?;
/// let windows = read_tariff_windows_async(
///     read, read_fields,
///     |t| !t.quality.clock_failure && !t.quality.clock_not_synchronized,
///     |price| match price {
///         6.0 => Some(TariffLevel::Low),
///         12.0 => Some(TariffLevel::Standard),
///         18.0 => Some(TariffLevel::High),
///         _ => None,
///     },
/// ).await?;
/// # Ok(()) }
/// ```
pub async fn read_tariff_windows_async(
    mut read: TariffRead<'_>,
    mut read_fields: impl AsyncFnMut(&[(String, Fc)]) -> Result<Vec<Value>>,
    accept_start: impl FnOnce(UtcTime) -> bool,
    classify: impl FnMut(f64) -> Option<TariffLevel>,
) -> Result<Vec<TariffWindow>> {
    loop {
        let response = read_fields(&read.fields()).await;
        match read.accept(response)? {
            ControlFlow::Continue(next) => read = next,
            ControlFlow::Break(schedule) => {
                return tariff_windows(schedule, accept_start, classify);
            }
        }
    }
}

fn tariff_windows(
    schedule: TariffSchedule,
    accept_start: impl FnOnce(UtcTime) -> bool,
    mut classify: impl FnMut(f64) -> Option<TariffLevel>,
) -> Result<Vec<TariffWindow>> {
    if !accept_start(schedule.start) {
        return Err(Error::InvalidValue("rejected FSCH start timestamp"));
    }
    schedule
        .slots
        .into_iter()
        .map(|slot| {
            let level =
                classify(slot.price).ok_or(Error::InvalidValue("unmapped FSCH tariff price"))?;
            Ok(TariffWindow {
                start_utc: i64::from(slot.utc.start),
                end_utc: i64::from(slot.utc.end),
                level,
            })
        })
        .collect()
}
