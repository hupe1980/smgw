//! Map commissioned IEC 61850 controls to EEBUS Limitation of Power Consumption (LPC).
//!
//! Configure the two control references, the reference power in watts, and the
//! target device. The limit control accepts a floating-point percentage (0–100).
//! A separate boolean control releases the limit. The caller authenticates and
//! authorizes operate events, drives SPINE delivery, and observes acknowledgement.

use core::{fmt, num::NonZeroU32, time::Duration};
use eebus::{
    model::AddressDevice,
    usecases::limitation::{Direction, EnergyGuardActor, LimitWrite},
};
use iec61850_kit::{
    Fc, ObjectReference,
    common::ControlModel,
    proto::data::Value,
    server::{ControlEvent, Stage},
};

/// One provisioned mapping to one LPC device.
pub struct LpcBridge {
    percent_control: String,
    release_control: String,
    device: AddressDevice,
    rated_watts: NonZeroU32,
}

/// Why the bridge refused a command before changing the guard's local requirement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeError {
    InvalidMapping,
    Expired,
    UnmappedControl,
    UnsupportedOperation,
    InvalidValue,
    PeerUnavailable,
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidMapping => "expected two distinct CO references and an LPC device",
            Self::Expired => "the command admission deadline has passed",
            Self::UnmappedControl => "no LPC mapping for this control",
            Self::UnsupportedOperation => "expected an immediate, non-test operate",
            Self::InvalidValue => "invalid control value for this LPC mapping",
            Self::PeerUnavailable => "the LPC guard has no ready binding to the mapped device",
        })
    }
}

impl std::error::Error for BridgeError {}

/// Local scheduling receipt. The enclosing MMS association supplies correlation
/// context for the control number. Confirm delivery from EEBUS guard events.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScheduledLimit {
    /// The originating MMS control number.
    pub control_number: u8,
    /// Required watts. `None` explicitly releases a limit.
    pub watts: Option<f64>,
}

impl LpcBridge {
    /// Bind two fully qualified MMS control references to one provisioned device.
    pub fn new(
        percent_control: &str,
        release_control: &str,
        device: AddressDevice,
        rated_watts: NonZeroU32,
    ) -> Result<Self, BridgeError> {
        for name in [percent_control, release_control] {
            let reference =
                ObjectReference::parse(name).map_err(|_| BridgeError::InvalidMapping)?;
            if reference.fc != Some(Fc::CO) || reference.path().next().is_none() {
                return Err(BridgeError::InvalidMapping);
            }
        }
        if percent_control == release_control || device.as_str().trim().is_empty() {
            return Err(BridgeError::InvalidMapping);
        }
        Ok(Self {
            percent_control: percent_control.into(),
            release_control: release_control.into(),
            device,
            rated_watts,
        })
    }

    /// Schedule an authorized event against a monotonic admission deadline. The
    /// deadline bounds acceptance here. Delivery, acknowledgement and physical
    /// enforcement are later outcomes. A zero-percent limit remains active.
    pub fn schedule(
        &self,
        guard: &mut EnergyGuardActor,
        event: &ControlEvent,
        expires_at: Duration,
        now: Duration,
    ) -> Result<ScheduledLimit, BridgeError> {
        if now >= expires_at {
            return Err(BridgeError::Expired);
        }
        if event.stage != Stage::Operate
            || event.model == ControlModel::StatusOnly
            || event.request.test
            || event.request.oper_tm.is_some()
        {
            return Err(BridgeError::UnsupportedOperation);
        }
        let limit = if event.object == self.percent_control {
            let percent = match event.request.ctl_val {
                Value::Float32(n) => f64::from(n),
                Value::Float64(n) => n,
                _ => return Err(BridgeError::InvalidValue),
            };
            if !(0.0..=100.0).contains(&percent) {
                return Err(BridgeError::InvalidValue);
            }
            Some(LimitWrite::active(
                f64::from(self.rated_watts.get()) * percent / 100.0,
            ))
        } else if event.object == self.release_control {
            if event.request.ctl_val != Value::Boolean(true) {
                return Err(BridgeError::InvalidValue);
            }
            None
        } else {
            return Err(BridgeError::UnmappedControl);
        };
        if guard.direction() != Direction::Consumption || !guard.is_ready(&self.device) {
            return Err(BridgeError::PeerUnavailable);
        }
        guard
            .require(&self.device, limit, now)
            .map_err(|_| BridgeError::InvalidValue)?;
        Ok(ScheduledLimit {
            control_number: event.request.ctl_num,
            watts: limit.map(|limit| limit.watts()),
        })
    }
}
