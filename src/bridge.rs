//! Map commissioned IEC 61850 controls to EEBUS consumption (LPC) and production (LPP) limits.
//!
//! Configure the two control references, the reference power in watts, and the
//! target device. The limit control accepts a floating-point percentage (0–100).
//! A separate boolean control releases the limit. The caller authenticates and
//! authorizes operate events, drives SPINE delivery, and observes acknowledgement.
//! Use [`measurement::PowerReadback`] to expose independently measured power over MMS.

pub mod measurement;

use core::{fmt, num::NonZeroU32, time::Duration};
use eebus::{
    model::{
        AddressDevice, CmdData, FeatureAddress, Filter, FilterElements, FilterSelectors, Function,
        LoadControlCategory, LoadControlLimitData, LoadControlLimitDataElements,
        LoadControlLimitId, LoadControlLimitListData, LoadControlLimitListDataSelectors,
        LoadControlLimitType, MsgCounter, ScaledNumber, ScopeType, TimePeriodElements,
        UnitOfMeasurement,
    },
    spine::Engine,
    usecases::limitation::{self, Direction, EnergyGuardActor, LimitWrite},
};
use iec61850_kit::{
    Fc, ObjectReference,
    common::ControlModel,
    proto::data::Value,
    server::{ControlEvent, Stage},
};

struct Mapping {
    percent_control: String,
    release_control: String,
    device: AddressDevice,
    rated_watts: NonZeroU32,
}

/// Commissioned consumption mapping, scheduled through an LPC Energy Guard.
pub struct LpcBridge(Mapping);

/// Commissioned production mapping, sending signed LPP writes on a SPINE connection.
pub struct LppBridge(Mapping);

/// An enqueued LPP request. Match its counter against the session's
/// [`eebus::spine::SpineEvent::ResultReceived`] to establish the peer outcome.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProductionWrite {
    /// The originating MMS control number.
    pub control_number: u8,
    /// Signed feed-in watts sent on the wire. Zero is active, and `None` releases.
    pub watts: Option<f64>,
    /// SPINE request identifier on this engine.
    pub message_counter: MsgCounter,
}

/// Why the bridge refused a command before dispatch.
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
            Self::InvalidMapping => "expected two distinct CO references and a target device",
            Self::Expired => "the command admission deadline has passed",
            Self::UnmappedControl => "no configured mapping for this control",
            Self::UnsupportedOperation => "expected an immediate, non-test operate",
            Self::InvalidValue => "invalid control value for this power limit mapping",
            Self::PeerUnavailable => {
                "the guard direction or ready peer does not match this mapping"
            }
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

impl Mapping {
    fn new(
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

    fn command(
        &self,
        event: &ControlEvent,
        expires_at: Duration,
        now: Duration,
    ) -> Result<Option<f64>, BridgeError> {
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
        if event.object == self.percent_control {
            // An APC Oper carries ctlVal as an AnalogueValue structure.
            let value = match &event.request.ctl_val {
                Value::Structure(fields) => match fields.as_slice() {
                    [value] => value,
                    _ => return Err(BridgeError::InvalidValue),
                },
                scalar => scalar,
            };
            let percent = match value {
                Value::Float32(n) => f64::from(*n),
                Value::Float64(n) => *n,
                _ => return Err(BridgeError::InvalidValue),
            };
            if !(0.0..=100.0).contains(&percent) {
                return Err(BridgeError::InvalidValue);
            }
            Ok(Some(f64::from(self.rated_watts.get()) * percent / 100.0))
        } else if event.object == self.release_control {
            if event.request.ctl_val != Value::Boolean(true) {
                return Err(BridgeError::InvalidValue);
            }
            Ok(None)
        } else {
            Err(BridgeError::UnmappedControl)
        }
    }
}

impl LpcBridge {
    /// Bind two fully qualified MMS control references to one LPC device.
    pub fn new(
        percent_control: &str,
        release_control: &str,
        device: AddressDevice,
        rated_watts: NonZeroU32,
    ) -> Result<Self, BridgeError> {
        Mapping::new(percent_control, release_control, device, rated_watts).map(Self)
    }

    /// Schedule an authorized event before its monotonic admission deadline.
    /// Delivery, acknowledgement and physical enforcement are later outcomes.
    pub fn schedule(
        &self,
        guard: &mut EnergyGuardActor,
        event: &ControlEvent,
        expires_at: Duration,
        now: Duration,
    ) -> Result<ScheduledLimit, BridgeError> {
        let limit = self
            .0
            .command(event, expires_at, now)?
            .map(LimitWrite::active);
        if guard.direction() != Direction::Consumption || !guard.is_ready(&self.0.device) {
            return Err(BridgeError::PeerUnavailable);
        }
        guard
            .require(&self.0.device, limit, now)
            .map_err(|_| BridgeError::InvalidValue)?;
        Ok(ScheduledLimit {
            control_number: event.request.ctl_num,
            watts: limit.map(|limit| limit.watts()),
        })
    }
}

impl LppBridge {
    /// Bind two fully qualified MMS control references to an LPP device.
    pub fn new(
        percent_control: &str,
        release_control: &str,
        device: AddressDevice,
        rated_watts: NonZeroU32,
    ) -> Result<Self, BridgeError> {
        Mapping::new(percent_control, release_control, device, rated_watts).map(Self)
    }

    /// Enqueue an authorized, indefinite LPP write before its admission deadline.
    ///
    /// The session must already have discovered the production limit description
    /// and current limit list, with one changeable production limit in watts, and
    /// hold both use-case bindings. The caller owns heartbeat freshness, spacing
    /// between writes, acknowledgement, retries and reconnect policy.
    pub fn write(
        &self,
        engine: &mut Engine,
        client: &FeatureAddress,
        event: &ControlEvent,
        expires_at: Duration,
        now: Duration,
    ) -> Result<ProductionWrite, BridgeError> {
        let magnitude = self.0.command(event, expires_at, now)?;
        let remote = engine
            .peer(&self.0.device)
            .ok_or(BridgeError::PeerUnavailable)?;
        let role = &eebus::usecases::lpp::CONTROLLABLE_SYSTEM;
        // The mapping provisions a device. Multiple production entities on it
        // would make the destination depend on discovery order.
        {
            let mut actors = remote.use_cases_played(role.name, role.actor);
            if !actors
                .next()
                .is_some_and(|actor| actor.available && actor.scenarios.contains(&1))
                || actors.next().is_some()
            {
                return Err(BridgeError::PeerUnavailable);
            }
        }
        let peer = limitation::locate(remote, Direction::Production)
            .ok_or(BridgeError::PeerUnavailable)?;
        if !engine.relations().holds_binding(client, &peer.load_control)
            || !engine
                .relations()
                .holds_binding(client, &peer.device_configuration)
        {
            return Err(BridgeError::PeerUnavailable);
        }
        let id =
            production_limit_id(engine, &peer.load_control).ok_or(BridgeError::PeerUnavailable)?;
        let value = ScaledNumber::from_f64(-magnitude.unwrap_or(0.0), 3)
            .ok_or(BridgeError::InvalidValue)?;
        let entry = LoadControlLimitData {
            limit_id: Some(id),
            is_limit_active: Some(magnitude.is_some()),
            value: Some(value),
            ..Default::default()
        };
        let data = CmdData::LoadControlLimitListData(LoadControlLimitListData {
            load_control_limit_data: Some(vec![entry]),
        });
        let message_counter = engine.write_filtered(
            &peer.load_control,
            client,
            data,
            vec![clear_end_time(id), Filter::partial()],
            now,
        );
        Ok(ProductionWrite {
            control_number: event.request.ctl_num,
            watts: magnitude.and(value.to_f64()),
            message_counter,
        })
    }
}

fn production_limit_id(engine: &Engine, target: &FeatureAddress) -> Option<LoadControlLimitId> {
    let CmdData::LoadControlLimitDescriptionListData(list) =
        engine.remote_data(target, &Function::LoadControlLimitDescriptionListData)?
    else {
        return None;
    };
    let descriptions = list.load_control_limit_description_data.as_ref()?;
    let mut production = descriptions.iter().filter(|entry| {
        entry.limit_type == Some(LoadControlLimitType::SignDependentAbsValueLimit)
            && entry.limit_category == Some(LoadControlCategory::Obligation)
            && entry.limit_direction == Some(Direction::Production.energy_direction())
            && entry.scope_type == Some(ScopeType::ActivePowerLimit)
    });
    let entry = production.next()?;
    let id = entry.limit_id?;
    if production.next().is_some()
        || entry.unit != Some(UnitOfMeasurement::W)
        || descriptions
            .iter()
            .filter(|other| other.limit_id == Some(id))
            .count()
            != 1
    {
        return None;
    }
    let CmdData::LoadControlLimitListData(data) =
        engine.remote_data(target, &Function::LoadControlLimitListData)?
    else {
        return None;
    };
    let mut current = data
        .load_control_limit_data
        .as_ref()?
        .iter()
        .filter(|entry| entry.limit_id == Some(id));
    (current.next()?.is_limit_changeable == Some(true) && current.next().is_none()).then_some(id)
}

// A partial write retains absent fields. Remove a previous expiry from this
// specific limit before updating it to an indefinite limit (LPP IG §3.4.1.4).
fn clear_end_time(id: LoadControlLimitId) -> Filter {
    Filter::delete()
        .select(FilterSelectors::LoadControlLimitListDataSelectors(
            LoadControlLimitListDataSelectors { limit_id: Some(id) },
        ))
        .covering(FilterElements::LoadControlLimitDataElements(
            LoadControlLimitDataElements {
                time_period: Some(TimePeriodElements {
                    end_time: Some(eebus::codec::ElementTag),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ))
}
