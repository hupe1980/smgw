#![cfg(feature = "fnn-bridge")]

#[path = "support/operate.rs"]
mod operate;

use iec61850_kit::{
    Limits,
    common::ControlModel,
    proto::{
        data::Value,
        mms::{ConfirmedRequest, Mms, ObjectName, VariableAccess, VariableSpecification},
    },
    server::{Answer, DATA_ACCESS_NON_EXISTENT, Stage},
};
use smgw_cls::{
    bridge::{
        BridgeError,
        relay::{RelayBridge, RelayFeedback},
    },
    fnn::relay::{ConsumptionMode, GenerationStage, RelayContacts, RelayState},
};
use std::time::Duration;

const GENERATION: &str = "CLSLD0/DWMX1$CO$WMaxSetPct";
const DIM: &str = "CLSLD0/GGIO1$CO$Dim";
const DEADLINE: Duration = Duration::from_secs(1);
const NOW: Duration = Duration::ZERO;

#[test]
fn mms_generation_commands_preserve_dimming_until_its_own_release() {
    let bridge = RelayBridge::new(GENERATION, DIM).unwrap();
    let mut state = RelayState::new(GenerationStage::Percent100, ConsumptionMode::Released);
    let load_control = |dimmed| operate::operate("GGIO1$CO$Dim$Oper", Value::Boolean(dimmed), 1);
    bridge
        .apply(&mut state, &load_control(true), DEADLINE, NOW)
        .unwrap();
    for (pct, bits) in [
        (60., [true, false, false]),
        (30., [false, true, false]),
        (0., [false, false, true]),
        (100., [false, false, false]),
    ] {
        bridge
            .apply(
                &mut state,
                &operate::production_operate(pct, 2),
                DEADLINE,
                NOW,
            )
            .unwrap();
        let c = state.contacts();
        assert_eq!([c.s1, c.s2, c.w3], bits);
        assert!(c.w4);
    }
    bridge
        .apply(&mut state, &load_control(false), DEADLINE, NOW)
        .unwrap();
    assert!(!state.contacts().w4);
}

#[test]
fn refused_operates_preserve_the_entire_commanded_snapshot() {
    let bridge = RelayBridge::new(GENERATION, DIM).unwrap();
    let initial = RelayState::new(GenerationStage::Percent30, ConsumptionMode::Dimmed);
    let mut state = initial;
    let mut event = operate::production_operate(40., 3);
    assert_eq!(
        bridge.apply(&mut state, &event, DEADLINE, NOW),
        Err(BridgeError::InvalidValue)
    );
    event.request.ctl_val = Value::Float64(60.);
    assert_eq!(
        bridge.apply(&mut state, &event, DEADLINE, DEADLINE),
        Err(BridgeError::Expired)
    );
    for stage in [Stage::Select, Stage::Cancel] {
        event.stage = stage;
        assert_eq!(
            bridge.apply(&mut state, &event, DEADLINE, NOW),
            Err(BridgeError::UnsupportedOperation)
        );
    }
    event.stage = Stage::Operate;
    event.request.test = true;
    assert_eq!(
        bridge.apply(&mut state, &event, DEADLINE, NOW),
        Err(BridgeError::UnsupportedOperation)
    );
    event.request.test = false;
    event.request.oper_tm = Some(event.request.t);
    assert_eq!(
        bridge.apply(&mut state, &event, DEADLINE, NOW),
        Err(BridgeError::UnsupportedOperation)
    );
    event.request.oper_tm = None;
    event.model = ControlModel::StatusOnly;
    assert_eq!(
        bridge.apply(&mut state, &event, DEADLINE, NOW),
        Err(BridgeError::UnsupportedOperation)
    );
    event.model = ControlModel::DirectNormal;
    event.object = DIM.into();
    assert_eq!(
        bridge.apply(&mut state, &event, DEADLINE, NOW),
        Err(BridgeError::InvalidValue)
    );
    event.object = "CLSLD0/GGIO1$CO$Other".into();
    assert_eq!(
        bridge.apply(&mut state, &event, DEADLINE, NOW),
        Err(BridgeError::UnmappedControl)
    );
    assert_eq!(state, initial);
    assert!(RelayBridge::new(DIM, DIM).is_err());
    assert!(RelayBridge::new("CLSLD0/GGIO1$CO$Ctl.nested", DIM).is_err());
    assert!(RelayBridge::new(GENERATION, "CLSLD0/GGIO1$ST$Dim$stVal").is_err());
}

#[test]
fn status_reads_report_observed_bits_even_when_they_conflict() {
    let leaf = |i| format!("CLSLD0/GGIO1$ST$Ind{i}$stVal");
    let map = RelayFeedback::new(&leaf(1), &leaf(2), &leaf(3), &leaf(4)).unwrap();
    assert!(RelayFeedback::new(&leaf(1), &leaf(2), &leaf(3), &leaf(1)).is_err());
    assert!(RelayFeedback::new(&leaf(1), &leaf(2), &leaf(3), DIM).is_err());
    for alias in ["CLSLD0/GGIO1$ST$Ind1.stVal", "CLSLD0/GGIO1$ST$Ind(1)$stVal"] {
        assert!(RelayFeedback::new(&leaf(1), &leaf(2), &leaf(3), alias).is_err());
    }
    let names = [
        "GGIO1$ST$Ind4$stVal",
        "GGIO1$ST$Ind1$stVal",
        "GGIO1$ST$Ind2$stVal",
        "GGIO1$ST$Ind3$stVal",
        "GGIO1$ST$Other$stVal",
    ];
    let wire = Mms::ConfirmedRequest {
        invoke_id: 42,
        service: ConfirmedRequest::Read {
            specification_with_result: false,
            access: VariableAccess::ListOfVariable(
                names
                    .iter()
                    .map(|item| {
                        VariableSpecification::Name(ObjectName::DomainSpecific {
                            domain: "CLSLD0",
                            item,
                        })
                    })
                    .collect(),
            ),
        },
    }
    .to_vec()
    .unwrap();
    let Mms::ConfirmedRequest { service, invoke_id } = Mms::parse(&wire, &Limits::DEFAULT).unwrap()
    else {
        unreachable!()
    };
    let observed = RelayContacts {
        s1: true,
        s2: true,
        w3: false,
        w4: false,
    };
    assert!(observed.decode().is_err());
    let expected = |bits: [Result<bool, i64>; 4]| {
        let mut result: Vec<_> = bits.into_iter().map(|v| v.map(Value::Boolean)).collect();
        result.push(Err(DATA_ACCESS_NON_EXISTENT));
        Answer::Read(result).encode(invoke_id).unwrap()
    };
    assert_eq!(
        map.answer_read(&service, Some(observed))
            .encode(invoke_id)
            .unwrap(),
        expected([Ok(false), Ok(true), Ok(true), Ok(false)])
    );
    assert_eq!(
        map.answer_read(&service, None).encode(invoke_id).unwrap(),
        expected([Err(2); 4])
    );
    assert_eq!(
        map.answer_read(&ConfirmedRequest::Identify, Some(observed)),
        Answer::DENIED
    );
}

#[test]
fn tariff_boundaries_leave_the_commissioned_command_in_force() {
    use smgw_cls::fnn::tariff::{TariffLevel, TariffSchedule, TariffWindow};
    let bridge = RelayBridge::new(GENERATION, DIM).unwrap();
    let mut state = RelayState::new(GenerationStage::Percent100, ConsumptionMode::Dimmed);
    bridge
        .apply(
            &mut state,
            &operate::production_operate(30.0, 4),
            DEADLINE,
            NOW,
        )
        .unwrap();
    let windows = [
        TariffWindow {
            start_utc: 0,
            end_utc: 60,
            level: TariffLevel::Low,
        },
        TariffWindow {
            start_utc: 60,
            end_utc: 120,
            level: TariffLevel::High,
        },
    ];
    let schedule = TariffSchedule::new(&windows).unwrap();
    for (utc, expected) in [
        (59, Some(TariffLevel::Low)),
        (60, Some(TariffLevel::High)),
        (120, None),
    ] {
        assert_eq!(schedule.active_at(utc).map(|w| w.level), expected);
        assert_eq!(
            state,
            RelayState::new(GenerationStage::Percent30, ConsumptionMode::Dimmed)
        );
    }
}
