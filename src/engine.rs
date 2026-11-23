//! A caller-clocked lifecycle for one outgoing CLS connection.
//!
//! Supply elapsed monotonic time. Whenever [`Session::deadline`] changes, arm a
//! timer and call [`Session::tick`] on expiry, including while the peer is idle.
//! The driver owns sockets and must close the generation named by [`Action::Close`].

use crate::reconnect_delay;
use core::time::Duration;

/// Timing and retry limits for one call to [`Session::start`].
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    /// Maximum time allowed for a connection attempt.
    pub connect_timeout: Duration,
    /// Maximum lifetime after authentication succeeds.
    pub session_lifetime: Duration,
    /// Delay before the first retry.
    pub initial_delay: Duration,
    /// Upper bound on any retry delay.
    pub maximum_delay: Duration,
    /// Additional attempts allowed across this lifecycle.
    pub retries: u32,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            session_lifetime: Duration::from_secs(60),
            initial_delay: Duration::from_secs(1),
            maximum_delay: Duration::from_secs(10),
            retries: 3,
        }
    }
}

/// An instruction referring to exactly one transport attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Start establishing and authenticating the connection.
    Connect { generation: u64 },
    /// Deliver payloads on the authenticated stream.
    Ready { generation: u64 },
    /// Release all transport resources for this attempt.
    Close { generation: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Connecting,
    Active,
    Backoff,
}

/// Lifecycle of a single connection. It owns no buffers or operating-system resources.
#[derive(Debug)]
pub struct Session {
    policy: Policy,
    state: State,
    generation: u64,
    retries_used: u32,
    deadline: Option<Duration>,
}

impl Session {
    /// Create an idle engine with a fixed retry policy.
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            state: State::Idle,
            generation: 0,
            retries_used: 0,
            deadline: None,
        }
    }

    /// The next absolute deadline on the caller's monotonic clock.
    pub fn deadline(&self) -> Option<Duration> {
        self.deadline
    }

    /// Start an idle lifecycle with a fresh retry budget.
    pub fn start(&mut self, now: Duration) -> Option<Action> {
        if self.state != State::Idle {
            return None;
        }
        self.retries_used = 0;
        self.connect(now)
    }

    fn connect(&mut self, now: Duration) -> Option<Action> {
        // Exhaustion leaves the session idle and prevents generation reuse.
        self.reset();
        self.generation = self.generation.checked_add(1)?;
        self.state = State::Connecting;
        self.deadline = Some(now.saturating_add(self.policy.connect_timeout));
        Some(Action::Connect {
            generation: self.generation,
        })
    }

    /// Report a verified, authorized handshake. A callback at the deadline is late.
    pub fn connected(&mut self, generation: u64, now: Duration) -> Option<Action> {
        if generation != self.generation || self.state != State::Connecting {
            return None;
        }
        if self.deadline.is_some_and(|at| now >= at) {
            return self.tick(now);
        }
        self.state = State::Active;
        self.deadline = Some(now.saturating_add(self.policy.session_lifetime));
        Some(Action::Ready { generation })
    }

    /// Report a failure. Only transient failures consume a retry.
    pub fn failed(&mut self, generation: u64, now: Duration, retryable: bool) -> Option<Action> {
        if generation != self.generation || !matches!(self.state, State::Active | State::Connecting)
        {
            return None;
        }
        self.reset();
        if retryable && self.retries_used < self.policy.retries {
            self.deadline = Some(now.saturating_add(reconnect_delay(
                self.policy.initial_delay,
                self.policy.maximum_delay,
                self.retries_used,
            )));
            self.retries_used += 1;
            self.state = State::Backoff;
        }
        Some(Action::Close { generation })
    }

    /// Process an expired timer. Early calls and obsolete timer wakeups are harmless.
    pub fn tick(&mut self, now: Duration) -> Option<Action> {
        if self.deadline.is_none_or(|at| now < at) {
            return None;
        }
        match self.state {
            State::Backoff => self.connect(now),
            State::Connecting | State::Active => self.failed(self.generation, now, true),
            State::Idle => None,
        }
    }

    /// Stop the lifecycle, cancel timers and close any current transport.
    pub fn cancel(&mut self) -> Option<Action> {
        let close = matches!(self.state, State::Active | State::Connecting);
        self.reset();
        close.then_some(Action::Close {
            generation: self.generation,
        })
    }

    fn reset(&mut self) {
        self.state = State::Idle;
        self.deadline = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn at(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    #[test]
    fn stale_handshake_after_cancel_is_ignored() {
        let mut s = Session::new(Policy::default());
        assert_eq!(s.start(at(0)), Some(Action::Connect { generation: 1 }));
        assert_eq!(s.cancel(), Some(Action::Close { generation: 1 }));
        assert_eq!(s.connected(1, at(1)), None);
        assert_eq!(s.deadline(), None);
        assert_eq!(s.start(at(2)), Some(Action::Connect { generation: 2 }));
        assert_eq!(s.failed(1, at(3), true), None);
        assert_eq!(s.connected(1, at(3)), None);
        assert_eq!(s.connected(2, at(3)), Some(Action::Ready { generation: 2 }));
    }

    #[test]
    fn deadlines_and_retries_are_bounded() {
        let mut s = Session::new(Policy {
            retries: 1,
            ..Policy::default()
        });
        s.start(at(0));
        assert_eq!(s.start(at(1)), None);
        assert_eq!(s.tick(at(4)), None);
        assert_eq!(s.connected(1, at(5)), Some(Action::Close { generation: 1 }));
        assert_eq!(s.deadline(), Some(at(6)));
        assert_eq!(s.tick(at(6)), Some(Action::Connect { generation: 2 }));
        s.connected(2, at(7));
        assert_eq!(s.tick(at(67)), Some(Action::Close { generation: 2 }));
        assert_eq!(s.deadline(), None);
    }

    #[test]
    fn permanent_failure_and_cancelled_backoff_stop_retries() {
        let mut s = Session::new(Policy::default());
        s.start(at(0));
        s.failed(1, at(1), false);
        assert_eq!(s.deadline(), None);
        s.start(at(2));
        s.failed(2, at(3), true);
        assert_eq!(s.cancel(), None);
        assert_eq!(s.tick(Duration::MAX), None);
    }
}
