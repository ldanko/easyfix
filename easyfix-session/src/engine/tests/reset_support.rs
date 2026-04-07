use easyfix_core::{
    base_messages::{AdminBase, SessionRejectReasonBase},
    basic_types::{FixStr, FixString, SeqNum},
    deserializer::DeserializeErrorKind,
    fix_str,
    message::DeserializeError,
};
use easyfix_test_messages::Message;

use crate::{
    application::InputAction,
    engine::{ResetPhase, SessionEngine},
    initiator::SessionStart,
    io::ControlMsg,
    messages_storage::{InMemoryStorage, MessagesStorage},
    test_helpers,
    test_helpers::{EngineBuilder, accept_input, as_admin, nz_seq, take_admin},
};

pub(super) fn commit_reset_admin(
    engine: &mut SessionEngine<Message>,
    storage: &mut InMemoryStorage,
) -> Box<Message> {
    let mut msg = take_admin(engine);
    assert!(engine.fill_header(&mut msg, storage).unwrap());
    let copy = msg.clone();
    assert!(engine.commit_send(msg, storage).is_ok());
    let _ = engine.take_pending();
    copy
}

pub(super) fn running_reset_probe(
    engine: &mut SessionEngine<Message>,
    storage: &mut InMemoryStorage,
) -> FixString {
    engine.on_control(ControlMsg::ResetRunningSession);
    assert_eq!(engine.reset_phase(), Some(ResetPhase::Pending));
    assert!(engine.reset_ready(true, storage));
    engine.start_reset_probe();
    committed_reset_probe_id(engine, storage)
}

pub(super) fn committed_reset_probe_id(
    engine: &mut SessionEngine<Message>,
    storage: &mut InMemoryStorage,
) -> FixString {
    let request = commit_reset_admin(engine, storage);
    let AdminBase::TestRequest(request) = as_admin(&request) else {
        panic!("expected reset TestRequest");
    };
    let id = request.test_req_id.into_owned();
    assert_eq!(engine.state.reset_probe_id.as_ref(), Some(&id));
    id
}

pub(super) fn running_reset_sent_engine() -> (SessionEngine<Message>, InMemoryStorage) {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    storage.set_next_sender_msg_seq_num(nz_seq(40)).unwrap();
    storage.set_next_target_msg_seq_num(nz_seq(40)).unwrap();
    let id = running_reset_probe(&mut engine, &mut storage);
    accept_input(
        &mut engine,
        test_helpers::heartbeat(40, Some(id)),
        &mut storage,
    );
    assert_eq!(engine.reset_phase(), Some(ResetPhase::Sent));
    let logon = commit_reset_admin(&mut engine, &mut storage);
    assert_eq!(logon.header.msg_seq_num, 1);
    (engine, storage)
}

// Send and commit an outgoing reset before isolating the receiving side.
pub(super) fn reset_waiting_engine(logout_sent: bool) -> (SessionEngine<Message>, InMemoryStorage) {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    engine
        .send_logon_request(&mut storage, SessionStart::Reset)
        .unwrap();
    let mut logon = take_admin(&mut engine);
    assert!(engine.fill_header(&mut logon, &mut storage).unwrap());
    assert!(engine.commit_send(logon, &mut storage).is_ok());
    let _ = engine.take_pending();
    assert!(engine.state.local_reset_unconfirmed);
    if logout_sent {
        engine.send_logout(None, Some(fix_str!("First Logout").to_owned()));
        let mut logout = take_admin(&mut engine);
        assert!(engine.fill_header(&mut logout, &mut storage).unwrap());
        assert!(engine.commit_send(logout, &mut storage).is_ok());
        let _ = engine.take_pending();
    }
    (engine, storage)
}

pub(super) fn reset_waiting_engine_with_origin(
    logout_sent: bool,
    running: bool,
) -> (SessionEngine<Message>, InMemoryStorage) {
    if !running {
        return reset_waiting_engine(logout_sent);
    }
    let (mut engine, mut storage) = running_reset_sent_engine();
    if logout_sent {
        engine.on_control(ControlMsg::Logout {
            session_status: None,
            text: Some(fix_str!("First Logout").to_owned()),
        });
        let _ = commit_reset_admin(&mut engine, &mut storage);
    }
    (engine, storage)
}

pub(super) fn reset_ack(
    seq: SeqNum,
    flag: Option<bool>,
    next_expected: Option<SeqNum>,
) -> Box<Message> {
    test_helpers::logon_with_options(
        seq,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        30,
        flag,
        next_expected,
    )
}

pub(super) fn broken_reset_response(
    msg_type: Option<&'static FixStr>,
    seq: SeqNum,
    with_header: bool,
) -> DeserializeError {
    let mut error: DeserializeError = DeserializeErrorKind::Reject {
        msg_type: msg_type.map(ToOwned::to_owned),
        seq_num: seq,
        tag: msg_type.map(|mt| if mt == "A" { 108 } else { 1409 }),
        reason: if msg_type.is_none() {
            SessionRejectReasonBase::InvalidMsgType
        } else {
            SessionRejectReasonBase::IncorrectDataFormatForValue
        }
        .into(),
    }
    .into();
    if with_header {
        error.header = Some(Box::new(test_helpers::inbound_header(seq)));
    }
    error
}

pub(super) fn reset_callback_action(index: usize) -> InputAction {
    match index {
        0 => InputAction::Reject {
            reason: SessionRejectReasonBase::ValueIsIncorrect.into(),
            text: None,
            tag: None,
        },
        1 => InputAction::Logout {
            session_status: None,
            text: None,
            disconnect: false,
        },
        2 => InputAction::Disconnect,
        _ => InputAction::Logout {
            session_status: None,
            text: None,
            disconnect: true,
        },
    }
}
