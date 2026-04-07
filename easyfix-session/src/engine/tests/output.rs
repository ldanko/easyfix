use std::assert_matches;

use easyfix_core::{
    base_messages::{AdminBase, MsgTypeBase, SessionRejectReasonBase},
    basic_types::{DateTime, FixString, SeqNum, TimePrecision, Utc, UtcTimestamp},
    fix_str,
    message::HeaderAccess,
    serializer::SerializeError,
    version::Version,
};
use easyfix_test_messages::Message;

use super::support::{assert_msg_type, limit};
use crate::{
    application::DisconnectReason,
    engine::{PendingOutput, SendFailure},
    messages_storage::MessagesStorage,
    test_helpers,
    test_helpers::{
        EngineBuilder, FailingStorage, FailureTiming, StorageOp, as_admin, nz_seq, take_admin,
    },
};

/// The scratch buffer is where transient output (resends, gap-fills) is
/// serialized, so it must hold a message of the configured maximum size.
#[test]
fn scratch_returns_buffer_of_at_least_max_message_size() {
    let (engine, _store) = EngineBuilder::new().max_message_size(limit(8192)).build();
    assert!(engine.scratch().len() >= 8192);
}

#[test]
fn send_reject_produces_reject_in_admin_output() {
    let (mut engine, _store) = EngineBuilder::new().logged_on().build();
    let text = fix_str!("Bad tag").to_owned();
    engine.send_reject(
        None,
        1,
        SessionRejectReasonBase::InvalidTagNumber.into(),
        Some(999),
        Some(text),
    );
    let msg = take_admin(&mut engine);
    assert_msg_type(&msg, MsgTypeBase::Reject);
    let AdminBase::Reject(reject) = as_admin(&msg) else {
        panic!("expected Reject");
    };
    assert_eq!(reject.ref_seq_num, 1);
    assert_eq!(reject.ref_tag_id, Some(999));
    assert_eq!(
        reject.session_reject_reason,
        Some(SessionRejectReasonBase::InvalidTagNumber.into())
    );
    assert_eq!(reject.text.as_deref(), Some(fix_str!("Bad tag")));
}

/// `Text(58)` is optional on `Reject<3>` (FIX Transport Section 5.5), so an
/// application that has nothing to add beyond `SessionRejectReason(373)` omits
/// the field. Both spellings of "no text" must reach that same encoding: an
/// empty value is not a legal FIX field - Session Layer reserves
/// `SessionRejectReason = 4, Tag specified without a value` for it - so
/// emitting `58=` would fail serialization and, before the Reject ever got a
/// seq num on the wire, cost the whole message.
#[test]
fn send_reject_omits_empty_text() {
    for text in [None, Some(FixString::default())] {
        let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
        engine.send_reject(
            None,
            1,
            SessionRejectReasonBase::InvalidTagNumber.into(),
            Some(999),
            text.clone(),
        );
        let mut msg = take_admin(&mut engine);
        let AdminBase::Reject(reject) = as_admin(&msg) else {
            panic!("expected Reject");
        };
        assert_eq!(
            reject.text, None,
            "empty Text(58) must be omitted: {text:?}"
        );

        // The reject must still serialize - that is the whole point.
        engine.fill_header(&mut msg, &mut storage).unwrap();
        let bytes = test_helpers::serialize_message(&msg);
        assert!(
            !bytes.windows(4).any(|w| w == b"\x0158="),
            "no Text(58) field may reach the wire: {:?}",
            String::from_utf8_lossy(&bytes)
        );
    }
}

fn messages_with_empty_headers() -> [(&'static str, Box<Message>); 2] {
    [
        ("Heartbeat", test_helpers::heartbeat_with_empty_header()),
        (
            "NewOrderSingle",
            test_helpers::new_order_single_with_empty_header(),
        ),
    ]
}

#[test]
fn fill_header_sets_empty_fields_and_increments_sender_counter() {
    for (case, mut msg) in messages_with_empty_headers() {
        let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
        assert_eq!(storage.next_sender_msg_seq_num().get(), 1, "{case}");
        assert_eq!(msg.msg_seq_num(), 0, "{case}");
        assert!(msg.sender_comp_id().is_empty(), "{case}");
        assert!(msg.target_comp_id().is_empty(), "{case}");
        assert_eq!(msg.sending_time(), UtcTimestamp::MIN_UTC, "{case}");

        let precision = engine.session_settings().time_precision;
        let before = Utc::now();
        engine.fill_header(&mut msg, &mut storage).unwrap();
        let after = Utc::now();

        assert_eq!(msg.version(), Version::FIXT11, "{case}");
        assert_eq!(msg.sender_comp_id(), fix_str!("SENDER"), "{case}");
        assert_eq!(msg.target_comp_id(), fix_str!("TARGET"), "{case}");
        assert_eq!(msg.msg_seq_num(), 1, "{case}");
        assert_eq!(storage.next_sender_msg_seq_num().get(), 2, "{case}");
        assert_ne!(msg.sending_time(), UtcTimestamp::MIN_UTC, "{case}");

        // Stamping truncates to the session precision, so apply the same
        // truncation to the lower bound before comparing timestamps.
        let lower = UtcTimestamp::with_precision(before, precision).timestamp();
        let sending_time = msg.sending_time().timestamp();
        assert!(
            sending_time >= lower && sending_time <= after,
            "{case}: SendingTime {sending_time} should be between {lower} and {after}"
        );
        assert_eq!(msg.sending_time().precision(), precision, "{case}");
    }
}

#[test]
fn fill_header_preserves_preset_sending_time() {
    for preset_time in [
        UtcTimestamp::now(TimePrecision::Nanos),
        UtcTimestamp::with_secs(DateTime::from_timestamp(1_700_000_000, 0).unwrap()),
    ] {
        for (case, mut msg) in messages_with_empty_headers() {
            let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
            msg.set_sending_time(preset_time);
            engine.fill_header(&mut msg, &mut storage).unwrap();
            assert_eq!(msg.sending_time(), preset_time, "{case}: {preset_time:?}");
        }
    }
}

#[test]
fn fill_header_preserves_preset_seq_num_without_incrementing_sender_counter() {
    let mut heartbeat = test_helpers::heartbeat_with_empty_header();
    heartbeat.set_msg_seq_num(42);
    for (case, mut msg) in [
        ("Heartbeat", heartbeat),
        ("NewOrderSingle", test_helpers::new_order_single(42)),
    ] {
        let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
        assert_eq!(msg.msg_seq_num(), 42, "{case}");
        engine.fill_header(&mut msg, &mut storage).unwrap();
        assert_eq!(msg.msg_seq_num(), 42, "{case}");
        assert_eq!(storage.next_sender_msg_seq_num().get(), 1, "{case}");
    }
}

#[test]
fn fill_header_preserves_preset_comp_ids() {
    for (case, mut msg) in messages_with_empty_headers() {
        let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
        msg.set_sender_comp_id(fix_str!("CUSTOM_SENDER").to_owned());
        msg.set_target_comp_id(fix_str!("CUSTOM_TARGET").to_owned());
        engine.fill_header(&mut msg, &mut storage).unwrap();
        assert_eq!(msg.sender_comp_id(), fix_str!("CUSTOM_SENDER"), "{case}");
        assert_eq!(msg.target_comp_id(), fix_str!("CUSTOM_TARGET"), "{case}");
    }
}

#[tokio::test]
async fn commit_send_serializes_to_store_and_produces_stored_output() {
    for (case, mut msg) in messages_with_empty_headers() {
        let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();
        engine.fill_header(&mut msg, &mut storage).unwrap();
        let seq_num = msg.msg_seq_num();
        let Ok(()) = engine.commit_send(msg, &mut storage) else {
            panic!("{case}: commit_send must succeed");
        };

        let pending = engine.take_pending().expect("expected pending output");
        assert_matches!(pending, PendingOutput::Stored(s) if s == nz_seq(seq_num), "{case}");

        let bytes = storage.fetch(nz_seq(seq_num), nz_seq(seq_num)).await;
        assert!(
            bytes.is_ok(),
            "{case}: storage should have message at seq {seq_num}"
        );
    }
}

#[tokio::test]
async fn on_user_send_then_store_for_resend() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();

    let mut msg = test_helpers::new_order_single_with_empty_header();
    engine.fill_header(&mut msg, &mut storage).unwrap();
    let seq_num = msg.msg_seq_num();

    let Ok(()) = engine.store_for_resend(msg, &mut storage) else {
        panic!("store_for_resend must succeed");
    };

    // Should NOT have any pending output
    assert!(engine.take_pending().is_none());

    // But storage should have the serialized bytes
    assert!(
        storage
            .fetch(nz_seq(seq_num), nz_seq(seq_num))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn on_user_send_commit_failure_returns_serialize_failure() {
    // max_message_size too small for a NewOrderSingle with filled header.
    let (mut engine, mut storage) = EngineBuilder::new()
        .max_message_size(limit(140))
        .logged_on()
        .build();

    let mut msg = test_helpers::new_order_single_with_empty_header();
    engine.fill_header(&mut msg, &mut storage).unwrap();
    let seq_num = msg.msg_seq_num();
    assert_eq!(storage.next_sender_msg_seq_num().get(), seq_num + 1);

    let result = engine.commit_send(msg, &mut storage);
    assert!(result.is_err());

    let SendFailure::Serialize(failure) = result.unwrap_err() else {
        panic!("expected serialization failure")
    };
    assert!(matches!(
        failure.error,
        SerializeError::MaxMessageSizeExceeded
    ));

    // Nothing is queued for output: a `Stored(seq_num)` would find no bytes
    // in `flush_output` and go on the wire as a live SequenceReset-GapFill,
    // telling the peer to skip a seq num that was never transmitted.
    assert!(
        engine.take_pending().is_none(),
        "a failed commit must not queue output"
    );

    // The storage must have no bytes for this seq num - no second write
    // was attempted.
    assert!(
        storage
            .fetch(nz_seq(seq_num), nz_seq(seq_num))
            .await
            .is_err()
    );

    // Only a message that is sent consumes a sequence number (FIX Session
    // Layer Section 4.1): the number goes back to the counter, and the stamps are
    // cleared on the message so fixing and re-staging it cannot produce a
    // duplicate number or a stale time.
    assert_eq!(
        storage.next_sender_msg_seq_num().get(),
        seq_num,
        "a failed commit must release its sequence number"
    );
    assert_eq!(failure.msg.msg_seq_num(), 0);
    assert_eq!(failure.msg.sending_time(), UtcTimestamp::MIN_UTC);

    let mut next = test_helpers::heartbeat_with_empty_header();
    assert!(engine.fill_header(&mut next, &mut storage).unwrap());
    assert_eq!(
        next.msg_seq_num(),
        seq_num,
        "the next message takes the number the failed one never used"
    );
}

/// The engine undoes only its own stamps. A number the producer set itself
/// never touched the counter, so there is nothing to release and the number
/// stays on the message - keeping it consistent with the counter is the
/// producer's business, as it was before the failure.
#[test]
fn commit_failure_leaves_a_producer_numbered_message_alone() {
    let (mut engine, mut storage) = EngineBuilder::new()
        .max_message_size(limit(140))
        .logged_on()
        .build();
    let counter_before = storage.next_sender_msg_seq_num();

    let mut msg = test_helpers::new_order_single_with_empty_header();
    msg.set_msg_seq_num(7);
    assert!(engine.fill_header(&mut msg, &mut storage).unwrap());
    assert_eq!(storage.next_sender_msg_seq_num(), counter_before);

    let SendFailure::Serialize(failure) = engine.commit_send(msg, &mut storage).unwrap_err() else {
        panic!("expected serialization failure")
    };

    assert_eq!(storage.next_sender_msg_seq_num(), counter_before);
    assert_eq!(failure.msg.msg_seq_num(), 7);
    assert_eq!(
        failure.msg.sending_time(),
        UtcTimestamp::MIN_UTC,
        "the SendingTime stamp was the engine's and is undone"
    );
}

/// Stamping `MAX - 1` exhausts the numbering. When that commit fails the
/// release puts the counter back where it was - at `MAX - 1` - so the next
/// message can still use the last number. The disconnect latched by the
/// stamping stays: it is the first writer, and the session ends either way.
#[test]
fn commit_failure_at_the_ceiling_restores_the_last_usable_number() {
    let (mut engine, mut storage) = EngineBuilder::new()
        .max_message_size(limit(140))
        .logged_on()
        .build();
    storage
        .set_next_sender_msg_seq_num(nz_seq(SeqNum::MAX - 1))
        .unwrap();

    let mut msg = test_helpers::new_order_single_with_empty_header();
    assert!(engine.fill_header(&mut msg, &mut storage).unwrap());
    assert_eq!(msg.msg_seq_num(), SeqNum::MAX - 1);
    assert_eq!(storage.next_sender_msg_seq_num().get(), SeqNum::MAX);

    let SendFailure::Serialize(failure) = engine.commit_send(msg, &mut storage).unwrap_err() else {
        panic!("expected serialization failure")
    };

    assert_eq!(storage.next_sender_msg_seq_num().get(), SeqNum::MAX - 1);
    assert_eq!(failure.msg.msg_seq_num(), 0);
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::SeqNumExhausted)
    );
}

#[tokio::test]
async fn store_for_resend_commit_failure_does_not_push_output() {
    // Post-disconnect drain: on failure we don't push output (we're
    // disconnected), the storage is left empty, and the seq num goes back to
    // the counter for the next drained message, so what is persisted for
    // resend stays contiguous.
    let (mut engine, mut storage) = EngineBuilder::new()
        .max_message_size(limit(140))
        .logged_on()
        .build();

    let mut msg = test_helpers::new_order_single_with_empty_header();
    engine.fill_header(&mut msg, &mut storage).unwrap();
    let seq_num = msg.msg_seq_num();

    let result = engine.store_for_resend(msg, &mut storage);
    assert!(result.is_err());
    let SendFailure::Serialize(failure) = result.unwrap_err() else {
        panic!("expected serialization failure")
    };

    // No pending output pushed
    assert!(engine.take_pending().is_none());
    // Store has no bytes for this seq num
    assert!(
        storage
            .fetch(nz_seq(seq_num), nz_seq(seq_num))
            .await
            .is_err()
    );
    assert_eq!(storage.next_sender_msg_seq_num().get(), seq_num);
    assert_eq!(failure.msg.msg_seq_num(), 0);
    assert_eq!(failure.msg.sending_time(), UtcTimestamp::MIN_UTC);
}

#[test]
fn on_user_send_allocates_sequential_seq_nums() {
    let (mut engine, mut storage) = EngineBuilder::new().logged_on().build();

    let mut msg1 = test_helpers::new_order_single_with_empty_header();
    engine.fill_header(&mut msg1, &mut storage).unwrap();
    assert_eq!(msg1.msg_seq_num(), 1);

    let mut msg2 = test_helpers::new_order_single_with_empty_header();
    engine.fill_header(&mut msg2, &mut storage).unwrap();
    assert_eq!(msg2.msg_seq_num(), 2);

    let mut msg3 = test_helpers::new_order_single_with_empty_header();
    engine.fill_header(&mut msg3, &mut storage).unwrap();
    assert_eq!(msg3.msg_seq_num(), 3);

    assert_eq!(storage.next_sender_msg_seq_num().get(), 4);
}

#[test]
fn sender_counter_failure_stops_before_serialization() {
    for timing in [FailureTiming::Before, FailureTiming::After] {
        let (mut engine, _) = EngineBuilder::new().logged_on().build();
        let mut storage = FailingStorage::new();
        storage.fail_on(StorageOp::SetSender, 1, timing);
        let mut msg = test_helpers::heartbeat_with_empty_header();
        assert!(engine.fill_header(&mut msg, &mut storage).is_err());
        assert!(engine.has_fatal_error());
        assert_eq!(
            engine.disconnect_reason(),
            Some(DisconnectReason::StorageError)
        );
        assert_eq!(storage.trace.borrow().serialized, 0);
        assert!(engine.take_pending().is_none());
        assert!(engine.fill_header(&mut msg, &mut storage).is_err());
    }
}

#[test]
fn backend_store_failure_never_rolls_back_numbering() {
    for timing in [FailureTiming::Before, FailureTiming::After] {
        for archive_only in [false, true] {
            let (mut engine, _) = EngineBuilder::new().logged_on().build();
            let mut storage = FailingStorage::new();
            let mut msg = test_helpers::heartbeat_with_empty_header();
            assert!(engine.fill_header(&mut msg, &mut storage).unwrap());
            storage.fail_on(StorageOp::Store, 1, timing);
            let result = if archive_only {
                engine.store_for_resend(msg, &mut storage)
            } else {
                engine.commit_send(msg, &mut storage)
            };
            assert_matches!(result, Err(SendFailure::Fatal(_)));
            assert_eq!(storage.sender.get(), 2);
            assert_eq!(storage.trace.borrow().calls.last(), Some(&StorageOp::Store));
            assert!(engine.has_fatal_error());
            assert!(engine.take_pending().is_none());
        }
    }
}

#[test]
fn serialization_rollback_failure_is_fatal_with_or_without_history() {
    for persist_messages in [false, true] {
        for timing in [FailureTiming::Before, FailureTiming::After] {
            let (mut engine, _) = EngineBuilder::new().max_message_size(limit(140)).build();
            engine.session_settings_mut().persist_messages = persist_messages;
            let mut storage = FailingStorage::new();
            storage.max_message_size = 140;
            let mut msg = test_helpers::new_order_single_with_empty_header();
            engine.fill_header(&mut msg, &mut storage).unwrap();
            storage.fail_on(StorageOp::SetSender, 1, timing);
            assert_matches!(
                engine.commit_send(msg, &mut storage),
                Err(SendFailure::Fatal(_))
            );
            assert!(engine.has_fatal_error());
            assert!(engine.take_pending().is_none());
            assert_eq!(
                storage.trace.borrow().calls.last(),
                Some(&StorageOp::SetSender)
            );
        }
    }
}

#[test]
fn disabled_history_serializes_first_send_to_scratch_and_releases_failed_number() {
    let (mut engine, _) = EngineBuilder::new().max_message_size(limit(140)).build();
    engine.session_settings_mut().persist_messages = false;
    let mut storage = FailingStorage::new();
    storage.fail_on(StorageOp::Store, 1, FailureTiming::Before);
    let mut msg = test_helpers::new_order_single_with_empty_header();
    engine.fill_header(&mut msg, &mut storage).unwrap();
    assert_matches!(engine.commit_send(msg, &mut storage), Err(SendFailure::Serialize(failure))
        if failure.msg.msg_seq_num() == 0 && failure.msg.sending_time() == UtcTimestamp::MIN_UTC);
    assert_eq!(storage.sender.get(), 1);
    let mut msg = test_helpers::heartbeat_with_empty_header();
    engine.fill_header(&mut msg, &mut storage).unwrap();
    engine.commit_send(msg, &mut storage).unwrap();
    assert_matches!(engine.take_pending(), Some(PendingOutput::Transient { len }) if len > 0);
    assert_eq!(storage.sender.get(), 2);
    assert!(storage.records.is_empty());
    assert!(!storage.trace.borrow().calls.contains(&StorageOp::Store));
    assert!(!engine.has_fatal_error());
}

#[test]
fn fatal_latch_survives_earlier_disconnect_reason() {
    let (mut engine, _) = EngineBuilder::new().build();
    let mut storage = FailingStorage::new();
    engine.begin_disconnect(DisconnectReason::IoError);
    storage.fail_on(StorageOp::SetSender, 1, FailureTiming::Before);
    let mut msg = test_helpers::heartbeat_with_empty_header();
    assert!(engine.fill_header(&mut msg, &mut storage).is_err());
    assert_eq!(engine.disconnect_reason(), Some(DisconnectReason::IoError));
    assert!(engine.has_fatal_error());
    engine.end_session_if_target_numbering_exhausted(&storage);
}

#[test]
fn fatal_latch_survives_unconfirmed_reset_reason_normalization() {
    let (mut engine, _) = EngineBuilder::new().build();
    let mut storage = FailingStorage::new();
    engine.state.local_reset_unconfirmed = true;
    storage.fail_on(StorageOp::SetSender, 1, FailureTiming::Before);
    let mut msg = test_helpers::heartbeat_with_empty_header();
    assert!(engine.fill_header(&mut msg, &mut storage).is_err());
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::SeqNumResetFailed)
    );
    assert!(engine.has_fatal_error());
    assert!(engine.next_queued_message(&mut storage).is_err());
}
