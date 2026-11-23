//! Commissioned IEC controls and independent contact feedback for a simulated relay bank.
use super::{BridgeError, admit_operate, percentage, validate_controls, validate_reference};
use crate::fnn::relay::{ConsumptionMode, GenerationStage, RelayContacts, RelayState};
use core::time::Duration;
use iec61850_kit::{
    Fc,
    proto::{
        data::Value,
        mms::{ConfirmedRequest, ObjectName, VariableAccess, VariableSpecification},
    },
    server::{Answer, ControlEvent, DATA_ACCESS_NON_EXISTENT},
};

const TEMPORARILY_UNAVAILABLE: i64 = 2; // MMS DataAccessError

/// Maps two authorized Operate events into a caller-owned command snapshot.
/// The consumption control uses true for dimming and false for release.
pub struct RelayBridge {
    generation: String,
    consumption: String,
}

impl RelayBridge {
    /// Configure two distinct control object references from the installation's SCL model.
    pub fn new(generation: &str, consumption: &str) -> Result<Self, BridgeError> {
        validate_controls(generation, consumption)?;
        Ok(Self {
            generation: generation.into(),
            consumption: consumption.into(),
        })
    }

    /// Update only the addressed function after all checks pass. The caller
    /// authenticates and authorizes the event before invoking this method.
    pub fn apply(
        &self,
        desired: &mut RelayState,
        event: &ControlEvent,
        expires_at: Duration,
        now: Duration,
    ) -> Result<(), BridgeError> {
        admit_operate(event, expires_at, now)?;
        if event.object == self.generation {
            desired.generation = GenerationStage::from_percent(percentage(&event.request.ctl_val)?)
                .map_err(|_| BridgeError::InvalidValue)?;
        } else if event.object == self.consumption {
            let Value::Boolean(dimmed) = event.request.ctl_val else {
                return Err(BridgeError::InvalidValue);
            };
            desired.consumption = if dimmed {
                ConsumptionMode::Dimmed
            } else {
                ConsumptionMode::Released
            };
        } else {
            return Err(BridgeError::UnmappedControl);
        }
        Ok(())
    }
}

/// Four independently observed Boolean status points in S1, S2, W3, W4 order.
pub struct RelayFeedback {
    references: [String; 4],
}

impl RelayFeedback {
    /// Configure unique MMS status leaf names for the four logical contact indications.
    pub fn new(s1: &str, s2: &str, w3: &str, w4: &str) -> Result<Self, BridgeError> {
        let names = [s1, s2, w3, w4];
        for (i, name) in names.iter().enumerate() {
            validate_reference(name, Fc::ST, 2)?;
            if names[..i].contains(name) {
                return Err(BridgeError::InvalidMapping);
            }
        }
        Ok(Self {
            references: names.map(String::from),
        })
    }

    /// Answer direct named reads using fresh, independently sampled indications.
    /// Supply None when feedback is unavailable or stale. Conflicting observed
    /// generation bits are reported individually to preserve the fault indication.
    pub fn answer_read(
        &self,
        request: &ConfirmedRequest<'_>,
        observed: Option<RelayContacts>,
    ) -> Answer {
        let ConfirmedRequest::Read {
            specification_with_result: false,
            access: VariableAccess::ListOfVariable(vars),
        } = request
        else {
            return Answer::DENIED;
        };
        let bits = observed.map(|v| [v.s1, v.s2, v.w3, v.w4]);
        Answer::Read(
            vars.iter()
                .map(|var| {
                    let VariableSpecification::Name(ObjectName::DomainSpecific { domain, item }) =
                        var
                    else {
                        return Err(DATA_ACCESS_NON_EXISTENT);
                    };
                    let index = self
                        .references
                        .iter()
                        .position(|name| name.split_once('/') == Some((*domain, *item)))
                        .ok_or(DATA_ACCESS_NON_EXISTENT)?;
                    bits.map(|b| Value::Boolean(b[index]))
                        .ok_or(TEMPORARILY_UNAVAILABLE)
                })
                .collect(),
        )
    }
}
