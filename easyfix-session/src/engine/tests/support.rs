use easyfix_core::{
    base_messages::MsgTypeBase, basic_types::NonZeroLength, message::SessionMessage,
};
use easyfix_test_messages::Message;

use crate::{
    engine::{SessionEngine, VerifyError},
    messages_storage::MessagesStorage,
};

/// Helper: check msg_type via the SessionMessage trait (returns MsgTypeField
/// which implements PartialEq<MsgTypeBase>). The concrete Message type has an
/// inherent msg_type() returning MsgType - we need the trait version.
pub(super) fn assert_msg_type(msg: &Message, expected: MsgTypeBase) {
    assert_eq!(SessionMessage::msg_type(msg), expected);
}

/// Test helper: a `max_message_size` setting value.
pub(super) fn limit(size: u16) -> NonZeroLength {
    NonZeroLength::new(size).unwrap()
}

/// Test helper: borrow `msg`'s header + msg_type and call
/// [`SessionEngine::verify_header`] with them (`reset_pending` always
/// `false`). Lets verify_header tests express their intent against
/// `&Box<Message>` without repeating the borrow boilerplate.
pub(super) fn verify_test(
    engine: &SessionEngine<Message>,
    msg: &Message,
    storage: &impl MessagesStorage,
    check_too_high: bool,
    check_too_low: bool,
) -> Result<(), VerifyError> {
    let header = msg.header();
    // SessionMessage::msg_type returns MsgTypeField; the concrete
    // generated `Message` type's inherent msg_type() returns the
    // generated MsgType enum and shadows the trait method - disambiguate
    // explicitly here.
    engine.verify_header(
        &header,
        SessionMessage::msg_type(msg),
        storage,
        check_too_high,
        check_too_low,
        false,
    )
}
