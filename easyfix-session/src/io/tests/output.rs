use std::{assert_matches, io::ErrorKind, time::Duration};

use tokio::io;

use super::wire::wire_bytes;
use crate::{
    application::DisconnectReason,
    engine::PendingOutput,
    io::{OutputError, flush_output},
    test_helpers::{EngineBuilder, commit_heartbeat, nz_seq},
};

#[tokio::test]
async fn flush_empty_output_returns_false() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    let (mut writer, _reader) = io::duplex(1024);

    let result = flush_output(&mut writer, &mut storage, &mut engine).await;
    assert!(
        !result.expect("flush should succeed"),
        "no bytes should have been written"
    );
}

#[tokio::test]
async fn flush_stored_message() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    let expected_bytes = commit_heartbeat(&mut engine, &mut storage);

    let (mut writer, reader) = io::duplex(8192);

    let result = flush_output(&mut writer, &mut storage, &mut engine).await;
    assert!(
        result.expect("flush should succeed"),
        "bytes should have been written"
    );

    // Verify written bytes
    let buf = wire_bytes(writer, reader).await;
    assert_eq!(buf, expected_bytes);
}

#[tokio::test]
async fn flush_transient_message() {
    let (mut engine, mut storage) = EngineBuilder::new().build();

    // Write known bytes into scratch and push a Transient entry
    let data = b"8=FIXT.1.1\x019=5\x0135=0\x0110=000\x01";
    engine.scratch_mut()[..data.len()].copy_from_slice(data);
    engine.push_pending(PendingOutput::Transient { len: data.len() });

    let (mut writer, reader) = io::duplex(8192);

    let result = flush_output(&mut writer, &mut storage, &mut engine).await;
    assert!(
        result.expect("flush should succeed"),
        "bytes should have been written"
    );

    let buf = wire_bytes(writer, reader).await;
    assert_eq!(buf, data);
}

#[tokio::test]
async fn flush_multiple_pending_in_order() {
    let (mut engine, mut storage) = EngineBuilder::new().build();

    let bytes1 = commit_heartbeat(&mut engine, &mut storage);
    let bytes2 = commit_heartbeat(&mut engine, &mut storage);

    let (mut writer, reader) = io::duplex(16384);

    let result = flush_output(&mut writer, &mut storage, &mut engine).await;
    assert!(result.expect("flush should succeed"));

    let buf = wire_bytes(writer, reader).await;

    let mut expected = bytes1;
    expected.extend_from_slice(&bytes2);
    assert_eq!(buf, expected);
}

#[tokio::test]
async fn missing_stored_message_is_fatal_without_gap_fill() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    engine.push_pending(PendingOutput::Stored(nz_seq(1)));
    let (mut writer, reader) = io::duplex(1024);
    assert_matches!(
        flush_output(&mut writer, &mut storage, &mut engine).await,
        Err(OutputError::Fatal(_))
    );
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::StorageError)
    );
    assert!(wire_bytes(writer, reader).await.is_empty());
}

#[tokio::test]
async fn flush_write_error_on_closed_writer() {
    let (mut engine, mut storage) = EngineBuilder::new().build();
    commit_heartbeat(&mut engine, &mut storage);

    // Create duplex and immediately drop the reader to close the pipe
    let (mut writer, reader) = io::duplex(64);
    drop(reader);

    let result = flush_output(&mut writer, &mut storage, &mut engine).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn flush_write_timeout_on_stalled_writer() {
    let (mut engine, mut storage) = EngineBuilder::new().build();

    // Use a very short write timeout so the test fails fast
    engine.session_settings_mut().write_timeout = Duration::from_millis(100);

    commit_heartbeat(&mut engine, &mut storage);

    // Create a duplex with a tiny buffer and never read from it,
    // so the write will stall
    let (mut writer, _reader) = io::duplex(1);

    // duplex(1) means 1 byte buffer - the message is much larger
    let result = flush_output(&mut writer, &mut storage, &mut engine).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_matches!(err, OutputError::Io(error) if error.kind() == ErrorKind::TimedOut);
}
