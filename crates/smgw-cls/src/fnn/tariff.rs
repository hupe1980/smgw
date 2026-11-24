//! Look up the advisory Module 3 grid tariff level in a network operator's schedule.
//!
//! Supply explicit windows, including standard-tariff periods. Expand the operator's
//! dates and local times to UTC upstream, applying the local time zone's rules for
//! skipped and repeated hours. The lookup only identifies the active level.

use core::fmt;

/// The three Module 3 grid tariff levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TariffLevel {
    /// Standard tariff (ST).
    Standard,
    /// Low-load tariff (NT).
    Low,
    /// High-load tariff (HT).
    High,
}

/// An operator-supplied interval measured in whole Unix seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TariffWindow {
    /// First second in this window.
    pub start_utc: i64,
    /// First second after this window.
    pub end_utc: i64,
    /// Advisory level during this window.
    pub level: TariffLevel,
}

/// Why the supplied set of windows cannot be used. Indexes are zero-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScheduleError {
    /// No interval was supplied.
    Empty,
    /// The end of this interval is at or before its start.
    InvalidInterval { index: usize },
    /// This interval starts after the preceding interval ends.
    Gap { index: usize },
    /// This interval starts before the preceding interval ends.
    OverlapOrOutOfOrder { index: usize },
}

impl fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("the tariff schedule is empty"),
            Self::InvalidInterval { index } => {
                write!(f, "tariff window {index} must end after its start")
            }
            Self::Gap { index } => write!(f, "gap before tariff window {index}"),
            Self::OverlapOrOutOfOrder { index } => write!(
                f,
                "tariff window {index} overlaps or precedes the previous window"
            ),
        }
    }
}

impl core::error::Error for ScheduleError {}

/// A finite, contiguous sequence of validated UTC tariff windows.
///
/// Construction checks the borrowed slice once. Lookups take logarithmic time.
/// Times outside the supplied coverage return `None`.
///
/// ```
/// use smgw_cls::fnn::tariff::{TariffLevel, TariffSchedule, TariffWindow};
///
/// let windows = [
///     TariffWindow { start_utc: 0, end_utc: 3600, level: TariffLevel::Low },
///     TariffWindow { start_utc: 3600, end_utc: 7200, level: TariffLevel::Standard },
/// ];
/// let schedule = TariffSchedule::new(&windows)?;
/// assert_eq!(schedule.active_at(3600), Some(&windows[1]));
/// assert_eq!(schedule.active_at(7200), None);
/// # Ok::<(), smgw_cls::fnn::tariff::ScheduleError>(())
/// ```
#[derive(Clone, Copy, Debug)]
pub struct TariffSchedule<'a> {
    windows: &'a [TariffWindow],
}

impl<'a> TariffSchedule<'a> {
    /// Validate the windows in the order supplied.
    pub fn new(windows: &'a [TariffWindow]) -> Result<Self, ScheduleError> {
        let first = windows.first().ok_or(ScheduleError::Empty)?;
        let mut previous_end = first.start_utc;
        for (index, window) in windows.iter().enumerate() {
            if window.end_utc <= window.start_utc {
                return Err(ScheduleError::InvalidInterval { index });
            }
            match window.start_utc.cmp(&previous_end) {
                core::cmp::Ordering::Greater => return Err(ScheduleError::Gap { index }),
                core::cmp::Ordering::Less => {
                    return Err(ScheduleError::OverlapOrOutOfOrder { index });
                }
                core::cmp::Ordering::Equal => previous_end = window.end_utc,
            }
        }
        Ok(Self { windows })
    }

    /// Return the window whose half-open interval contains this Unix second.
    pub fn active_at(&self, utc: i64) -> Option<&'a TariffWindow> {
        let index = self.windows.partition_point(|window| window.end_utc <= utc);
        self.windows
            .get(index)
            .filter(|window| window.start_utc <= utc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(start_utc: i64, end_utc: i64, level: TariffLevel) -> TariffWindow {
        TariffWindow {
            start_utc,
            end_utc,
            level,
        }
    }

    #[test]
    fn lookup_uses_half_open_intervals_and_stops_at_coverage_edges() {
        let windows = [
            window(-10, 0, TariffLevel::Standard),
            window(0, 1, TariffLevel::High),
            window(1, 2, TariffLevel::Low),
            window(2, 5, TariffLevel::Low),
        ];
        let schedule = TariffSchedule::new(&windows).unwrap();
        for (utc, index) in [(-10, 0), (-1, 0), (0, 1), (1, 2), (2, 3), (4, 3)] {
            assert_eq!(schedule.active_at(utc), Some(&windows[index]));
        }
        for utc in [i64::MIN, -11, 5, 6, i64::MAX] {
            assert_eq!(schedule.active_at(utc), None);
        }
    }

    #[test]
    fn malformed_schedules_report_the_first_invalid_window() {
        use ScheduleError::*;
        let base = window(10, 20, TariffLevel::Standard);
        assert_eq!(TariffSchedule::new(&[]).unwrap_err(), Empty);
        for (start, end, error) in [
            (20, 20, InvalidInterval { index: 1 }),
            (20, 19, InvalidInterval { index: 1 }),
            (21, 30, Gap { index: 1 }),
            (19, 30, OverlapOrOutOfOrder { index: 1 }),
            (10, 20, OverlapOrOutOfOrder { index: 1 }),
            (0, 10, OverlapOrOutOfOrder { index: 1 }),
        ] {
            assert_eq!(
                TariffSchedule::new(&[base, window(start, end, TariffLevel::Low)]).unwrap_err(),
                error
            );
        }
        assert_eq!(
            TariffSchedule::new(&[window(0, 0, TariffLevel::High)]).unwrap_err(),
            InvalidInterval { index: 0 }
        );
    }

    #[test]
    fn single_window_supports_full_timestamp_range_without_arithmetic_overflow() {
        let windows = [window(i64::MIN, i64::MAX, TariffLevel::High)];
        let schedule = TariffSchedule::new(&windows).unwrap();
        assert_eq!(schedule.active_at(i64::MIN), Some(&windows[0]));
        assert_eq!(schedule.active_at(i64::MAX - 1), Some(&windows[0]));
        assert_eq!(schedule.active_at(i64::MAX), None);
    }
}
