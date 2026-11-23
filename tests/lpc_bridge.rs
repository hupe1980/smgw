#![cfg(feature = "fnn-bridge")]

use core::{num::NonZeroU32, time::Duration};
use eebus::{
    model::{DeviceType, EntityType, Function},
    spine::{Engine, LocalDevice, LocalEntity, node_management},
    usecases::{
        limitation::{
            self, ControllableSystem, ControllableSystemActor, CsConfig, CsFeatures, Direction,
            EnergyGuardActor,
        },
        lpc,
    },
};
use iec61850_kit::{
    UtcTime,
    common::ControlModel,
    proto::{data::Value, mms::control::ControlRequest},
    server::{ControlEvent, Stage},
};
use smgw_cls::bridge::{BridgeError, LpcBridge};

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
    pump: ControllableSystemActor,
    appliance: Engine,
}

impl Peers {
    fn new() -> Self {
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
            Direction::Consumption,
            controller.address_of(&[1], 1),
            controller.address_of(&[1], 2),
            zero,
        );
        let mut controller = Engine::new(controller);
        controller.add_use_case([1], 1, &lpc::ENERGY_GUARD);

        let mut appliance =
            LocalDevice::new("i:67890", "HeatPump", DeviceType::HeatGenerationSystem).unwrap();
        appliance
            .add_entity(
                LocalEntity::new([1], EntityType::HeatPumpAppliance)
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
        appliance.add_use_case([1], 1, &lpc::CONTROLLABLE_SYSTEM);
        let pump = ControllableSystemActor::builder(
            ControllableSystem::new(CsConfig::new(4200.0, Duration::from_secs(7200)), zero),
            Direction::Consumption,
            features,
        )
        .install(&mut appliance, zero);
        Self {
            controller,
            guard,
            appliance,
            pump,
        }
    }

    fn commission(&mut self) {
        let guard = node_management(self.controller.device().address());
        let pump = node_management(self.appliance.device().address());
        for f in [
            Function::NodeManagementDetailedDiscoveryData,
            Function::NodeManagementUseCaseData,
        ] {
            self.controller
                .read(&pump, &guard, f.clone(), Duration::ZERO);
            self.appliance.read(&guard, &pump, f, Duration::ZERO);
        }
        self.settle(Duration::ZERO);
        assert!(self.guard.is_ready(self.appliance.device().address()));
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
                self.pump.handle_event(&mut self.appliance, &event, now);
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
        self.pump.handle_timeout(&mut self.appliance, now);
        self.controller.handle_timeout(now);
        self.appliance.handle_timeout(now);
        self.settle(now);
    }
}

#[test]
fn percentage_and_release_reach_the_commissioned_lpc_peer() {
    let mut peers = Peers::new();
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
    let mut peers = Peers::new();
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
    let bad_release = event(RELEASE, Value::Boolean(false), 5);
    assert_eq!(
        bridge.schedule(&mut peers.guard, &bad_release, SECOND, Duration::ZERO),
        Err(BridgeError::InvalidValue)
    );
    assert_eq!(peers.guard.deferred_requirements().count(), 0);
}
