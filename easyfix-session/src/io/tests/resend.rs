use std::{assert_matches, ops::RangeInclusive, time::Duration};

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase},
    basic_types::SeqNum,
    deserializer::raw_message,
    fix_str,
    message::{HeaderAccess, MsgCat, SessionMessage},
};
use easyfix_test_messages::Message;
use rstest::rstest;
use tokio::{
    io,
    io::{AsyncReadExt, AsyncWriteExt},
    time,
};

use super::{
    harness::TestEvent, reset_peer::running_reset_peer, reset_transport::gated_reset_peer,
    wire::wire_bytes,
};
use crate::{
    application::{EndContext, SessionEndReason, TimeoutKind},
    io::{flush_output, process_one_resend},
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{EngineBuilder, SessionEndSnapshot, TerminationSnapshot, nz_seq},
};

#[rstest]
#[tokio::test(flavor = "local", start_paused = true)]
async fn stalled_inbound_recovery_times_out_despite_garbled_replays_and_new_messages(
    #[values(false, true)] redundant_requests: bool,
    #[values(false, true)] heartbeats_enabled: bool,
) {
    let mut settings = test_helpers::default_session_settings();
    settings.send_redundant_resend_requests = redundant_requests;
    settings.heartbeat_interval = heartbeats_enabled.then(|| 60.try_into().unwrap());
    let mut peer = running_reset_peer(settings).await;
    let start = time::Instant::now();

    let mut replay = test_helpers::heartbeat(2, None);
    replay.header.poss_dup_flag = Some(true);
    replay.header.orig_sending_time = Some(replay.header.sending_time);
    let mut garbled = test_helpers::serialize_message(&replay);
    let checksum_digit = garbled.len() - 2;
    garbled[checksum_digit] = if garbled[checksum_digit] == b'0' {
        b'1'
    } else {
        b'0'
    };
    let mut batch = garbled.clone();
    batch.extend_from_slice(&test_helpers::heartbeat_bytes(3));
    peer.wire.write_all(&batch).await.unwrap();
    let request = peer.read().await;
    assert_matches!(request.try_as_admin(), Some(AdminBase::ResendRequest(r))
        if r.begin_seq_no == 2 && r.end_seq_no == 2);

    for (seconds, seq) in [(10, 4), (20, 5)] {
        time::sleep_until(start + Duration::from_secs(seconds)).await;
        let mut batch = garbled.clone();
        batch.extend_from_slice(&test_helpers::heartbeat_bytes(seq));
        peer.wire.write_all(&batch).await.unwrap();
        if redundant_requests {
            let request = peer.read().await;
            assert_matches!(request.try_as_admin(), Some(AdminBase::ResendRequest(r))
                if r.begin_seq_no == 2 && r.end_seq_no == 2);
        }
        // Let every ready input frame drain before the paused clock advances.
        time::sleep(Duration::from_millis(1)).await;
        peer.assert_silent();
        assert!(!peer.task.is_finished());
        assert!(peer.events.try_recv().is_err());
    }

    time::sleep_until(start + Duration::from_secs(30)).await;
    let logout = peer.read().await;
    assert_eq!(start.elapsed(), Duration::from_secs(30));
    assert_matches!(logout.try_as_admin(), Some(AdminBase::Logout(logout))
    if logout.text.as_deref() == Some(fix_str!(
        "Inbound recovery timeout, expecting MsgSeqNum 2"
    )));
    assert_matches!(
        peer.events.recv().await.unwrap(),
        TestEvent::SessionEnd(SessionEndSnapshot {
            context: EndContext::Closing(SessionEndReason::Timeout(TimeoutKind::InboundRecovery)),
            termination: TerminationSnapshot::Timeout(TimeoutKind::InboundRecovery),
            reset_unconfirmed: false
        })
    );
    let storage = peer.task.await.unwrap();
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    assert!(peer.buffer.is_empty());
    let mut trailing = Vec::new();
    peer.wire.read_to_end(&mut trailing).await.unwrap();
    assert!(trailing.is_empty());
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn inbound_queue_overflow_sends_logout_without_consuming_the_gap() {
    let mut settings = test_helpers::default_session_settings();
    settings.heartbeat_interval = None;
    settings.max_inbound_queued_messages = Some(2.try_into().unwrap());
    let mut peer = running_reset_peer(settings).await;
    let start = time::Instant::now();

    let mut batch = test_helpers::heartbeat_bytes(3);
    batch.extend_from_slice(&test_helpers::heartbeat_bytes(4));
    peer.wire.write_all(&batch).await.unwrap();
    let request = peer.read().await;
    assert_matches!(request.try_as_admin(), Some(AdminBase::ResendRequest(r))
        if r.begin_seq_no == 2 && r.end_seq_no == 2);
    peer.send(&test_helpers::heartbeat(5, None)).await;

    let logout = peer.read().await;
    assert_eq!(start.elapsed(), Duration::ZERO);
    assert_matches!(logout.try_as_admin(), Some(AdminBase::Logout(logout))
    if logout.text.as_deref() == Some(fix_str!(
        "Inbound queue limit 2 exceeded, expecting MsgSeqNum 2"
    )));
    assert_matches!(
        peer.events.recv().await.unwrap(),
        TestEvent::SessionEnd(SessionEndSnapshot {
            context: EndContext::Closing(SessionEndReason::InboundQueueLimitExceeded),
            termination: TerminationSnapshot::LocalClose,
            reset_unconfirmed: false
        })
    );
    let storage = peer.task.await.unwrap();
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    assert!(peer.buffer.is_empty());
    let mut trailing = Vec::new();
    peer.wire.read_to_end(&mut trailing).await.unwrap();
    assert!(trailing.is_empty());
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn inbound_recovery_timeout_stops_outgoing_replay_between_batches() {
    let mut settings = test_helpers::default_session_settings();
    settings.heartbeat_interval = None;
    settings.write_timeout = Duration::from_secs(60);
    let (mut peer, mut gate) = gated_reset_peer(settings).await;
    for seq in 2..=3 {
        peer.sender
            .send(test_helpers::new_order_single_with_empty_header())
            .unwrap();
        assert_eq!(peer.read().await.msg_seq_num(), seq);
    }
    peer.send(&test_helpers::heartbeat(4, None)).await;
    let request = peer.read().await;
    assert_matches!(request.try_as_admin(), Some(AdminBase::ResendRequest(r))
        if r.begin_seq_no == 2 && r.end_seq_no == 3);

    // The peer's too-high ResendRequest starts outgoing replay without
    // advancing NextNumIn. Hold its first write past the recovery deadline.
    gate.armed.set(true);
    peer.send(&test_helpers::resend_request(5, 2, 3)).await;
    let pending = Message::from_bytes(&gate.entered.recv().await.unwrap()).unwrap();
    assert_eq!(pending.msg_seq_num(), 2);
    assert_eq!(pending.poss_dup_flag(), Some(true));
    assert_eq!(pending.msg_cat(), MsgCat::App);
    time::advance(Duration::from_secs(31)).await;
    assert!(!peer.task.is_finished(), "in-flight IO is not cancelled");
    gate.armed.set(false);
    gate.release.send(Ok(())).unwrap();

    let replay = peer.read().await;
    assert_eq!(replay.msg_seq_num(), 2);
    assert_eq!(replay.poss_dup_flag(), Some(true));
    // The next replay batch would send seq 3. Expiry must be checked
    // before that batch, even though active replay bypasses event select.
    let logout = peer.read().await;
    assert_matches!(logout.try_as_admin(), Some(AdminBase::Logout(logout))
    if logout.text.as_deref() == Some(fix_str!(
        "Inbound recovery timeout, expecting MsgSeqNum 2"
    )));
    assert_matches!(
        peer.events.recv().await.unwrap(),
        TestEvent::AdminMsgIn(MsgTypeBase::ResendRequest)
    );
    assert_matches!(
        peer.events.recv().await.unwrap(),
        TestEvent::SessionEnd(SessionEndSnapshot {
            context: EndContext::Closing(SessionEndReason::Timeout(TimeoutKind::InboundRecovery)),
            termination: TerminationSnapshot::Timeout(TimeoutKind::InboundRecovery),
            reset_unconfirmed: false
        })
    );
    let storage = peer.task.await.unwrap();
    assert_eq!(storage.next_target_msg_seq_num().get(), 2);
    assert!(peer.buffer.is_empty());
    let mut trailing = Vec::new();
    peer.wire.read_to_end(&mut trailing).await.unwrap();
    assert!(trailing.is_empty());
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn recovering_one_gap_requests_the_next_queued_gap_without_new_input() {
    let mut settings = test_helpers::default_session_settings();
    settings.heartbeat_interval = None;
    let mut peer = running_reset_peer(settings).await;
    let mut batch = test_helpers::heartbeat_bytes(4);
    batch.extend_from_slice(&test_helpers::heartbeat_bytes(6));
    peer.wire.write_all(&batch).await.unwrap();
    let request = peer.read().await;
    assert_matches!(request.try_as_admin(), Some(AdminBase::ResendRequest(r))
        if r.begin_seq_no == 2 && r.end_seq_no == 3);

    let mut recovered = test_helpers::heartbeat_bytes(2);
    recovered.extend_from_slice(&test_helpers::heartbeat_bytes(3));
    peer.wire.write_all(&recovered).await.unwrap();
    // Once 2 and 3 close the first gap, the queued 4 is processed. The
    // queued 6 must expose the next gap without another read from the peer.
    let request = peer.read().await;
    assert_matches!(request.try_as_admin(), Some(AdminBase::ResendRequest(r))
        if r.begin_seq_no == 5 && r.end_seq_no == 5);
    for _ in 2..=4 {
        assert_matches!(
            peer.events.recv().await.unwrap(),
            TestEvent::AdminMsgIn(MsgTypeBase::Heartbeat)
        );
    }

    time::advance(Duration::from_secs(30)).await;
    let logout = peer.read().await;
    assert_matches!(logout.try_as_admin(), Some(AdminBase::Logout(logout))
    if logout.text.as_deref() == Some(fix_str!(
        "Inbound recovery timeout, expecting MsgSeqNum 5"
    )));
    assert_matches!(
        peer.events.recv().await.unwrap(),
        TestEvent::SessionEnd(SessionEndSnapshot {
            context: EndContext::Closing(SessionEndReason::Timeout(TimeoutKind::InboundRecovery)),
            termination: TerminationSnapshot::Timeout(TimeoutKind::InboundRecovery),
            reset_unconfirmed: false
        })
    );
    let storage = peer.task.await.unwrap();
    assert_eq!(storage.next_target_msg_seq_num().get(), 5);
    assert!(peer.buffer.is_empty());
    let mut trailing = Vec::new();
    peer.wire.read_to_end(&mut trailing).await.unwrap();
    assert!(trailing.is_empty());
}

/// `process_one_resend` must produce at most one [`PendingOutput::Transient`]
/// per call so the shared `engine.scratch` buffer is drained before the
/// next serialize overwrites it. Specifically: when the next seq_num is a
/// real resend AND a gap-fill is currently accumulating, the function
/// flushes the gap on this invocation and leaves the cursor on the resend's
/// seq_num - the resend itself is processed on the following call.
///
/// This test drives `process_one_resend` end-to-end across a 3..=4 range
/// where seq 3 is admin (gap-filled) and seq 4 is app (resent), and asserts
/// the wire output is the gap-fill SequenceReset followed by the resent
/// NewOrderSingle with `PossDupFlag=Y`.
#[tokio::test]
async fn process_one_resend_defers_real_resend_with_pending_gap() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
    let mut handler = test_helpers::StubHandler;

    // Skip seq 1 and 2 - irrelevant to this test. Commits land at 3 and 4.
    storage.set_next_sender_msg_seq_num(nz_seq(3)).unwrap();

    // seq 3 - admin Heartbeat (resend_as_gap_fill returns true).
    engine.send_heartbeat(None);
    let mut hb = engine.take_admin_output().expect("heartbeat queued");
    assert!(engine.fill_header(&mut hb, &mut storage).unwrap());
    engine
        .commit_send(hb, &mut storage)
        .expect("commit succeeds");

    // seq 4 - app NewOrderSingle (default should_gap_fill returns false).
    let mut nos = test_helpers::new_order_single_with_empty_header();
    assert!(engine.fill_header(&mut nos, &mut storage).unwrap());
    engine
        .commit_send(nos, &mut storage)
        .expect("commit succeeds");

    // Drop the Stored entries the commits pushed - they're outside the
    // scope of this test. The resend path operates on storage bytes,
    // not the live output queue.
    while engine.take_pending().is_some() {}

    let mut active_resend: Option<RangeInclusive<SeqNum>> = Some(3..=4);
    let (mut writer, reader) = io::duplex(16384);

    // Iteration 1: seq 3 (admin) - accumulate gap, advance to 4..=4.
    let cont = process_one_resend(&mut active_resend, &mut storage, &mut engine, &mut handler)
        .await
        .unwrap();
    assert!(cont, "iter 1 should continue");
    assert_eq!(active_resend.as_ref().unwrap(), &(4..=4));
    assert!(engine.has_accumulated_resend_gap());
    flush_output(&mut writer, &mut storage, &mut engine)
        .await
        .expect("flush ok");

    // Iteration 2: would resend seq 4, but a gap-fill is pending -
    // flush the gap, defer the resend. Cursor must NOT advance.
    let cont = process_one_resend(&mut active_resend, &mut storage, &mut engine, &mut handler)
        .await
        .unwrap();
    assert!(cont, "iter 2 should continue");
    assert_eq!(
        active_resend.as_ref().unwrap(),
        &(4..=4),
        "cursor must stay on seq 4 when deferring resend"
    );
    assert!(
        !engine.has_accumulated_resend_gap(),
        "gap-fill should have been flushed"
    );
    flush_output(&mut writer, &mut storage, &mut engine)
        .await
        .expect("flush ok");

    // Iteration 3: seq 4 (app, no gap pending) - process resend, exhaust range.
    let cont = process_one_resend(&mut active_resend, &mut storage, &mut engine, &mut handler)
        .await
        .unwrap();
    assert!(cont, "iter 3 should continue");
    assert!(active_resend.is_none(), "range exhausted after iter 3");
    flush_output(&mut writer, &mut storage, &mut engine)
        .await
        .expect("flush ok");

    // Iteration 4: range already None - returns false.
    let cont = process_one_resend(&mut active_resend, &mut storage, &mut engine, &mut handler)
        .await
        .unwrap();
    assert!(!cont, "iter 4 - range exhausted, should return false");

    let wire = wire_bytes(writer, reader).await;

    // First wire message: gap-fill SequenceReset for seq 3.
    let (rest, raw1) = raw_message(&wire).expect("first wire message must parse");
    let parsed1 = Message::from_raw_message(raw1).expect("first message must deserialize");
    assert_eq!(
        SessionMessage::msg_type(&*parsed1),
        MsgTypeBase::SequenceReset,
        "first wire message should be the gap-fill SequenceReset"
    );
    assert_eq!(
        HeaderAccess::msg_seq_num(&*parsed1),
        3,
        "gap-fill should reference seq 3"
    );

    // Second wire message: resent NewOrderSingle at seq 4 with PossDupFlag=Y.
    let (rest, raw2) = raw_message(rest).expect("second wire message must parse");
    let parsed2 = Message::from_raw_message(raw2).expect("second message must deserialize");
    assert_eq!(parsed2.msg_cat(), MsgCat::App);
    assert_eq!(HeaderAccess::msg_seq_num(&*parsed2), 4);
    assert_eq!(HeaderAccess::poss_dup_flag(&*parsed2), Some(true));

    assert!(rest.is_empty(), "no leftover bytes on wire");
}
