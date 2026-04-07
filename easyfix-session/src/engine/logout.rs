//! Logout exchange and waiting for the peer to close the connection.

use std::time::Instant;

use easyfix_core::{
    base_messages::{AdminBase, HeaderBase, LogoutBase, MsgTypeBase, SessionStatusBase},
    basic_types::{FixString, MsgTypeField, SessionStatusField},
    fix_str,
    message::SessionMessage,
};
use tracing::{info, warn};

use super::{FatalError, HandlerResult, LogonState, SessionEngine, output::text_field};
use crate::{application::DisconnectReason, messages_storage::MessagesStorage};

impl<M: SessionMessage> SessionEngine<M> {
    /// When the session gives up waiting for the peer to close the connection
    /// after its Logout was acknowledged. `None` unless that is what the
    /// session is doing.
    pub(crate) fn awaiting_peer_close(&self) -> Option<Instant> {
        if let LogonState::LogoutAcknowledged { sent_at } = self.state.logon_state {
            sent_at.checked_add(self.session_settings.auto_disconnect_after_no_logout)
        } else {
            None
        }
    }

    /// When the session gives up waiting for the peer's `Logout<5>` reply.
    /// `None` while no Logout of ours is outstanding.
    pub(crate) fn logout_deadline(&self) -> Option<Instant> {
        if let LogonState::LogoutSent { sent_at } = self.state.logon_state {
            sent_at.checked_add(self.session_settings.auto_disconnect_after_no_logout)
        } else {
            None
        }
    }

    /// Push a Logout to admin_output and transition to
    /// [`LogonState::LogoutSent`] capturing the send instant - for a Logout
    /// the session then waits to have answered. A farewell on a connection
    /// that ends in this same iteration is [`Self::push_logout`] instead.
    /// The shared `push_logout` gate suppresses repeats in both ending states;
    /// suppression preserves the state and original `sent_at`.
    pub(super) fn send_logout(
        &mut self,
        session_status: Option<SessionStatusField>,
        text: Option<FixString>,
    ) {
        if self.push_logout(session_status, text) {
            self.state.reset_probe_id = None;
            self.state.logon_state = LogonState::LogoutSent {
                sent_at: self.timer_backend.now(),
            };
        }
    }

    /// Push a Logout to admin_output without entering
    /// [`LogonState::LogoutSent`] - for a farewell on a connection that ends in
    /// this same iteration, where there is no response to wait for.
    /// Returns false once a Logout has been sent or acknowledged, without
    /// adding a message or changing the first Logout's deadline.
    //
    // The state is not a cosmetic difference. `LogoutSent` makes
    // `accepts_app_sends()` true unless a reset is unconfirmed. Entering it
    // from an ordinary `LogonSent` would put an application message on the
    // wire ahead of a `Logon<A>` that was never
    // acknowledged (Session Layer §4.3.10).
    pub(super) fn push_logout(
        &mut self,
        session_status: Option<SessionStatusField>,
        text: Option<FixString>,
    ) -> bool {
        if matches!(
            self.state.logon_state,
            LogonState::LogoutSent { .. } | LogonState::LogoutAcknowledged { .. }
        ) {
            return false;
        }
        self.push_admin(AdminBase::Logout(LogoutBase {
            session_status,
            text: text_field(text, MsgTypeBase::Logout),
        }));
        true
    }

    pub(super) fn on_logout<S: MessagesStorage>(
        &mut self,
        header: &HeaderBase<'_>,
        _logout: LogoutBase<'_>,
        storage: &mut S,
    ) -> Result<HandlerResult, FatalError> {
        self.ensure_healthy()?;

        Ok(if self.state.local_reset_unconfirmed {
            self.validate_impl(
                header,
                MsgTypeBase::Logout.into(),
                storage,
                false,
                false,
                false,
            )?
            .unwrap_or(HandlerResult::AdminMsg)
        } else if self.session_settings.verify_logout {
            let msg_type = MsgTypeField::from(MsgTypeBase::Logout);
            self.validate(header, msg_type, storage)?
                .unwrap_or(HandlerResult::AdminMsg)
        } else {
            HandlerResult::AdminMsg
        })
    }

    pub(super) fn process_logout<S: MessagesStorage>(
        &mut self,
        _header: &HeaderBase<'_>,
        _logout: LogoutBase<'_>,
        storage: &mut S,
    ) -> Result<HandlerResult, FatalError> {
        self.ensure_healthy()?;

        self.state.reset_probe_id = None;
        self.advance_target(storage)?;
        Ok(match self.state.logon_state {
            LogonState::LogoutSent { .. } => {
                info!("Received logout response");
                HandlerResult::Disconnect(DisconnectReason::LocalRequestedLogout)
            }
            // The peer's request: acknowledge it, then leave the connection
            // to the peer. The Logout initiator is the one who terminates
            // the transport (FIX Session Layer §4.6, Figure 9); the IO loop
            // waits out the close, bounded by `awaiting_peer_close` (Test
            // Cases Scenario 13(b)), and names the reason from what ends the
            // wait.
            _ => {
                info!("Received logout request");
                self.push_logout(
                    Some(SessionStatusBase::SessionLogoutComplete.into()),
                    Some(fix_str!("Responding").to_owned()),
                );
                self.state.logon_state = LogonState::LogoutAcknowledged {
                    sent_at: self.timer_backend.now(),
                };
                HandlerResult::Handled
            }
        })
    }

    pub(super) fn push_unexpected_reset_logout(&mut self, msg_type: MsgTypeField) {
        let name = MsgTypeBase::ALL
            .iter()
            .find(|&&kind| kind == msg_type)
            .map_or_else(|| "MsgType".to_owned(), |kind| format!("{kind:?}"));
        self.push_logout(
            None,
            Some(FixString::from_ascii_lossy(
                format!("Unexpected {name}({msg_type}) during sequence number reset").into_bytes(),
            )),
        );
    }

    /// Logout response not received in time.
    pub(crate) fn on_logout_timeout(&mut self) {
        self.begin_disconnect(DisconnectReason::LocalRequestedLogoutTimeout);
    }

    /// The peer did not close the connection after its Logout was
    /// acknowledged. Test Cases Scenario 13(b) has us disconnect and report
    /// an error condition.
    pub(crate) fn on_peer_close_timeout(&mut self) {
        warn!("peer did not close the connection after its Logout was acknowledged");
        self.begin_disconnect(DisconnectReason::RemoteRequestedLogoutTimeout);
    }
}
