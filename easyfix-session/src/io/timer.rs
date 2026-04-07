use std::{
    future,
    time::{Duration, Instant},
};

use easyfix_core::message::SessionMessage;

use super::time::{Sleep, TimerBackend};
use crate::engine::{ResetPhase, SessionEngine};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug)]
pub(super) enum TimerEvent {
    Input,
    Output,
    Logon,
    Logout,
    Reset,
}

/// Timers for the running session. Synchronize them before waiting and report
/// input activity and completed writes at their points in the IO loop.
pub(super) struct SessionTimers {
    time: TimerBackend,
    heartbeat_interval: Option<Duration>,
    input_timeout: Option<Duration>,
    input: Option<Sleep>,
    output: Option<Sleep>,
    logon: Option<Sleep>,
    logout: Option<Sleep>,
    grace: Option<Sleep>,
    reset: ResetTimeout,
}

impl SessionTimers {
    /// Start with all timers disarmed.
    pub(super) fn new(heartbeat_interval: Option<Duration>, time: TimerBackend) -> Self {
        Self {
            time,
            heartbeat_interval,
            input_timeout: heartbeat_interval.map(|interval| interval.mul_f64(1.2)),
            input: None,
            output: None,
            logon: None,
            logout: None,
            grace: None,
            reset: ResetTimeout::default(),
        }
    }

    /// Synchronize handshake and keep-alive timers with the engine.
    /// Check the reset budget separately with [`Self::sync_reset_and_check`].
    pub(super) fn sync<M: SessionMessage>(&mut self, engine: &SessionEngine<M>) {
        sync_deadline(&mut self.logout, engine.logout_deadline(), self.time);
        sync_deadline(&mut self.logon, engine.logon_deadline(), self.time);

        // Start the keep-alive clocks at establishment. During the handshake
        // only the logon deadline bounds the wait.
        if let Some(timeout) = self.input_timeout {
            if !engine.is_logged_on() {
                self.input = None;
                self.grace = None;
            } else if engine.grace_period_count() > 0 {
                if self.grace.is_none() {
                    self.grace = Some(self.time.sleep(timeout));
                }
                self.input = None;
            } else {
                self.grace = None;
                if self.input.is_none() {
                    self.input = Some(self.time.sleep(timeout));
                }
            }
        }

        match (engine.is_logged_on(), self.heartbeat_interval, &self.output) {
            (true, Some(interval), None) => self.output = Some(self.time.sleep(interval)),
            (false, _, Some(_)) => self.output = None,
            _ => {}
        }
    }

    /// Synchronize the reset phase and report whether its budget has expired.
    /// Call at work boundaries even when the loop does not await an event.
    pub(super) fn sync_reset_and_check<M: SessionMessage>(
        &mut self,
        engine: &SessionEngine<M>,
    ) -> bool {
        self.reset.sync_and_check(engine, self.time)
    }

    /// Restart input-idle detection after non-garbled input. An outstanding
    /// probe's grace timer is unaffected by inbound activity.
    pub(super) fn on_input_received(&mut self) {
        reset_after(&mut self.input, self.input_timeout);
    }

    /// Restart the output timer after bytes have been written to the transport.
    pub(super) fn on_output_written(&mut self) {
        reset_after(&mut self.output, self.heartbeat_interval);
    }

    /// Rearm a periodic timer after the engine has handled its timeout.
    pub(super) fn on_timeout(&mut self, event: TimerEvent) {
        match event {
            TimerEvent::Input => {
                // Only one input timer is armed, and handling the event does
                // not synchronize them. Rearm whichever produced the event.
                reset_after(&mut self.input, self.input_timeout);
                reset_after(&mut self.grace, self.input_timeout);
            }
            TimerEvent::Output => self.on_output_written(),
            TimerEvent::Logon | TimerEvent::Logout | TimerEvent::Reset => {}
        }
    }

    /// Wait for one timer to expire. Call [`Self::on_timeout`] after handling it.
    /// Remains pending if all timers are disarmed. Cancelling the wait leaves
    /// deadlines unchanged; other expired timers remain available next time.
    pub(super) async fn next_event(&mut self) -> TimerEvent {
        // Keep selection among ready timers fair as well as the outer IO
        // select. All Sleep values outlive this cancellable wait.
        tokio::select! {
            () = elapsed(&mut self.input) => TimerEvent::Input,
            () = elapsed(&mut self.grace) => TimerEvent::Input,
            () = elapsed(&mut self.output) => TimerEvent::Output,
            () = elapsed(&mut self.logout) => TimerEvent::Logout,
            () = elapsed(&mut self.logon) => TimerEvent::Logon,
            () = elapsed(&mut self.reset.deadline) => TimerEvent::Reset,
        }
    }
}

fn sync_deadline(timer: &mut Option<Sleep>, deadline: Option<Instant>, time: TimerBackend) {
    match (deadline, &timer) {
        (Some(at), None) => *timer = Some(time.sleep_until(at)),
        (None, Some(_)) => *timer = None,
        _ => {}
    }
}

fn reset_after(timer: &mut Option<Sleep>, interval: Option<Duration>) {
    if let Some((timer, interval)) = timer.as_mut().zip(interval) {
        timer.reset_after(interval);
    }
}

async fn elapsed(timer: &mut Option<Sleep>) {
    match timer {
        Some(timer) => timer.await,
        None => future::pending().await,
    }
}

#[derive(Default)]
struct ResetTimeout {
    deadline: Option<Sleep>,
    at: Option<Instant>,
    phase: Option<ResetPhase>,
}

impl ResetTimeout {
    fn sync_and_check<M: SessionMessage>(
        &mut self,
        engine: &SessionEngine<M>,
        time: TimerBackend,
    ) -> bool {
        let phase = engine.reset_phase();
        let budget = match (self.phase, phase) {
            (None, Some(ResetPhase::Pending | ResetPhase::Probe)) => {
                Some(engine.session_settings().running_session_reset_timeout)
            }
            (previous, Some(ResetPhase::Sent)) if previous != Some(ResetPhase::Sent) => Some(
                engine
                    .session_settings()
                    .auto_disconnect_after_no_logon_response,
            ),
            _ => None,
        };
        if let Some(budget) = budget {
            self.at = time.now().checked_add(budget);
            self.deadline = self.at.map(|at| time.sleep_until(at));
        } else if phase.is_none() {
            self.at = None;
            self.deadline = None;
        }
        // Record even an unrepresentable deadline, so later work cannot
        // re-arm the same phase. Pending/Probe share the original budget.
        self.phase = phase;
        self.at.is_some_and(|at| time.now() >= at)
    }
}
