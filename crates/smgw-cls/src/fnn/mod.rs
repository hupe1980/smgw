//! Logical relay commands and operator-supplied tariff windows for local simulations.

pub mod relay;
pub mod tariff;

#[cfg(feature = "fsch-tariff")]
pub mod fsch;
