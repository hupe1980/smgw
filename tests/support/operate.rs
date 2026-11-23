use iec61850_kit::{
    Limits, UtcTime,
    ber::Cursor,
    common::monotonic_now,
    proto::{
        data::Value,
        mms::{
            ConfirmedRequest, Mms, ObjectName, VariableAccess, VariableSpecification,
            control::ControlRequest,
        },
    },
    server::{Acsi, Answer, ControlEvent, Ied},
};

/// Capture the control delivered by the ACSI hook for an encoded MMS Write.
pub(super) fn production_operate(percent: f32, number: u8) -> ControlEvent {
    let ied = Ied::from_scl(include_str!("production.cid"), Some("CLS")).unwrap();
    let mut acsi = Acsi::new(ied);
    let (sender, receiver) = std::sync::mpsc::channel();
    acsi.on_control(Box::new(move |event| {
        sender.send(event.clone()).unwrap();
        Ok(())
    }));
    acsi.on_association_opened(1, Vec::new());
    acsi.set_authenticated(1, true);

    let request = ControlRequest::new(
        Value::Structure(vec![Value::Float32(percent)]),
        number,
        UtcTime::from_unix_nanos(0, Default::default()),
    );
    let data = Value::encode_all(&[request.to_value()]).unwrap();
    let access = VariableAccess::ListOfVariable(vec![VariableSpecification::Name(
        ObjectName::DomainSpecific {
            domain: "CLSLD0",
            item: "DWMX1$CO$WMaxSetPct$Oper",
        },
    )]);
    let wire = Mms::ConfirmedRequest {
        invoke_id: 1,
        service: ConfirmedRequest::Write {
            access,
            values: vec![Cursor::new(&data).next_required().unwrap()],
        },
    }
    .to_vec()
    .unwrap();
    let Mms::ConfirmedRequest { service, .. } = Mms::parse(&wire, &Limits::DEFAULT).unwrap() else {
        panic!("expected an MMS confirmed request")
    };
    let answer = acsi.request(1, monotonic_now(), &service);
    assert!(
        matches!(answer, Answer::Write(ref results) if results == &[Ok(())]),
        "{answer:?}"
    );
    receiver
        .try_recv()
        .expect("ACSI delivered the operate event")
}
