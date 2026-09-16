use std::assert_matches;

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase},
    basic_types::{Int, MsgTypeField, SeqNum},
    fix_str,
    message::{HeaderAccess, SessionMessage},
    serializer::SerializeError,
};
use easyfix_test_messages::Message;
use tokio::{
    io::{self, AsyncWriteExt, DuplexStream},
    sync::{mpsc, mpsc::error::TryRecvError},
    task::{self, JoinHandle, LocalSet},
};

use super::{
    harness::build_harness,
    wire::{build_order_missing_symbol_bytes, build_peer_logon},
};
use crate::{
    InputError, ValidationError,
    application::{Application, DisconnectReason, InputAction},
    io::{ControlMsg, InputStream, SessionOpening, sender::Sender, session_loop},
    messages_storage::MessagesStorage,
    session_id::SessionId,
    test_helpers::{self, DEFAULT_MAX_MESSAGE_SIZE, FailingStorage, FailureTiming, StorageOp},
};

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Ready,
    End(DisconnectReason),
    AdminIn(MsgTypeField, SeqNum),
    AppIn(SeqNum),
    AdminOut(MsgTypeField),
    Validation(Vec<u8>, ValidationError),
    Deserialize,
}

struct RecordingApp(mpsc::UnboundedSender<Event>);

impl Application<Message> for RecordingApp {
    async fn on_session_ready(&mut self, _: &SessionId, _: Sender<Message>) {
        self.0.send(Event::Ready).unwrap();
    }

    async fn on_session_end(&mut self, _: &SessionId, reason: DisconnectReason) {
        self.0.send(Event::End(reason)).unwrap();
    }

    async fn on_admin_msg_in(&mut self, msg: &Message) -> InputAction {
        self.0
            .send(Event::AdminIn(
                SessionMessage::msg_type(msg),
                msg.msg_seq_num(),
            ))
            .unwrap();
        InputAction::Accept
    }

    async fn on_app_msg_in(&mut self, msg: Box<Message>) -> InputAction {
        self.0.send(Event::AppIn(msg.msg_seq_num())).unwrap();
        InputAction::Accept
    }

    fn on_admin_msg_out(&mut self, msg: &mut Message) {
        self.0
            .send(Event::AdminOut(SessionMessage::msg_type(msg)))
            .unwrap();
    }

    fn on_input_error(&mut self, error: InputError<'_, Message>) {
        let event = match error {
            InputError::Deserialize(_) => Event::Deserialize,
            InputError::Validation { msg, error } => {
                Event::Validation(test_helpers::serialize_message(msg), error.clone())
            }
        };
        self.0.send(event).unwrap();
    }

    fn on_output_error(&mut self, _: Box<Message>, error: &SerializeError) {
        panic!("unexpected serialization error: {error}");
    }
}

struct Session {
    wire: DuplexStream,
    buffer: Vec<u8>,
    events: mpsc::UnboundedReceiver<Event>,
    control: mpsc::Sender<ControlMsg>,
    task: JoinHandle<()>,
}

impl Session {
    fn spawn<S: MessagesStorage + 'static>(first: Box<Message>, mut storage: S) -> Self {
        let harness = build_harness();
        let (server, wire) = io::duplex(8192);
        let (reader, writer) = io::split(server);
        let (events_tx, events) = mpsc::unbounded_channel();
        let control = harness.control_tx;
        let task = task::spawn_local(async move {
            session_loop(
                SessionOpening::FirstMessage(Ok(first)),
                InputStream::new(reader, DEFAULT_MAX_MESSAGE_SIZE),
                writer,
                harness.engine,
                &mut storage,
                RecordingApp(events_tx),
                harness.sender,
                harness.app_rx,
                harness.control_rx,
            )
            .await;
        });
        Self {
            wire,
            buffer: Vec::new(),
            events,
            control,
            task,
        }
    }

    async fn handshake(&mut self) {
        assert_eq!(
            self.events.recv().await.unwrap(),
            Event::AdminIn(MsgTypeBase::Logon.into(), 1)
        );
        assert_eq!(
            self.events.recv().await.unwrap(),
            Event::AdminOut(MsgTypeBase::Logon.into())
        );
        assert_eq!(
            SessionMessage::msg_type(&*self.read().await),
            MsgTypeBase::Logon
        );
        assert_eq!(self.events.recv().await.unwrap(), Event::Ready);
    }

    async fn send(&mut self, msg: &Message) {
        self.wire
            .write_all(&test_helpers::serialize_message(msg))
            .await
            .unwrap();
    }

    async fn read(&mut self) -> Box<Message> {
        test_helpers::read_one_message(&mut self.wire, &mut self.buffer).await
    }

    async fn finish(mut self, reason: DisconnectReason) {
        assert_eq!(self.events.recv().await.unwrap(), Event::End(reason));
        self.task.await.unwrap();
        assert_matches!(self.events.try_recv(), Err(TryRecvError::Disconnected));
    }

    async fn stop(self) {
        self.control.send(ControlMsg::Disconnect).await.unwrap();
        self.finish(DisconnectReason::Disconnected).await;
    }
}

#[tokio::test]
async fn validation_reports_original_message_and_cause_before_reject() {
    LocalSet::new()
        .run_until(async {
            for (msg, error) in [
                (
                    test_helpers::resend_request(2, 0, 0),
                    ValidationError::InvalidResendRange,
                ),
                (
                    test_helpers::sequence_reset(2, 2, true),
                    ValidationError::InvalidNewSeqNo { expected: 2 },
                ),
                (
                    test_helpers::sequence_reset(2, 0, false),
                    ValidationError::InvalidNewSeqNo { expected: 2 },
                ),
            ] {
                let mut session =
                    Session::spawn(build_peer_logon(1, 30), test_helpers::default_storage());
                session.handshake().await;
                session.send(&msg).await;
                assert_eq!(
                    session.events.recv().await.unwrap(),
                    Event::Validation(test_helpers::serialize_message(&msg), error.clone())
                );
                assert_eq!(
                    session.events.recv().await.unwrap(),
                    Event::AdminOut(MsgTypeBase::Reject.into())
                );
                let reply = session.read().await;
                assert_matches!(reply.try_as_admin(), Some(AdminBase::Reject(reject))
                    if reject.ref_tag_id == error.tag().map(Int::from)
                        && reject.session_reject_reason == error.reject_reason());
                session.stop().await;
            }
        })
        .await;
}

#[tokio::test]
async fn invalid_first_logon_reports_error_before_logout_without_ready() {
    LocalSet::new()
        .run_until(async {
            let logon = build_peer_logon(1, -1);
            let original = test_helpers::serialize_message(&logon);
            let mut session = Session::spawn(logon, test_helpers::default_storage());
            assert_eq!(
                session.events.recv().await.unwrap(),
                Event::Validation(original, ValidationError::InvalidHeartBtInt)
            );
            assert_eq!(
                session.events.recv().await.unwrap(),
                Event::AdminOut(MsgTypeBase::Reject.into())
            );
            assert_eq!(
                SessionMessage::msg_type(&*session.read().await),
                MsgTypeBase::Reject
            );
            assert_eq!(
                session.events.recv().await.unwrap(),
                Event::AdminOut(MsgTypeBase::Logout.into())
            );
            assert_eq!(
                SessionMessage::msg_type(&*session.read().await),
                MsgTypeBase::Logout
            );
            session.finish(DisconnectReason::InvalidLogonState).await;
        })
        .await;
}

#[tokio::test]
async fn queued_message_is_reported_only_when_it_becomes_due() {
    LocalSet::new()
        .run_until(async {
            for invalid in [false, true] {
                let mut session =
                    Session::spawn(build_peer_logon(1, 30), test_helpers::default_storage());
                session.handshake().await;
                let mut queued = test_helpers::new_order_single(3);
                if invalid {
                    queued.header.poss_dup_flag = Some(true);
                }
                session.send(&queued).await;
                assert_matches!(
                    session.read().await.try_as_admin(),
                    Some(AdminBase::ResendRequest(rr))
                        if rr.begin_seq_no == 2 && rr.end_seq_no == 2
                );
                assert_eq!(
                    session.events.recv().await.unwrap(),
                    Event::AdminOut(MsgTypeBase::ResendRequest.into())
                );
                assert_matches!(session.events.try_recv(), Err(TryRecvError::Empty));

                session.send(&test_helpers::heartbeat(2, None)).await;
                assert_eq!(
                    session.events.recv().await.unwrap(),
                    Event::AdminIn(MsgTypeBase::Heartbeat.into(), 2)
                );
                if invalid {
                    assert_eq!(
                        session.events.recv().await.unwrap(),
                        Event::Validation(
                            test_helpers::serialize_message(&queued),
                            ValidationError::MissingOrigSendingTime
                        )
                    );
                    assert_eq!(
                        session.events.recv().await.unwrap(),
                        Event::AdminOut(MsgTypeBase::Reject.into())
                    );
                    assert_eq!(
                        SessionMessage::msg_type(&*session.read().await),
                        MsgTypeBase::Reject
                    );
                } else {
                    assert_eq!(session.events.recv().await.unwrap(), Event::AppIn(3));
                }
                session.stop().await;
            }
        })
        .await;
}

#[tokio::test]
async fn invalid_resend_above_gap_reports_once_before_reject_and_recovery_request() {
    LocalSet::new()
        .run_until(async {
            let mut session =
                Session::spawn(build_peer_logon(1, 30), test_helpers::default_storage());
            session.handshake().await;
            let invalid = test_helpers::resend_request(3, 0, 0);
            session.send(&invalid).await;
            assert_eq!(
                session.events.recv().await.unwrap(),
                Event::Validation(
                    test_helpers::serialize_message(&invalid),
                    ValidationError::InvalidResendRange
                )
            );
            for kind in [MsgTypeBase::Reject, MsgTypeBase::ResendRequest] {
                assert_eq!(
                    session.events.recv().await.unwrap(),
                    Event::AdminOut(kind.into())
                );
                assert_eq!(SessionMessage::msg_type(&*session.read().await), kind);
            }

            session.send(&test_helpers::heartbeat(2, None)).await;
            session
                .send(&test_helpers::test_request(4, fix_str!("after-gap")))
                .await;
            assert_eq!(
                session.events.recv().await.unwrap(),
                Event::AdminIn(MsgTypeBase::Heartbeat.into(), 2)
            );
            assert_eq!(
                session.events.recv().await.unwrap(),
                Event::AdminIn(MsgTypeBase::TestRequest.into(), 4)
            );
            assert_eq!(
                session.events.recv().await.unwrap(),
                Event::AdminOut(MsgTypeBase::Heartbeat.into())
            );
            assert_matches!(
                session.read().await.try_as_admin(),
                Some(AdminBase::Heartbeat(hb))
                    if hb.test_req_id.as_deref() == Some(fix_str!("after-gap"))
            );
            session.stop().await;
        })
        .await;
}

#[tokio::test]
async fn duplicate_and_deserialization_error_do_not_report_validation_error() {
    LocalSet::new()
        .run_until(async {
            let mut session =
                Session::spawn(build_peer_logon(1, 30), test_helpers::default_storage());
            session.handshake().await;
            let mut duplicate = test_helpers::heartbeat(1, None);
            duplicate.header.poss_dup_flag = Some(true);
            duplicate.header.orig_sending_time = Some(duplicate.header.sending_time);
            session.send(&duplicate).await;
            session
                .wire
                .write_all(&build_order_missing_symbol_bytes(2))
                .await
                .unwrap();
            assert_eq!(session.events.recv().await.unwrap(), Event::Deserialize);
            assert_eq!(
                session.events.recv().await.unwrap(),
                Event::AdminOut(MsgTypeBase::Reject.into())
            );
            assert_eq!(
                SessionMessage::msg_type(&*session.read().await),
                MsgTypeBase::Reject
            );
            session.send(&test_helpers::heartbeat(3, None)).await;
            assert_eq!(
                session.events.recv().await.unwrap(),
                Event::AdminIn(MsgTypeBase::Heartbeat.into(), 3)
            );
            session.stop().await;
        })
        .await;
}

#[tokio::test]
async fn validation_error_is_reported_even_when_reaction_fails_in_storage() {
    LocalSet::new()
        .run_until(async {
            let mut storage = FailingStorage::new();
            storage.fail_on(StorageOp::SetTarget, 2, FailureTiming::Before);
            let trace = storage.trace.clone();
            let mut session = Session::spawn(build_peer_logon(1, 30), storage);
            session.handshake().await;
            let mut invalid = test_helpers::heartbeat(2, None);
            invalid.header.poss_dup_flag = Some(true);
            session.send(&invalid).await;
            assert_eq!(
                session.events.recv().await.unwrap(),
                Event::Validation(
                    test_helpers::serialize_message(&invalid),
                    ValidationError::MissingOrigSendingTime
                )
            );
            session.finish(DisconnectReason::StorageError).await;
            assert!(trace.borrow().failed);
        })
        .await;
}
