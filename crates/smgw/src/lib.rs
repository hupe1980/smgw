#![cfg_attr(not(feature = "wakeup-verify"), no_std)]
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

mod minute;
mod registration;

pub use smgw_cls as cls;
pub mod taf1;
pub mod taf10;
pub mod taf14;
pub mod taf15;
pub mod taf2;
pub mod taf6;
pub mod taf7;
pub mod taf8;
pub mod taf9;

#[cfg(feature = "wakeup")]
pub mod wakeup;
