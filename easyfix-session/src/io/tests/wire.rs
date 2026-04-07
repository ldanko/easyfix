use std::assert_matches;

use chrono::Utc;
use easyfix_core::{
    base_messages::MsgTypeBase,
    basic_types::{Int, SeqNum},
    fix_str,
    message::SessionMessage,
};
use easyfix_test_messages::Message;
use tokio::{
    io,
    io::{AsyncRead, AsyncReadExt, DuplexStream},
    sync::mpsc,
};

use super::harness::TestEvent;
use crate::{test_helpers, test_helpers::read_one_message};

/// Close the writing end and collect everything the reading end still holds:
/// the bytes a test put on its duplex wire.
pub(super) async fn wire_bytes(writer: io::DuplexStream, mut reader: io::DuplexStream) -> Vec<u8> {
    drop(writer);
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).await.unwrap();
    bytes
}

pub(super) fn build_peer_logon(seq: SeqNum, heart_bt_int: Int) -> Box<Message> {
    test_helpers::logon_with_options(
        seq,
        fix_str!("TARGET"),
        fix_str!("SENDER"),
        heart_bt_int,
        None,
        None,
    )
}

/// A well-framed `NewOrderSingle<D>` whose header decodes but whose body
/// does not - `Symbol(55)` is missing - so the session answers with a
/// `Reject<3>` (`SessionRejectReason(373)=1`) and carries on.
pub(super) fn build_order_missing_symbol_bytes(seq: SeqNum) -> Vec<u8> {
    let now = Utc::now().format("%Y%m%d-%H:%M:%S%.3f");
    test_helpers::frame_message(
        "FIXT.1.1",
        &format!(
            "35=D|49=TARGET|56=SENDER|34={seq}|52={now}|11=ORD{seq}|54=1|60={now}|38=10|40=1|"
        ),
    )
}

/// Read exactly one FIX message, discarding any bytes past it - fine
/// only when messages never coalesce on the wire. Use
/// [`read_one_message`] directly when they can.
pub(super) async fn read_lone_message<R: AsyncRead + Unpin>(reader: &mut R) -> Box<Message> {
    read_one_message(reader, &mut Vec::new()).await
}

/// Drive the standard acceptor logon prelude to completion: the
/// AdminMsgIn(Logon) callback, the Logon response on the wire, then
/// SessionReady. Pure setup for tests that exercise an established
/// session; `acceptor_logon_handshake` keeps the sequence inline as
/// its verdict.
pub(super) async fn logon_handshake(
    client_io: &mut DuplexStream,
    events_rx: &mut mpsc::UnboundedReceiver<TestEvent>,
) {
    assert_matches!(
        events_rx.recv().await.unwrap(),
        TestEvent::AdminMsgIn(MsgTypeBase::Logon)
    );
    let response = read_lone_message(client_io).await;
    assert_eq!(SessionMessage::msg_type(&*response), MsgTypeBase::Logon);
    assert_matches!(events_rx.recv().await.unwrap(), TestEvent::SessionReady);
}
