use std::assert_matches;

use easyfix_core::basic_types::SeqNum;

use super::*;
use crate::test_helpers::{DEFAULT_MAX_MESSAGE_SIZE_BYTES, nz_seq};

fn store_bytes(storage: &mut InMemoryStorage, seq_num: u32, bytes: &[u8]) {
    storage
        .store(nz_seq(seq_num), |buf| {
            assert!(buf.len() >= DEFAULT_MAX_MESSAGE_SIZE_BYTES);
            buf[..bytes.len()].copy_from_slice(bytes);
            Ok(bytes.len())
        })
        .unwrap();
}

#[tokio::test]
async fn stores_exact_prefix_and_preserves_records_in_any_read_order() {
    let mut storage = InMemoryStorage::new(DEFAULT_MAX_MESSAGE_SIZE_BYTES);
    store_bytes(&mut storage, 5, b"opaque\x00\xffbytes");
    store_bytes(&mut storage, 1, b"one");
    store_bytes(&mut storage, 8, b"");
    for (seq, bytes) in [
        (5, b"opaque\x00\xffbytes".as_slice()),
        (1, b"one"),
        (5, b"opaque\x00\xffbytes"),
        (8, b""),
    ] {
        assert_eq!(storage.fetch(nz_seq(seq), nz_seq(8)).await.unwrap(), bytes);
    }
}

#[tokio::test]
async fn serialization_error_preserves_history_and_publishes_nothing() {
    let mut storage = InMemoryStorage::new(DEFAULT_MAX_MESSAGE_SIZE_BYTES);
    store_bytes(&mut storage, 1, b"original");
    let result = storage.store(nz_seq(2), |buf| {
        buf.fill(b'x');
        Err(SerializeError::InvalidValue)
    });
    assert_matches!(
        result,
        Err(StoreError::Serialize(SerializeError::InvalidValue))
    );
    assert_eq!(
        storage.fetch(nz_seq(1), nz_seq(2)).await.unwrap(),
        b"original"
    );
    assert_eq!(
        storage.fetch(nz_seq(2), nz_seq(2)).await,
        Err(InMemoryStorageError::MissingMessage(nz_seq(2)))
    );
    store_bytes(&mut storage, 2, b"retry");
    assert_eq!(storage.fetch(nz_seq(2), nz_seq(2)).await.unwrap(), b"retry");
}

#[tokio::test]
async fn occupied_key_rejects_without_serializing_or_changing_original() {
    let mut storage = InMemoryStorage::new(DEFAULT_MAX_MESSAGE_SIZE_BYTES);
    store_bytes(&mut storage, 1, b"original");
    let result = storage.store(nz_seq(1), |_| panic!("occupied key must be rejected first"));
    assert_matches!(result, Err(StoreError::Backend(InMemoryStorageError::DuplicateSequenceNumber(seq))) if seq == nz_seq(1));
    assert_eq!(
        storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
        b"original"
    );
}

#[tokio::test]
async fn counters_are_independent_and_reset_clears_history() {
    let mut storage = InMemoryStorage::new(DEFAULT_MAX_MESSAGE_SIZE_BYTES);
    assert_eq!(storage.next_sender_msg_seq_num().get(), 1);
    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
    storage
        .set_next_sender_msg_seq_num(nz_seq(SeqNum::MAX))
        .unwrap();
    assert_eq!(storage.next_sender_msg_seq_num().get(), SeqNum::MAX);
    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
    storage.set_next_target_msg_seq_num(nz_seq(9)).unwrap();
    assert_eq!(storage.next_sender_msg_seq_num().get(), SeqNum::MAX);
    assert_eq!(storage.next_target_msg_seq_num().get(), 9);
    store_bytes(&mut storage, 1, b"history");
    storage.reset().unwrap();
    assert_eq!(storage.next_sender_msg_seq_num().get(), 1);
    assert_eq!(storage.next_target_msg_seq_num().get(), 1);
    assert_eq!(
        storage.fetch(nz_seq(1), nz_seq(1)).await,
        Err(InMemoryStorageError::MissingMessage(nz_seq(1)))
    );
    store_bytes(&mut storage, 1, b"new session");
    assert_eq!(
        storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(),
        b"new session"
    );
}
