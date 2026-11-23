#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

use core::time::Duration;

/// Returns the reconnect delay after a failed connection attempt.
///
/// The first retry uses `initial` (limited to `maximum`). Each subsequent retry
/// doubles the delay up to the cap. A zero initial delay stays zero.
///
/// ```
/// use core::time::Duration;
/// use smgw_cls::reconnect_delay;
///
/// let delay = |retry| reconnect_delay(Duration::from_secs(1), Duration::from_secs(5), retry);
/// assert_eq!(delay(0), Duration::from_secs(1));
/// assert_eq!(delay(2), Duration::from_secs(4));
/// assert_eq!(delay(u32::MAX), Duration::from_secs(5));
/// ```
pub fn reconnect_delay(initial: Duration, maximum: Duration, retry: u32) -> Duration {
    if initial.is_zero() {
        return Duration::ZERO;
    }
    let mut delay = initial.min(maximum);
    for _ in 0..retry {
        if delay == maximum {
            break;
        }
        delay = delay.saturating_mul(2).min(maximum);
    }
    delay
}

pub mod engine;
pub mod fnn;

#[cfg(feature = "runtime")]
pub mod transport;

#[cfg(feature = "fnn-bridge")]
pub mod bridge;

#[cfg(feature = "wakeup")]
pub mod wakeup;
