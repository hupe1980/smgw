//! Logical contacts for the FNN four-stage generation and consumption relay profile.
//!
//! Construct the initial desired state explicitly, for example from the last
//! commissioned settings. Updating a desired state only computes a command.
//! To interpret a separately supplied observation, use [`RelayContacts::decode`].
//! The snapshot has no connection or timer input and retains its value until updated.
//!
//! Contact assignments follow [section 6 of the VDE FNN recommendation](https://www.vde.com/resource/blob/2380710/0220c532a4ecec2b29c25c52858290dc/vde-fnn-hinweis-schnittstellen-steuerungseinrichtung-data.pdf).

use core::fmt;

/// Allowed generation output as a percentage of reference power.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationStage {
    /// All generation contacts inactive.
    Percent100,
    /// Only S1 active.
    Percent60,
    /// Only S2 active.
    Percent30,
    /// Only W3 active.
    Percent0,
}

impl GenerationStage {
    /// Select an exact relay stage. Other values, including nonfinite values, fail.
    pub fn from_percent(percent: f64) -> Result<Self, RelayError> {
        match percent {
            100.0 => Ok(Self::Percent100),
            60.0 => Ok(Self::Percent60),
            30.0 => Ok(Self::Percent30),
            0.0 => Ok(Self::Percent0),
            _ => Err(RelayError::UnsupportedPercentage),
        }
    }

    /// Return the allowed generation percentage.
    pub const fn percent(self) -> u8 {
        match self {
            Self::Percent100 => 100,
            Self::Percent60 => 60,
            Self::Percent30 => 30,
            Self::Percent0 => 0,
        }
    }
}

/// The W4 signal to the controlled load. The installation defines its dimmed power.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumptionMode {
    /// Unrestricted consumption.
    Released,
    /// Request the commissioned reduced consumption level.
    Dimmed,
}

/// A complete logical snapshot. Each setting is independent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayState {
    /// Generation setting on S1, S2 and W3.
    pub generation: GenerationStage,
    /// Consumption setting on W4.
    pub consumption: ConsumptionMode,
}

impl RelayState {
    /// Initialize both settings explicitly.
    pub const fn new(generation: GenerationStage, consumption: ConsumptionMode) -> Self {
        Self {
            generation,
            consumption,
        }
    }

    /// Encode all four desired logical contact signals in one snapshot.
    pub const fn contacts(self) -> RelayContacts {
        RelayContacts {
            s1: matches!(self.generation, GenerationStage::Percent60),
            s2: matches!(self.generation, GenerationStage::Percent30),
            w3: matches!(self.generation, GenerationStage::Percent0),
            w4: matches!(self.consumption, ConsumptionMode::Dimmed),
        }
    }
}

/// Logical activation signals. Field values use the FNN table's active polarity.
/// A hardware adapter must interpret actual wiring and terminal polarity separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayContacts {
    /// S1 requests 60 percent generation.
    pub s1: bool,
    /// S2 requests 30 percent generation.
    pub s2: bool,
    /// W3 requests zero generation.
    pub w3: bool,
    /// W4 requests consumption dimming.
    pub w4: bool,
}

impl RelayContacts {
    /// Decode one observation. Simultaneous active generation signals have no
    /// assigned stage in the four-stage profile.
    pub const fn decode(self) -> Result<RelayState, RelayError> {
        let generation = match (self.s1, self.s2, self.w3) {
            (false, false, false) => GenerationStage::Percent100,
            (true, false, false) => GenerationStage::Percent60,
            (false, true, false) => GenerationStage::Percent30,
            (false, false, true) => GenerationStage::Percent0,
            _ => return Err(RelayError::ConflictingGenerationContacts),
        };
        Ok(RelayState::new(
            generation,
            if self.w4 {
                ConsumptionMode::Dimmed
            } else {
                ConsumptionMode::Released
            },
        ))
    }
}

/// An unsupported command or unassigned feedback pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayError {
    /// The requested percentage has no exact generation stage.
    UnsupportedPercentage,
    /// More than one generation signal was active in the same snapshot.
    ConflictingGenerationContacts,
}

impl fmt::Display for RelayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnsupportedPercentage => "generation relay stages are 0, 30, 60 and 100 percent",
            Self::ConflictingGenerationContacts => "multiple generation relay signals are active",
        })
    }
}

impl core::error::Error for RelayError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_stage_encodes_independently_of_w4() {
        for (percent, signals) in [
            (100, [false, false, false]),
            (60, [true, false, false]),
            (30, [false, true, false]),
            (0, [false, false, true]),
        ] {
            for (mode, w4) in [
                (ConsumptionMode::Released, false),
                (ConsumptionMode::Dimmed, true),
            ] {
                let generation = GenerationStage::from_percent(f64::from(percent)).unwrap();
                let state = RelayState::new(generation, mode);
                let contacts = state.contacts();
                assert_eq!([contacts.s1, contacts.s2, contacts.w3], signals);
                assert_eq!(contacts.w4, w4);
                assert_eq!(generation.percent(), percent);
                assert_eq!(contacts.decode(), Ok(state));
            }
        }
    }

    #[test]
    fn all_unassigned_generation_patterns_are_rejected_with_either_w4_state() {
        for [s1, s2, w3] in [
            [true, true, false],
            [true, false, true],
            [false, true, true],
            [true, true, true],
        ] {
            for w4 in [false, true] {
                assert_eq!(
                    RelayContacts { s1, s2, w3, w4 }.decode(),
                    Err(RelayError::ConflictingGenerationContacts)
                );
            }
        }
    }

    #[test]
    fn percentages_are_never_rounded_to_a_stage() {
        for percent in [
            f64::NAN,
            f64::NEG_INFINITY,
            f64::INFINITY,
            -1.0,
            29.999,
            40.0,
            100.001,
        ] {
            assert_eq!(
                GenerationStage::from_percent(percent),
                Err(RelayError::UnsupportedPercentage)
            );
        }
    }
}
