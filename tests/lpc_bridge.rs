#![cfg(feature = "fnn-bridge")]

#[path = "support/operate.rs"]
mod operate;
#[path = "support/telemetry.rs"]
mod telemetry;

use core::{num::NonZeroU32, time::Duration};
use eebus::{
    model::{CmdData, DeviceType, EntityType, Function},
    spine::{Engine, LocalDevice, LocalEntity, SpineEvent, node_management},
    usecases::{
        limitation::{
            self, ControllableSystem, ControllableSystemActor, CsConfig, CsFeatures, Direction,
            EnergyGuardActor,
        },
        lpc, lpp,
    },
};
use iec61850_kit::{
    UtcTime,
    common::ControlModel,
    proto::{data::Value, mms::control::ControlRequest},
    server::{ControlEvent, Stage},
};
use smgw_cls::bridge::{
    BridgeError, LpcBridge, LppBridge, ProductionWrite, measurement::MeasurementSource,
};

const POWER: &str = "CLSLD0/DWMX1$CO$WMaxSetPct";
const RELEASE: &str = "CLSLD0/GGIO1$CO$Release";
const SECOND: Duration = Duration::from_secs(1);

fn event(object: &str, value: Value, number: u8) -> ControlEvent {
    ControlEvent {
        stage: Stage::Operate,
        request: ControlRequest::new(
            value,
            number,
            UtcTime::from_unix_nanos(0, Default::default()),
        ),
        object: object.into(),
        model: ControlModel::DirectNormal,
    }
}

/// Drive both SPINE actors using their public datagram and timer interfaces.
struct Peers {
    guard: EnergyGuardActor,
    controller: Engine,
    system: ControllableSystemActor,
    appliance: Engine,
}

impl Peers {
    fn new(direction: Direction) -> Self {
        let (guard_role, system_role, name, device_type, entity_type) = match direction {
            Direction::Consumption => (
                &lpc::ENERGY_GUARD,
                &lpc::CONTROLLABLE_SYSTEM,
                "HeatPump",
                DeviceType::HeatGenerationSystem,
                EntityType::HeatPumpAppliance,
            ),
            Direction::Production => (
                &lpp::ENERGY_GUARD,
                &lpp::CONTROLLABLE_SYSTEM,
                "PVInverter",
                DeviceType::Inverter,
                EntityType::Inverter,
            ),
        };
        let zero = Duration::ZERO;
        let mut controller =
            LocalDevice::new("i:12345", "Controller", DeviceType::ElectricitySupplySystem).unwrap();
        controller
            .add_entity(
                LocalEntity::new([1], EntityType::GridGuard)
                    .with_feature(limitation::client_feature(1))
                    .with_feature(limitation::device_diagnosis_feature(2)),
            )
            .unwrap();
        let guard = EnergyGuardActor::new(
            direction,
            controller.address_of(&[1], 1),
            controller.address_of(&[1], 2),
            zero,
        );
        let mut controller = Engine::new(controller);
        controller.add_use_case([1], 1, guard_role);

        let mut appliance = LocalDevice::new("i:67890", name, device_type).unwrap();
        appliance
            .add_entity(
                LocalEntity::new([1], entity_type)
                    .with_feature(limitation::load_control_feature(1))
                    .with_feature(limitation::device_configuration_feature(2))
                    .with_feature(limitation::device_diagnosis_feature(3))
                    .with_feature(limitation::device_diagnosis_client_feature(4)),
            )
            .unwrap();
        let features = CsFeatures {
            load_control: appliance.address_of(&[1], 1),
            device_configuration: appliance.address_of(&[1], 2),
            device_diagnosis: appliance.address_of(&[1], 3),
            device_diagnosis_client: appliance.address_of(&[1], 4),
        };
        let mut appliance = Engine::new(appliance);
        appliance.add_use_case([1], 1, system_role);
        let system = ControllableSystemActor::builder(
            ControllableSystem::new(CsConfig::new(4200.0, Duration::from_secs(7200)), zero),
            direction,
            features,
        )
        .install(&mut appliance, zero);
        Self {
            controller,
            guard,
            appliance,
            system,
        }
    }

    fn commission(&mut self) {
        let guard = node_management(self.controller.device().address());
        let appliance = node_management(self.appliance.device().address());
        for f in [
            Function::NodeManagementDetailedDiscoveryData,
            Function::NodeManagementUseCaseData,
        ] {
            self.controller
                .read(&appliance, &guard, f.clone(), Duration::ZERO);
            self.appliance.read(&guard, &appliance, f, Duration::ZERO);
        }
        self.settle(Duration::ZERO);
        assert!(self.guard.is_ready(self.appliance.device().address()));
    }

    fn read_load_control(&mut self, function: Function) {
        let client = self.controller.device().address_of(&[1], 1);
        let target = self.appliance.device().address_of(&[1], 1);
        self.controller
            .read(&target, &client, function, Duration::ZERO);
        self.settle(Duration::ZERO);
    }

    fn settle(&mut self, now: Duration) {
        for _ in 0..128 {
            let mut progress = false;
            while let Some(data) = self.controller.poll_transmit() {
                self.appliance.handle_datagram(&data, now);
                progress = true;
            }
            while let Some(data) = self.appliance.poll_transmit() {
                self.controller.handle_datagram(&data, now);
                progress = true;
            }
            while let Some(event) = self.appliance.poll_event() {
                self.system.handle_event(&mut self.appliance, &event, now);
                progress = true;
            }
            while let Some(event) = self.controller.poll_event() {
                self.guard.handle_event(&mut self.controller, &event, now);
                progress = true;
            }
            if !progress {
                return;
            }
        }
        panic!("the simulated SPINE exchange failed to settle");
    }

    fn tick(&mut self, now: Duration) {
        self.guard.handle_timeout(&mut self.controller, now);
        self.system.handle_timeout(&mut self.appliance, now);
        self.controller.handle_timeout(now);
        self.appliance.handle_timeout(now);
        self.settle(now);
    }
}

#[test]
fn percentage_and_release_reach_the_commissioned_lpc_peer() {
    let mut peers = Peers::new(Direction::Consumption);
    let device = peers.appliance.device().address().clone();
    let bridge = LpcBridge::new(
        POWER,
        RELEASE,
        device.clone(),
        NonZeroU32::new(5000).unwrap(),
    )
    .unwrap();
    let apply = event(POWER, Value::Float32(40.0), 12);
    let expiry = Duration::from_secs(900);
    assert_eq!(
        bridge.schedule(&mut peers.guard, &apply, expiry, SECOND),
        Err(BridgeError::PeerUnavailable)
    );
    assert_eq!(peers.guard.deferred_requirements().count(), 0);
    peers.commission();
    let wrong_device = LpcBridge::new(
        POWER,
        RELEASE,
        "uncommissioned-device".into(),
        NonZeroU32::new(5000).unwrap(),
    )
    .unwrap();
    assert_eq!(
        wrong_device.schedule(&mut peers.guard, &apply, expiry, SECOND),
        Err(BridgeError::PeerUnavailable)
    );
    assert_eq!(peers.guard.deferred_requirements().count(), 0);

    let receipt = bridge
        .schedule(&mut peers.guard, &apply, expiry, SECOND)
        .unwrap();
    assert_eq!((receipt.control_number, receipt.watts), (12, Some(2000.0)));
    peers.tick(SECOND);
    assert_eq!(peers.guard.applied_limit(&device).unwrap().watts(), 2000.0);

    let release = event(RELEASE, Value::Boolean(true), 13);
    assert_eq!(
        bridge
            .schedule(&mut peers.guard, &release, expiry, SECOND * 2)
            .unwrap()
            .watts,
        None
    );
    // The guard enforces its minimum interval between changes. Both actors
    // continue heartbeats during the interval, on a virtual clock.
    for second in 2..=limitation::MIN_WRITE_INTERVAL.as_secs() + 2 {
        peers.tick(Duration::from_secs(second));
    }
    assert!(!peers.guard.applied_limit(&device).unwrap().is_active);
}

#[test]
fn rejected_controls_leave_no_deferred_requirements() {
    let mut peers = Peers::new(Direction::Consumption);
    let bridge = LpcBridge::new(
        POWER,
        RELEASE,
        peers.appliance.device().address().clone(),
        NonZeroU32::new(5000).unwrap(),
    )
    .unwrap();
    let mut apply = event(POWER, Value::Float32(30.0), 4);
    assert_eq!(
        bridge.schedule(&mut peers.guard, &apply, SECOND, SECOND),
        Err(BridgeError::Expired)
    );
    let other = event("WRONG/DWMX1$CO$WMaxSetPct", Value::Float32(30.0), 4);
    assert_eq!(
        bridge.schedule(&mut peers.guard, &other, SECOND, Duration::ZERO),
        Err(BridgeError::UnmappedControl)
    );
    apply.request.test = true;
    assert_eq!(
        bridge.schedule(&mut peers.guard, &apply, SECOND, Duration::ZERO),
        Err(BridgeError::UnsupportedOperation)
    );
    for value in [f32::NAN, f32::INFINITY, -1.0, 100.1] {
        let bad = event(POWER, Value::Float32(value), 4);
        assert_eq!(
            bridge.schedule(&mut peers.guard, &bad, SECOND, Duration::ZERO),
            Err(BridgeError::InvalidValue)
        );
    }
    for value in [
        Value::Structure(vec![]),
        Value::Structure(vec![Value::Integer(40)]),
        Value::Structure(vec![Value::Float32(40.0), Value::Float32(0.0)]),
        Value::Structure(vec![Value::Structure(vec![Value::Float32(40.0)])]),
    ] {
        let bad = event(POWER, value, 4);
        assert_eq!(
            bridge.schedule(&mut peers.guard, &bad, SECOND, Duration::ZERO),
            Err(BridgeError::InvalidValue)
        );
    }
    let bad_release = event(RELEASE, Value::Boolean(false), 5);
    assert_eq!(
        bridge.schedule(&mut peers.guard, &bad_release, SECOND, Duration::ZERO),
        Err(BridgeError::InvalidValue)
    );
    assert_eq!(peers.guard.deferred_requirements().count(), 0);
}

/// Deliver the real SPINE write to a peer application that checks its signed
/// wire value before accepting it, then match the remote result to the request.
fn acknowledge_production_write(
    peers: &mut Peers,
    write: ProductionWrite,
    expected: Option<f64>,
    now: Duration,
) {
    let datagram = peers.controller.poll_transmit().expect("queued LPP write");
    peers.appliance.handle_datagram(&datagram, now);
    let Some(SpineEvent::WriteRequested(request)) = peers.appliance.poll_event() else {
        panic!("LPP application should receive a deferred SPINE write");
    };
    assert_eq!(request.request, write.message_counter);
    let CmdData::LoadControlLimitListData(payload) = &request.data else {
        panic!("expected a typed load control limit");
    };
    let entries = payload.load_control_limit_data.as_deref().unwrap();
    let [entry] = entries else {
        panic!("expected exactly one limit");
    };
    assert_eq!(entry.limit_id, Some(limitation::LIMIT_ID));
    assert_eq!(entry.is_limit_active, Some(expected.is_some()));
    assert_eq!(
        entry.value.and_then(|n| n.to_f64()),
        Some(expected.unwrap_or(0.0))
    );
    peers.appliance.accept_write(request.token, now).unwrap();
    while let Some(data) = peers.appliance.poll_transmit() {
        peers.controller.handle_datagram(&data, now);
    }
    assert!(std::iter::from_fn(|| peers.controller.poll_event()).any(|event| matches!(
        event,
        SpineEvent::ResultReceived { request, error } if request == write.message_counter && error.is_success()
    )));
}

#[test]
fn production_operate_reaches_inverter_and_independent_measurement_is_read_over_mms() {
    let mut peers = Peers::new(Direction::Production);
    let device = peers.appliance.device().address().clone();
    let client = peers.controller.device().address_of(&[1], 1);
    let rating = NonZeroU32::new(10_000).unwrap();
    let bridge = LppBridge::new(POWER, RELEASE, device.clone(), rating).unwrap();
    let apply = operate::production_operate(40.0, 21);
    let expiry = Duration::from_secs(900);
    assert_eq!(
        bridge.write(&mut peers.controller, &client, &apply, expiry, SECOND),
        Err(BridgeError::PeerUnavailable)
    );
    assert!(peers.controller.poll_transmit().is_none());
    peers.commission();
    peers.read_load_control(Function::LoadControlLimitListData);
    let consumption = LpcBridge::new(POWER, RELEASE, device.clone(), rating).unwrap();
    assert_eq!(
        consumption.schedule(&mut peers.guard, &apply, expiry, SECOND),
        Err(BridgeError::PeerUnavailable)
    );
    assert_eq!(peers.guard.deferred_requirements().count(), 0);
    let wrong_client = peers.controller.device().address_of(&[1], 2);
    assert_eq!(
        bridge.write(&mut peers.controller, &wrong_client, &apply, expiry, SECOND),
        Err(BridgeError::PeerUnavailable)
    );
    assert!(peers.controller.poll_transmit().is_none());

    let receipt = bridge
        .write(&mut peers.controller, &client, &apply, expiry, SECOND)
        .unwrap();
    assert_eq!((receipt.control_number, receipt.watts), (21, Some(-4000.0)));
    acknowledge_production_write(&mut peers, receipt, Some(-4000.0), SECOND);

    // The metering peer reports its own signed sample independently of the cap.
    assert_eq!(
        telemetry::read_monitoring_watts(&device, MeasurementSource::Mgcp, -2700.0),
        -2700.0
    );

    let zero = operate::production_operate(0.0, 22);
    let release = event(RELEASE, Value::Boolean(true), 23);
    let mut now = SECOND;
    for (command, expected) in [(zero, Some(0.0)), (release, None)] {
        for _ in 0..limitation::MIN_WRITE_INTERVAL.as_secs() {
            now += SECOND;
            peers.tick(now);
        }
        let receipt = bridge
            .write(&mut peers.controller, &client, &command, expiry, now)
            .unwrap();
        assert_eq!(
            (receipt.control_number, receipt.watts),
            (command.request.ctl_num, expected)
        );
        acknowledge_production_write(&mut peers, receipt, expected, now);
    }
}

#[test]
fn production_mapping_rejects_a_commissioned_consumption_peer() {
    let mut peers = Peers::new(Direction::Consumption);
    peers.commission();
    let device = peers.appliance.device().address().clone();
    let client = peers.controller.device().address_of(&[1], 1);
    let bridge = LppBridge::new(POWER, RELEASE, device, NonZeroU32::new(10_000).unwrap()).unwrap();
    assert_eq!(
        bridge.write(
            &mut peers.controller,
            &client,
            &event(POWER, Value::Float32(40.0), 24),
            SECOND,
            Duration::ZERO
        ),
        Err(BridgeError::PeerUnavailable)
    );
    assert!(peers.controller.poll_transmit().is_none());
}

#[test]
fn production_mapping_requires_one_changeable_limit_measured_in_watts() {
    let mut peers = Peers::new(Direction::Production);
    peers.commission();
    let device = peers.appliance.device().address().clone();
    let client = peers.controller.device().address_of(&[1], 1);
    let target = peers.appliance.device().address_of(&[1], 1);
    let bridge = LppBridge::new(POWER, RELEASE, device, NonZeroU32::new(10_000).unwrap()).unwrap();
    let command = event(POWER, Value::Float32(40.0), 3);
    let reject = |peers: &mut Peers| {
        assert_eq!(
            bridge.write(
                &mut peers.controller,
                &client,
                &command,
                SECOND,
                Duration::ZERO
            ),
            Err(BridgeError::PeerUnavailable)
        );
        assert!(peers.controller.poll_transmit().is_none());
    };

    let CmdData::LoadControlLimitListData(mut current) =
        limitation::limit_data(peers.system.system())
    else {
        unreachable!()
    };
    for changeable in [None, Some(false)] {
        current.load_control_limit_data.as_mut().unwrap()[0].is_limit_changeable = changeable;
        peers
            .appliance
            .device_mut()
            .resolve_mut(&target)
            .unwrap()
            .set_data(CmdData::LoadControlLimitListData(current.clone()))
            .unwrap();
        peers.read_load_control(Function::LoadControlLimitListData);
        reject(&mut peers);
    }
    peers.system.publish(&mut peers.appliance, Duration::ZERO);
    peers.read_load_control(Function::LoadControlLimitListData);

    let CmdData::LoadControlLimitDescriptionListData(description) =
        limitation::limit_description(Direction::Production)
    else {
        unreachable!()
    };
    for (unit, duplicate) in [
        (None, false),
        (Some(eebus::model::UnitOfMeasurement::Wh), false),
        (Some(eebus::model::UnitOfMeasurement::W), true),
    ] {
        let mut list = description.clone();
        let entries = list.load_control_limit_description_data.as_mut().unwrap();
        entries[0].unit = unit;
        if duplicate {
            let mut second = entries[0].clone();
            second.limit_id = Some(eebus::model::LoadControlLimitId(23));
            entries.push(second);
        }
        peers
            .appliance
            .device_mut()
            .resolve_mut(&target)
            .unwrap()
            .set_data(CmdData::LoadControlLimitDescriptionListData(list))
            .unwrap();
        peers.read_load_control(Function::LoadControlLimitDescriptionListData);
        reject(&mut peers);
    }
}

#[test]
fn device_mapping_refuses_ambiguous_production_entities() {
    let mut peers = Peers::new(Direction::Production);
    peers
        .appliance
        .add_use_case([2], 1, &lpp::CONTROLLABLE_SYSTEM);
    peers.commission();
    let device = peers.appliance.device().address().clone();
    let client = peers.controller.device().address_of(&[1], 1);
    let bridge = LppBridge::new(POWER, RELEASE, device, NonZeroU32::new(10_000).unwrap()).unwrap();
    assert_eq!(
        bridge.write(
            &mut peers.controller,
            &client,
            &operate::production_operate(40.0, 25),
            SECOND,
            Duration::ZERO
        ),
        Err(BridgeError::PeerUnavailable)
    );
    assert!(peers.controller.poll_transmit().is_none());
}
