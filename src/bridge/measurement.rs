//! Read total active power reported by a commissioned EEBUS monitoring actor over MMS.

use core::{fmt, time::Duration};
use eebus::{
    model::{CmdData, FeatureType, Function, MeasurementDescriptionData, MeasurementId, Role},
    ship::Ski,
    spine::Engine,
    usecases::monitoring::{
        Measurand, MonitoringApplianceActor, MonitoringEvent, Reading, ReadingState, UnitId,
    },
    usecases::{mgcp, mpc},
};
use iec61850_kit::{
    ObjectReference,
    common::{Quality, TimeQuality, UtcTime, Validity},
    proto::{
        data::Value,
        mms::{ConfirmedRequest, ObjectName, VariableAccess, VariableSpecification},
    },
    server::{Answer, DATA_ACCESS_NON_EXISTENT},
};

/// The actor that resolved the SPINE measurements. MPC covers an appliance,
/// MGCP the grid connection point. Both use positive consumption and negative production.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasurementSource {
    Mpc,
    Mgcp,
}

/// The mapping is invalid or the peer could not be commissioned for total active power.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidMeasurementMapping;
impl fmt::Display for InvalidMeasurementMapping {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "expected a logical device and node, a device and entity, and a positive maximum age",
        )
    }
}
impl std::error::Error for InvalidMeasurementMapping {}

const UNAVAILABLE: i64 = 2; // ISO 9506 DataAccessError temporarily-unavailable
const INVALID: Quality = Quality {
    validity: Validity::Invalid,
    ..Quality::GOOD
};

/// One commissioned MMXU total active power mapping. Configure distinct mappings
/// for independently monitored sources. A new mapping starts without a sample.
pub struct PowerReadback {
    domain: String,
    node: String,
    source: MeasurementSource,
    peer: Ski,
    unit: UnitId,
    max_age: Duration,
    described: bool,
    latest: Option<Duration>,
    source_timestamp: Option<Duration>,
    watts: Option<f32>,
    observed: Duration,
    expires: Duration,
    quality: Quality,
}

impl PowerReadback {
    /// Bind MMXU TotW at a logical device and node to an authenticated SHIP
    /// identity and the exact SPINE device/entity discovered on that association.
    /// Call [Self::commission] after the actor resolves its measurement descriptions.
    pub fn new(
        domain: &str,
        logical_node: &str,
        source: MeasurementSource,
        peer: Ski,
        unit: UnitId,
        max_age: Duration,
    ) -> Result<Self, InvalidMeasurementMapping> {
        let name = format!("{domain}/{logical_node}");
        let r = ObjectReference::parse(&name).map_err(|_| InvalidMeasurementMapping)?;
        if r.ld != domain
            || r.ln != logical_node
            || max_age.is_zero()
            || unit.device.as_str().trim().is_empty()
            || unit.entity.is_empty()
        {
            return Err(InvalidMeasurementMapping);
        }
        Ok(Self {
            domain: domain.into(),
            node: logical_node.into(),
            source,
            peer,
            unit,
            max_age,
            described: false,
            latest: None,
            source_timestamp: None,
            watts: None,
            observed: Duration::ZERO,
            expires: Duration::ZERO,
            quality: INVALID,
        })
    }

    /// Verify the authenticated session's advertised use case, exact entity,
    /// actor-resolved total-power ID, and original SPINE measurement description in watts.
    /// Run this after the initial description exchange and whenever the peer changes its
    /// discovery or descriptions. Recommissioning this peer discards the current sample.
    pub fn commission(
        &mut self,
        peer: &Ski,
        engine: &Engine,
        actor: &MonitoringApplianceActor,
    ) -> Result<(), InvalidMeasurementMapping> {
        if *peer != self.peer {
            return Err(InvalidMeasurementMapping);
        }
        self.disconnect();
        let remote = engine
            .peer(&self.unit.device)
            .ok_or(InvalidMeasurementMapping)?;
        let (descriptor, scenario) = match self.source {
            MeasurementSource::Mpc => (&mpc::MONITORED_UNIT, 1),
            MeasurementSource::Mgcp => (&mgcp::GRID_CONNECTION_POINT, 2),
        };
        let feature = actor
            .units()
            .find(|p| p.is_unit(&self.unit))
            .and_then(|p| p.measurement.as_ref())
            .ok_or(InvalidMeasurementMapping)?;
        if UnitId::of(feature).as_ref() != Some(&self.unit)
            || !remote
                .use_cases_played(descriptor.name, descriptor.actor)
                .any(|uc| {
                    uc.available
                        && uc.scenarios.contains(&scenario)
                        && remote
                            .address_of(uc, &FeatureType::Measurement, Role::Server)
                            .as_ref()
                            == Some(feature)
                })
        {
            return Err(InvalidMeasurementMapping);
        }
        let mut ids = actor
            .readings(&self.unit)
            .ok_or(InvalidMeasurementMapping)?
            .described()
            .filter_map(|(id, kind)| (kind == Measurand::total_power()).then_some(id));
        let id = ids.next().ok_or(InvalidMeasurementMapping)?;
        let Some(CmdData::MeasurementDescriptionListData(list)) =
            engine.remote_data(feature, &Function::MeasurementDescriptionListData)
        else {
            return Err(InvalidMeasurementMapping);
        };
        if ids.next().is_some()
            || !watt_description(
                list.measurement_description_data
                    .as_deref()
                    .unwrap_or_default(),
                id,
            )
        {
            return Err(InvalidMeasurementMapping);
        }
        self.described = true;
        Ok(())
    }

    /// Feed an event with the use case and authenticated SHIP identity supplied by
    /// its connection. Supply synchronized UTC since the Unix epoch at reception
    /// and a monotonic reading from the clock used for MMS reads. Only absolute
    /// timestamps with a known offset can establish freshness across merged notifications.
    pub fn observe(
        &mut self,
        source: MeasurementSource,
        peer: &Ski,
        event: &MonitoringEvent,
        utc: Duration,
        now: Duration,
    ) {
        if self.source != source || self.peer != *peer {
            return;
        }
        let readings = match event {
            MonitoringEvent::Measured { unit, readings }
                if unit == &self.unit && self.described =>
            {
                readings
            }
            _ => return,
        };
        let mut total = readings
            .iter()
            .filter(|r| r.measurand == Measurand::total_power());
        let Some(reading) = total.next() else { return };
        if total.next().is_some() || now < self.observed {
            self.source_timestamp = None;
            self.invalidate(INVALID);
            return;
        }
        self.observed = now;
        let Some(stamp) = source_time(reading, utc) else {
            self.source_timestamp = None;
            self.invalidate(INVALID);
            return;
        };
        if self.latest.is_some_and(|previous| stamp < previous) {
            return;
        }
        let duplicate = self.latest == Some(stamp);
        self.latest = Some(stamp);
        self.source_timestamp = Some(stamp);
        let watts = reading.value.and_then(|raw| {
            let rounded = raw as f32;
            (rounded.is_finite() && (raw == 0.0 || rounded != 0.0)).then_some(rounded)
        });
        let Some(watts) = watts.filter(|_| reading.state == ReadingState::Normal) else {
            self.invalidate(Quality {
                out_of_range: reading.state == ReadingState::OutOfRange,
                failure: reading.state == ReadingState::Error,
                ..INVALID
            });
            return;
        };
        let age = utc - stamp;
        if age >= self.max_age {
            self.invalidate(Quality {
                old_data: true,
                ..INVALID
            });
            return;
        }
        let expiry = now.saturating_add(self.max_age - age);
        if !duplicate {
            self.expires = expiry;
            self.watts = Some(watts);
            self.quality = Quality::GOOD;
        } else if self.watts.is_some_and(|old| old != watts) {
            self.invalidate(Quality {
                inconsistent: true,
                ..INVALID
            });
        } else {
            self.expires = self.expires.min(expiry);
        }
    }

    /// Mark a lost or detached connection unavailable immediately. Reattach the
    /// actor and call [Self::commission] before admitting newer values.
    pub fn disconnect(&mut self) {
        self.described = false;
        self.source_timestamp = None;
        self.invalidate(INVALID);
    }

    fn invalidate(&mut self, quality: Quality) {
        self.watts = None;
        self.quality = quality;
    }

    /// Serve direct named reads of `TotW$mag$f` (signed watts), `TotW$q` and
    /// `TotW$t`. Missing or expired watts return MMS temporarily-unavailable.
    /// Encode the returned answer with the association's original invoke ID.
    pub fn answer_read(&self, request: &ConfirmedRequest<'_>, now: Duration) -> Answer {
        let ConfirmedRequest::Read {
            specification_with_result: false,
            access: VariableAccess::ListOfVariable(vars),
        } = request
        else {
            return Answer::DENIED;
        };
        let quality = if self.watts.is_some() && (now < self.observed || now >= self.expires) {
            Quality {
                old_data: true,
                ..INVALID
            }
        } else {
            self.quality
        };
        Answer::Read(
            vars.iter()
                .map(|var| {
                    let VariableSpecification::Name(ObjectName::DomainSpecific { domain, item }) =
                        var
                    else {
                        return Err(DATA_ACCESS_NON_EXISTENT);
                    };
                    let Some((node, path)) = item.split_once('$') else {
                        return Err(DATA_ACCESS_NON_EXISTENT);
                    };
                    if *domain != self.domain || node != self.node {
                        return Err(DATA_ACCESS_NON_EXISTENT);
                    }
                    match path {
                        "MX$TotW$mag$f" if quality == Quality::GOOD => {
                            self.watts.map(Value::Float32).ok_or(UNAVAILABLE)
                        }
                        "MX$TotW$mag$f" => Err(UNAVAILABLE),
                        "MX$TotW$q" => Ok(Value::quality(quality)),
                        "MX$TotW$t" => self
                            .source_timestamp
                            .map(|stamp| {
                                Value::UtcTime(UtcTime::from_unix(
                                    stamp.as_secs() as u32,
                                    stamp.subsec_nanos(),
                                    TimeQuality::UNSYNCHRONIZED,
                                ))
                            })
                            .ok_or(UNAVAILABLE),
                        _ => Err(DATA_ACCESS_NON_EXISTENT),
                    }
                })
                .collect(),
        )
    }
}

fn watt_description(rows: &[MeasurementDescriptionData], id: MeasurementId) -> bool {
    let mut matches = rows.iter().filter(|d| d.measurement_id == Some(id));
    let Some(row) = matches.next() else {
        return false;
    };
    let total = Measurand::total_power();
    matches.next().is_none()
        && row.unit == Some(total.unit())
        && row.measurement_type == Some(total.measurement_type())
        && row.scope_type == Some(total.scope_type())
}

fn source_time(reading: &Reading, utc: Duration) -> Option<Duration> {
    let time = reading.taken_at()?.as_timestamp()?;
    let stamp = Duration::new(time.unix_seconds()?.try_into().ok()?, time.nanosecond());
    (stamp <= utc && stamp.as_secs() <= u32::MAX.into()).then_some(stamp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eebus::model::{AbsoluteOrRelativeTime, ElectricalConnectionPhaseName};
    use eebus::usecases::monitoring::Quantity;

    const SKI: Ski = Ski::from_bytes([0x51; 20]);
    const UTC: Duration = Duration::from_secs(100);
    const NOW: Duration = Duration::from_secs(2);
    const STAMP: &str = "1970-01-01T00:01:38.500Z";
    const SOURCE: MeasurementSource = MeasurementSource::Mgcp;

    fn unit() -> UnitId {
        UnitId {
            device: "grid-meter".into(),
            entity: vec![2],
        }
    }

    fn mapping() -> PowerReadback {
        PowerReadback::new("LD0", "MMXU1", SOURCE, SKI, unit(), Duration::from_secs(5)).unwrap()
    }

    fn reading(stamp: &str) -> Reading {
        Reading {
            measurand: Measurand::total_power(),
            state: ReadingState::Normal,
            value: Some(-2700.0),
            timestamp: Some(AbsoluteOrRelativeTime::from(stamp)),
        }
    }

    fn describe(map: &mut PowerReadback) {
        map.described = true; // Wire commissioning is exercised with both actual monitoring actors.
    }

    fn measured(r: Reading) -> MonitoringEvent {
        MonitoringEvent::Measured {
            unit: unit(),
            readings: vec![r],
        }
    }

    fn read(map: &PowerReadback, now: Duration) -> Vec<Result<Value, i64>> {
        let vars = [
            "MMXU1$MX$TotW$mag$f",
            "MMXU1$MX$TotW$q",
            "MMXU1$MX$TotW$t",
            "MMXU2$MX$TotW$mag$f",
        ]
        .map(|item| {
            VariableSpecification::Name(ObjectName::DomainSpecific {
                domain: "LD0",
                item,
            })
        });
        let Answer::Read(result) = map.answer_read(
            &ConfirmedRequest::Read {
                specification_with_result: false,
                access: VariableAccess::ListOfVariable(vars.into()),
            },
            now,
        ) else {
            panic!("expected read response")
        };
        assert_eq!(result[3], Err(DATA_ACCESS_NON_EXISTENT));
        result
    }

    #[test]
    fn only_described_exact_peer_source_and_entity_can_publish_total_power() {
        let mut map = mapping();
        let event = measured(reading(STAMP));
        map.observe(SOURCE, &SKI, &event, UTC, NOW);
        assert_eq!(read(&map, NOW)[0], Err(UNAVAILABLE));
        describe(&mut map);
        map.observe(MeasurementSource::Mpc, &SKI, &event, UTC, NOW);
        map.observe(SOURCE, &Ski::from_bytes([9; 20]), &event, UTC, NOW);
        let mut other = unit();
        other.entity = vec![3];
        map.observe(
            SOURCE,
            &SKI,
            &MonitoringEvent::Measured {
                unit: other,
                readings: vec![reading(STAMP)],
            },
            UTC,
            NOW,
        );
        let mut phase = reading(STAMP);
        phase.measurand = Measurand::on(Quantity::Power, ElectricalConnectionPhaseName::A);
        map.observe(SOURCE, &SKI, &measured(phase), UTC, NOW);
        assert_eq!(read(&map, NOW)[0], Err(UNAVAILABLE));

        map.observe(SOURCE, &SKI, &event, UTC, NOW);
        let r = read(&map, NOW);
        assert_eq!(r[0], Ok(Value::Float32(-2700.0)));
        assert_eq!(r[1], Ok(Value::quality(Quality::GOOD)));
        assert_eq!(
            r[2],
            Ok(Value::UtcTime(UtcTime::from_unix(
                98,
                500_000_000,
                TimeQuality::UNSYNCHRONIZED
            )))
        );
    }

    #[test]
    fn source_age_counts_and_duplicates_never_extend_expiry() {
        let mut map = mapping();
        describe(&mut map);
        let event = measured(reading(STAMP));
        map.observe(SOURCE, &SKI, &event, UTC, NOW);
        let second = Duration::from_secs(1);
        map.observe(SOURCE, &SKI, &event, UTC + second, NOW + second);
        let expiry = NOW + Duration::from_millis(3500);
        assert_eq!(
            read(&map, expiry - Duration::from_nanos(1))[0],
            Ok(Value::Float32(-2700.0))
        );
        let old = read(&map, expiry);
        assert_eq!(old[0], Err(UNAVAILABLE));
        assert_eq!(
            old[1],
            Ok(Value::quality(Quality {
                old_data: true,
                ..INVALID
            }))
        );
        assert_eq!(read(&map, NOW)[0], Err(UNAVAILABLE)); // monotonic time before last observation
    }

    #[test]
    fn malformed_or_unresolvable_timestamp_never_falls_back_to_receipt() {
        for stamp in [
            "not-time",
            "PT1S",
            "-PT1S",
            "1970-01-01T00:01:38",
            "1970-01-01T00:01:41Z",
        ] {
            let mut map = mapping();
            describe(&mut map);
            map.observe(SOURCE, &SKI, &measured(reading(STAMP)), UTC, NOW);
            for delta in [0, 1, 8] {
                let delta = Duration::from_secs(delta);
                map.observe(
                    SOURCE,
                    &SKI,
                    &measured(reading(stamp)),
                    UTC + delta,
                    NOW + delta,
                );
                // A future absolute time can become eligible on a later notification.
                if stamp == "1970-01-01T00:01:41Z" && !delta.is_zero() {
                    break;
                }
                let r = read(&map, NOW + delta);
                assert_eq!(r[0], Err(UNAVAILABLE), "{stamp}");
                assert_eq!(r[2], Err(UNAVAILABLE), "{stamp}");
            }
        }
        let mut map = mapping();
        describe(&mut map);
        let mut missing = reading(STAMP);
        missing.timestamp = None;
        map.observe(SOURCE, &SKI, &measured(missing), UTC, NOW);
        assert_eq!(read(&map, NOW)[0], Err(UNAVAILABLE));
    }

    #[test]
    fn errors_out_of_range_duplicates_and_nonfinite_values_fail_closed() {
        for (state, value) in [
            (ReadingState::Error, Some(88.0)),
            (ReadingState::OutOfRange, Some(88.0)),
            (ReadingState::Normal, None),
            (ReadingState::Normal, Some(f64::NAN)),
            (ReadingState::Normal, Some(f64::MAX)),
            (ReadingState::Normal, Some(f64::MIN_POSITIVE)),
        ] {
            let mut map = mapping();
            describe(&mut map);
            map.observe(SOURCE, &SKI, &measured(reading(STAMP)), UTC, NOW);
            let mut bad = reading(STAMP);
            bad.value = value;
            bad.state = state;
            map.observe(SOURCE, &SKI, &measured(bad), UTC, NOW);
            let r = read(&map, NOW);
            assert_eq!(r[0], Err(UNAVAILABLE));
            assert_eq!(
                r[1],
                Ok(Value::quality(Quality {
                    out_of_range: state == ReadingState::OutOfRange,
                    failure: state == ReadingState::Error,
                    ..INVALID
                }))
            );
            map.observe(SOURCE, &SKI, &measured(reading(STAMP)), UTC, NOW);
            assert_eq!(read(&map, NOW)[0], Err(UNAVAILABLE));
        }
        let mut map = mapping();
        describe(&mut map);
        map.observe(
            SOURCE,
            &SKI,
            &MonitoringEvent::Measured {
                unit: unit(),
                readings: vec![reading(STAMP), reading(STAMP)],
            },
            UTC,
            NOW,
        );
        assert_eq!(read(&map, NOW)[0], Err(UNAVAILABLE));
    }

    #[test]
    fn disconnect_requires_recommissioning_and_strictly_newer_sample() {
        let mut map = mapping();
        describe(&mut map);
        map.observe(SOURCE, &SKI, &measured(reading(STAMP)), UTC, NOW);
        map.disconnect();
        let newer = measured(reading("1970-01-01T00:01:39Z"));
        map.observe(SOURCE, &SKI, &newer, UTC, NOW);
        assert_eq!(read(&map, NOW)[0], Err(UNAVAILABLE));
        assert_eq!(read(&map, NOW)[2], Err(UNAVAILABLE));
        describe(&mut map);
        map.observe(SOURCE, &SKI, &measured(reading(STAMP)), UTC, NOW);
        assert_eq!(read(&map, NOW)[0], Err(UNAVAILABLE));
        map.observe(SOURCE, &SKI, &newer, UTC, NOW);
        assert_eq!(read(&map, NOW)[0], Ok(Value::Float32(-2700.0)));
    }
}
