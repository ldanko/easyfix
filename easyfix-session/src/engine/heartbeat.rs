//! Heartbeat and TestRequest handling, including keep-alive timeouts.

use std::{borrow::Cow, num::NonZeroU64, time::Duration};

use chrono::Utc;
use easyfix_core::{
    base_messages::{AdminBase, HeaderBase, HeartbeatBase, MsgTypeBase, TestRequestBase},
    basic_types::{FixString, Int, MsgTypeField, NonZeroSeqNum, SeqNum},
    fix_str,
    message::SessionMessage,
};
use tracing::{error, warn};

use super::{FatalError, HandlerResult, LogonState, SessionEngine};
use crate::{application::DisconnectReason, messages_storage::MessagesStorage};

impl<M: SessionMessage> SessionEngine<M> {
    /// Hold new keep-alive TestRequests throughout preparation and ACK wait.
    /// Outstanding probes must still receive their matching Heartbeats.
    fn keep_alive_probes_held(&self) -> bool {
        self.reset_phase().is_some()
            || (matches!(self.state.logon_state, LogonState::LogoutSent { .. })
                && self.state.local_reset_unconfirmed)
    }

    /// Hold all regular keep-alive output after our reset Logon, including
    /// during LogoutSent while its reset remains unconfirmed.
    fn keep_alive_silent(&self) -> bool {
        self.state.logon_state == LogonState::ResetSent
            || (matches!(self.state.logon_state, LogonState::LogoutSent { .. })
                && self.state.local_reset_unconfirmed)
    }

    #[cfg(test)]
    pub(crate) fn set_heartbeat_interval_in_force(&mut self, interval: Option<NonZeroU64>) {
        self.state.heartbeat_interval = interval;
    }

    /// Effective heartbeat interval, or `None` when heartbeats are disabled
    /// (negotiated `HeartBtInt=0`, FIX Transport §5.1). The single source of
    /// truth for the IO loop's deadlines: the output heartbeat fires at this
    /// interval and the input-timeout TestRequest at 1.2x it; when `None` both
    /// deadlines stay unarmed.
    pub(crate) fn heartbeat_interval(&self) -> Option<Duration> {
        self.state
            .heartbeat_interval
            .map(|secs| Duration::from_secs(secs.get()))
    }

    /// Push a Heartbeat to admin_output.
    pub(crate) fn send_heartbeat(&mut self, test_req_id: Option<FixString>) {
        self.push_admin(AdminBase::Heartbeat(HeartbeatBase {
            test_req_id: test_req_id.map(Cow::Owned),
        }));
    }

    /// Push a TestRequest to admin_output.
    pub(super) fn send_test_request(&mut self, test_req_id: FixString) {
        self.push_admin(AdminBase::TestRequest(TestRequestBase {
            test_req_id: Cow::Owned(test_req_id),
        }));
    }

    pub(super) fn on_heartbeat<S: MessagesStorage>(
        &mut self,
        header: &HeaderBase<'_>,
        _heartbeat: HeartbeatBase<'_>,
        storage: &mut S,
    ) -> Result<HandlerResult, FatalError> {
        self.ensure_healthy()?;

        let msg_type = MsgTypeField::from(MsgTypeBase::Heartbeat);
        Ok(self
            .validate(header, msg_type, storage)?
            .unwrap_or(HandlerResult::AdminMsg))
    }

    pub(super) fn process_heartbeat<S: MessagesStorage>(
        &mut self,
        _header: &HeaderBase<'_>,
        heartbeat: HeartbeatBase<'_>,
        storage: &mut S,
    ) -> Result<HandlerResult, FatalError> {
        self.ensure_healthy()?;

        // Session Layer §4.5.5 answers a TestRequest only with a Heartbeat
        // carrying its TestReqID(112); Transport §5.1 reads the same rule
        // without the id ("If there is still no Heartbeat message
        // received"). `verify_test_request_id` picks between the two. Either
        // way the probe is cleared by a Heartbeat and by nothing else -
        // Testcases §4.5.5 Scenario 6 notes the answer "may not be the next
        // message received", so intervening traffic proves nothing.
        if self.session_settings.verify_test_request_id {
            if let Some(test_req_id) = heartbeat.test_req_id.as_deref() {
                self.state.grace_period_test_req_ids.remove(test_req_id);
            }
        } else {
            self.state.grace_period_test_req_ids.clear();
        }
        self.advance_target(storage)?;
        if let Some(id) = heartbeat.test_req_id.as_deref() {
            self.state.reset_barrier_ids.remove(id);
        }
        if self.state.logon_state == LogonState::ResetProbe
            && heartbeat.test_req_id.as_deref() == self.state.reset_probe_id.as_deref()
        {
            if storage.next_target_msg_seq_num().get() == SeqNum::MAX {
                warn!("reset probe response exhausted incoming sequence numbers");
                return Ok(HandlerResult::Handled);
            }
            if self.state.probe_stale
                || !self.state.queue.is_empty()
                || !self.inbound_recovery_finished(storage)
            {
                self.state.reset_probe_id = None;
                self.state.probe_stale = false;
                self.state.logon_state = LogonState::ResetPending;
                return Ok(HandlerResult::Handled);
            }
            // The IO drain must commit old-numbered replies before this
            // Heartbeat can discard their store and start fresh numbering.
            debug_assert!(self.admin_output.is_empty());
            if !self.admin_output.is_empty() {
                error!("sequence number reset reached with unflushed admin output");
            }
            self.reset_storage(storage)?;
            self.state.reset_probe_id = None;
            self.discard_recovery_state();
            self.state.local_reset_unconfirmed = true;
            let next_expected = self
                .session_settings
                .enable_next_expected_msg_seq_num
                .then_some(1);
            self.state.next_expected_msg_seq_num = next_expected.and_then(NonZeroSeqNum::new);
            // Production writers originate in u16 settings or nonnegative
            // wire Int values, so the effective interval always fits Int.
            let heart_bt_int =
                Int::try_from(self.state.heartbeat_interval.map_or(0, NonZeroU64::get))
                    .unwrap_or(Int::MAX);
            self.send_logon_response(heart_bt_int, true, next_expected);
            self.state.logon_state = LogonState::ResetSent;
        }
        Ok(HandlerResult::Handled)
    }

    pub(super) fn on_test_request<S: MessagesStorage>(
        &mut self,
        header: &HeaderBase<'_>,
        _test_request: TestRequestBase<'_>,
        storage: &mut S,
    ) -> Result<HandlerResult, FatalError> {
        self.ensure_healthy()?;

        let msg_type = MsgTypeField::from(MsgTypeBase::TestRequest);
        Ok(self
            .validate(header, msg_type, storage)?
            .unwrap_or(HandlerResult::AdminMsg))
    }

    pub(super) fn process_test_request<S: MessagesStorage>(
        &mut self,
        _header: &HeaderBase<'_>,
        test_request: TestRequestBase<'_>,
        storage: &mut S,
    ) -> Result<HandlerResult, FatalError> {
        self.ensure_healthy()?;

        self.send_heartbeat(Some(test_request.test_req_id.into_owned()));
        self.advance_target(storage)?;
        Ok(HandlerResult::Handled)
    }

    /// No input received within timeout. Send TestRequest or escalate.
    ///
    /// If grace period is active (unanswered TestRequests >= limit),
    /// sends Logout and sets disconnect. Otherwise generates a unique
    /// TestReqID, sends TestRequest, and registers it for grace period
    /// tracking. Does nothing while the session is not logged on.
    //
    // Silent while not logged on because a `TestRequest<1>` there asks the peer
    // to prove the liveness of a session that does not exist yet. Worse, if our
    // own `Logon<A>` never reached the peer, the probe becomes its first
    // message - and a first message that is not a Logon puts us on the
    // disconnect path (Session Layer §4.3.1; Test Cases Scenario 2S), which is
    // exactly what `check_logon_state` does to a peer in `Idle`. An incomplete
    // handshake is `logon_deadline`'s job instead.
    //
    // `is_logged_on()` also covers `LogoutSent`, which a session can enter
    // straight from `Idle` / `LogonSent` when the application or a `ControlMsg`
    // logs out mid-handshake. Probing there is a deliberate non-exception: we
    // have written a `Logout<5>` the peer owes an answer to, so the keep-alive
    // has something to keep alive, and `logout_deadline` bounds it regardless.
    pub(crate) fn on_input_timeout(&mut self) {
        if !self.is_logged_on() || self.keep_alive_probes_held() {
            return;
        }
        let limit = usize::from(
            self.session_settings
                .auto_disconnect_after_no_heartbeat
                .get(),
        );
        if self.state.grace_period_test_req_ids.len() >= limit {
            warn!("Grace period is over");
            self.push_logout(None, Some(fix_str!("Heartbeat timeout").to_owned()));
            self.begin_disconnect(DisconnectReason::HeartbeatTimeout);
            return;
        }

        let test_req_id = FixString::from_ascii_lossy(
            Utc::now()
                .format("%Y%m%d-%H:%M:%S.%f")
                .to_string()
                .into_bytes(),
        );
        self.state
            .grace_period_test_req_ids
            .insert(test_req_id.clone());
        self.send_test_request(test_req_id);
    }

    /// No output sent within timeout. Sends a Heartbeat once the session is
    /// logged on, and nothing before that.
    //
    // Same reasoning as [`Self::on_input_timeout`]: a `Heartbeat<0>` that
    // overtakes a lost `Logon<A>` is a first message that is not a Logon.
    pub(crate) fn on_output_timeout(&mut self) {
        if !self.is_logged_on() || self.keep_alive_silent() {
            return;
        }
        self.send_heartbeat(None);
    }

    /// Number of outstanding (unanswered) TestRequest probes. The IO loop
    /// uses this to decide whether to drive `on_input_timeout` from the
    /// inbound-resettable `input_deadline` (grace empty) or the independent
    /// `grace_deadline` (probe outstanding).
    pub(crate) fn grace_period_count(&self) -> usize {
        self.state.grace_period_test_req_ids.len()
    }
}
