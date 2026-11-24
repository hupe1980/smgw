use std::time::Duration;

use eebus::{
    model::{
        AbsoluteOrRelativeTime, AddressDevice, CmdData, DeviceType, EntityType, Function,
        UnitOfMeasurement,
    },
    ship::Ski,
    spine::{Engine, LocalDevice, LocalEntity, node_management},
    usecases::{
        limitation, mgcp,
        monitoring::{self, Measurand, MonitoredUnit, MonitoringApplianceActor, Naming},
        mpc,
    },
};
use iec61850_kit::{
    Limits,
    proto::{
        data::DataView,
        mms::{
            AccessResult, ConfirmedRequest, ConfirmedResponse, Mms, ObjectName, VariableAccess,
            VariableSpecification,
        },
    },
};
use smgw_cls::bridge::measurement::{InvalidMeasurementMapping, MeasurementSource, PowerReadback};

// Synthetic test identity of the transport delivering the unit's datagrams.
const SKI: Ski = Ski::from_bytes([0x81; 20]);
const UTC: Duration = Duration::from_secs(1_700_000_000);
const NOW: Duration = Duration::from_secs(100);

pub fn read_monitoring_watts(device: &AddressDevice, source: MeasurementSource, watts: f64) -> f32 {
    monitor(device, source, watts, Some(UnitOfMeasurement::W)).unwrap()
}

fn monitor(
    device: &AddressDevice,
    source: MeasurementSource,
    watts: f64,
    wire_unit: Option<UnitOfMeasurement>,
) -> Result<f32, InvalidMeasurementMapping> {
    let (descriptor, monitor, entity, naming, scenario) = match source {
        MeasurementSource::Mpc => (
            &mpc::MONITORED_UNIT,
            &mpc::MONITORING_APPLIANCE,
            EntityType::HeatPumpAppliance,
            Naming::Appliance,
            1,
        ),
        MeasurementSource::Mgcp => (
            &mgcp::GRID_CONNECTION_POINT,
            &mgcp::MONITORING_APPLIANCE,
            EntityType::GridConnectionPointOfPremises,
            Naming::GridConnectionPoint,
            2,
        ),
    };
    let mut unit = MonitoredUnit::new(1)
        .naming(naming)
        .with(Measurand::total_power());
    let mut remote = LocalDevice::from_address(device.clone(), DeviceType::SubMeter);
    remote
        .add_entity(
            LocalEntity::new([2], entity)
                .with_feature(unit.electrical_connection_feature(1))
                .with_feature(unit.measurement_feature(2)),
        )
        .unwrap();
    let electrical = remote.address_of(&[2], 1);
    let measurement = remote.address_of(&[2], 2);
    let mut remote = Engine::new(remote);
    remote.add_use_case_scenarios([2], 1, descriptor, &[scenario]);
    unit.publish(&mut remote, &electrical, &measurement);
    let mut descriptions = unit.measurement_descriptions();
    let CmdData::MeasurementDescriptionListData(list) = &mut descriptions else {
        unreachable!()
    };
    list.measurement_description_data.as_mut().unwrap()[0].unit = wire_unit;
    remote
        .device_mut()
        .resolve_mut(&measurement)
        .unwrap()
        .set_data(descriptions)
        .unwrap();

    let mut local =
        LocalDevice::new("i:12345", "Monitor", DeviceType::EnergyManagementSystem).unwrap();
    local
        .add_entity(
            LocalEntity::new([1], EntityType::CEM).with_feature(limitation::client_feature(1)),
        )
        .unwrap();
    let mut actor = MonitoringApplianceActor::new(local.address_of(&[1], 1));
    let mut local = Engine::new(local);
    local.add_use_case_scenarios([1], 1, monitor, &[scenario]);
    for f in [
        Function::NodeManagementDetailedDiscoveryData,
        Function::NodeManagementUseCaseData,
    ] {
        local.read(
            &node_management(device),
            &node_management(local.device().address()),
            f,
            NOW,
        );
    }
    exchange(&mut local, &mut remote);
    let peer = monitoring::locate(
        local.peer(device).unwrap(),
        descriptor.name,
        descriptor.actor,
    )
    .unwrap();
    let mut mapping = PowerReadback::new(
        "CLSLD0",
        "MMXU1",
        source,
        SKI,
        peer.id(),
        Duration::from_secs(5),
    )
    .unwrap();
    actor.attach(&mut local, peer, NOW);
    exchange(&mut local, &mut remote);
    while let Some(spine) = local.poll_event() {
        actor.handle_event(&spine);
    }
    mapping.commission(&SKI, &local, &actor)?;
    unit.set_at(
        &Measurand::total_power(),
        watts,
        AbsoluteOrRelativeTime::from("2023-11-14T22:13:19Z"),
    );
    unit.notify(&mut remote, &measurement, NOW);
    exchange(&mut local, &mut remote);
    while let Some(spine) = local.poll_event() {
        if let Some(event) = actor.handle_event(&spine) {
            mapping.observe(source, &SKI, &event, UTC, NOW);
        }
    }

    let request = Mms::ConfirmedRequest {
        invoke_id: 7,
        service: ConfirmedRequest::Read {
            specification_with_result: false,
            access: VariableAccess::ListOfVariable(vec![VariableSpecification::Name(
                ObjectName::DomainSpecific {
                    domain: "CLSLD0",
                    item: "MMXU1$MX$TotW$mag$f",
                },
            )]),
        },
    }
    .to_vec()
    .unwrap();
    let Mms::ConfirmedRequest { invoke_id, service } =
        Mms::parse(&request, &Limits::DEFAULT).unwrap()
    else {
        panic!("expected MMS Read")
    };
    let response = mapping
        .answer_read(&service, NOW)
        .encode(invoke_id)
        .unwrap();
    let Mms::ConfirmedResponse {
        invoke_id: 7,
        service: ConfirmedResponse::Read { results, .. },
    } = Mms::parse(&response, &Limits::DEFAULT).unwrap()
    else {
        panic!("expected matching MMS Read response")
    };
    let [AccessResult::Success(data)] = results.as_slice() else {
        panic!("measurement unavailable: {results:?}")
    };
    let DataView::Float32(watts) = DataView::from_tlv(*data).unwrap() else {
        panic!("expected FLOAT32 watts")
    };
    Ok(watts)
}

fn exchange(local: &mut Engine, remote: &mut Engine) {
    for _ in 0..32 {
        let mut moved = false;
        while let Some(data) = local.poll_transmit() {
            remote.handle_datagram(&data, NOW);
            moved = true;
        }
        while let Some(data) = remote.poll_transmit() {
            local.handle_datagram(&data, NOW);
            moved = true;
        }
        while remote.poll_event().is_some() {}
        if !moved {
            return;
        }
    }
    panic!("monitoring exchange failed to settle");
}

#[test]
fn real_mpc_appliance_read_returns_signed_consumption() {
    let device =
        LocalDevice::new("i:12345", "Appliance", DeviceType::HeatGenerationSystem).unwrap();
    assert_eq!(
        read_monitoring_watts(device.address(), MeasurementSource::Mpc, 2190.5),
        2190.5
    );
}

#[test]
fn commissioning_rejects_missing_or_wrong_wire_unit() {
    let device =
        LocalDevice::new("i:12345", "Appliance", DeviceType::HeatGenerationSystem).unwrap();
    for unit in [
        None,
        Some(UnitOfMeasurement::Wh),
        Some(UnitOfMeasurement::Unknown),
    ] {
        assert_eq!(
            monitor(device.address(), MeasurementSource::Mpc, 2.0, unit),
            Err(InvalidMeasurementMapping)
        );
    }
}
