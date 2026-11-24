//! Fixed-rate, single-value reception shared by instantaneous metering profiles.

/// Direction of a strict threshold crossing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Crossing {
    Above,
    Below,
}

pub(crate) struct UnexpectedSlot;

pub(crate) struct Samples<V> {
    next: Option<i64>,
    latest: Option<(i64, V)>,
    threshold: Option<i64>,
    pub(crate) periodic: bool,
}

impl<V: Copy> Samples<V> {
    pub(crate) fn new(first_utc: i64, periodic: bool, threshold: Option<i64>) -> Self {
        Self {
            next: Some(first_utc),
            latest: None,
            threshold,
            periodic,
        }
    }

    /// Callers check source and unit before submitting the slot.
    pub(crate) fn record(
        &mut self,
        utc: i64,
        value: Option<V>,
        magnitude: impl Fn(V) -> i64,
    ) -> Result<Option<(V, Option<Crossing>)>, UnexpectedSlot> {
        if self.next != Some(utc) {
            return Err(UnexpectedSlot);
        }
        self.next = utc.checked_add(60);
        let previous = self.latest.take();
        let Some(value) = value else {
            return Ok(None);
        };
        self.latest = Some((utc, value));
        let crossing = previous
            .zip(self.threshold)
            .and_then(|((_, last), threshold)| {
                let old = magnitude(last);
                let new = magnitude(value);
                if old <= threshold && new > threshold {
                    Some(Crossing::Above)
                } else if old >= threshold && new < threshold {
                    Some(Crossing::Below)
                } else {
                    None
                }
            });
        Ok((self.periodic || crossing.is_some()).then_some((value, crossing)))
    }

    pub(crate) fn current(&self, now: i64) -> Option<(i64, V)> {
        self.latest
            .filter(|(utc, _)| now >= *utc && now.abs_diff(*utc) < 60)
    }
}
