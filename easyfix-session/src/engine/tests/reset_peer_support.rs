use easyfix_core::base_messages::AdminBase;
use easyfix_test_messages::Message;

use crate::{
    application::DisconnectReason,
    engine::SessionEngine,
    messages_storage::{InMemoryStorage, MessagesStorage},
    test_helpers,
    test_helpers::{EngineBuilder, as_admin, nz_seq, take_admin},
};

pub(super) fn reset_test_session(
    established: bool,
) -> (SessionEngine<Message>, InMemoryStorage, Vec<u8>) {
    let builder = EngineBuilder::new();
    let (mut engine, mut storage) = if established {
        builder.logged_on().build()
    } else {
        builder.build()
    };
    let history = test_helpers::commit_heartbeat(&mut engine, &mut storage);
    let _ = engine.take_pending();
    storage.set_next_sender_msg_seq_num(nz_seq(40)).unwrap();
    storage.set_next_target_msg_seq_num(nz_seq(40)).unwrap();
    (engine, storage, history)
}

pub(super) async fn assert_reset_refused(
    engine: &mut SessionEngine<Message>,
    storage: &mut InMemoryStorage,
    history: &[u8],
    text: &str,
) {
    assert_eq!(
        engine.disconnect_reason(),
        Some(DisconnectReason::InvalidLogonState)
    );
    assert_eq!(storage.next_sender_msg_seq_num().get(), 40);
    assert_eq!(storage.next_target_msg_seq_num().get(), 40);
    assert_eq!(storage.fetch(nz_seq(1), nz_seq(1)).await.unwrap(), history);
    let logout = take_admin(engine);
    let AdminBase::Logout(logout) = as_admin(&logout) else {
        panic!("expected Logout")
    };
    assert_eq!(logout.text.unwrap().as_ref(), text);
    assert!(engine.take_admin_output().is_none());
    assert!(engine.pending_resends.is_empty());
}
