//! Generate `From` conversions between admin base messages (easyfix-core)
//! and generated admin message structs.
//!
//! For each admin message, two conversions are generated:
//! - Incoming: `From<&'a Generated> for Base<'a>` — zero-copy via `Cow::Borrowed`
//! - Outgoing: `From<Base<'_>> for Generated` — consumes `Cow`, defaults remaining fields
//!
//! Dual-representation enum fields (typed + raw in base, single enum in generated):
//! - Incoming: typed field gets `Default`, raw field gets the Int parsed from enum bytes
//! - Outgoing: generated enum is built from the typed base field via `From`

use std::collections::HashMap;

use easyfix_dictionary::{BasicType, Version};
use proc_macro2::TokenStream;
use quote::quote;

use super::{member::Member, message::MessageCodeGen};

/// Generate `From` conversions for all admin messages.
pub fn generate_admin_base_conversions(
    messages: &[MessageCodeGen],
    version: Version,
) -> TokenStream {
    let find = |name: &str| -> &[Member] {
        messages
            .iter()
            .find(|m| m.name() == name)
            .unwrap_or_else(|| panic!("Admin message '{name}' not found in dictionary"))
            .body_members()
    };

    let heartbeat = generate_heartbeat(find("Heartbeat"));
    let test_request = generate_test_request(find("TestRequest"));
    let resend_request = generate_resend_request(find("ResendRequest"));
    let sequence_reset = generate_sequence_reset(find("SequenceReset"));
    let logout = generate_logout(find("Logout"), version);
    let reject = generate_reject(find("Reject"), version);
    let logon = generate_logon(find("Logon"), version);

    quote! {
        #heartbeat
        #test_request
        #resend_request
        #sequence_reset
        #logout
        #reject
        #logon

        impl From<AdminBase<'_>> for Body {
            fn from(admin: AdminBase<'_>) -> Self {
                match admin {
                    AdminBase::Logon(base) => Body::Logon(base.into()),
                    AdminBase::Logout(base) => Body::Logout(base.into()),
                    AdminBase::Heartbeat(base) => Body::Heartbeat(base.into()),
                    AdminBase::TestRequest(base) => Body::TestRequest(base.into()),
                    AdminBase::ResendRequest(base) => Body::ResendRequest(base.into()),
                    AdminBase::SequenceReset(base) => Body::SequenceReset(base.into()),
                    AdminBase::Reject(base) => Body::Reject(base.into()),
                }
            }
        }
    }
}

/// Generate `try_as_admin_base` method body for the `Message` impl block.
pub fn generate_admin_base_dispatch() -> TokenStream {
    quote! {
        pub fn try_as_admin_base(&self) -> Option<AdminBase<'_>> {
            match self {
                Body::Logon(msg) => Some(AdminBase::Logon(msg.into())),
                Body::Logout(msg) => Some(AdminBase::Logout(msg.into())),
                Body::Heartbeat(msg) => Some(AdminBase::Heartbeat(msg.into())),
                Body::TestRequest(msg) => Some(AdminBase::TestRequest(msg.into())),
                Body::ResendRequest(msg) => Some(AdminBase::ResendRequest(msg.into())),
                Body::SequenceReset(msg) => Some(AdminBase::SequenceReset(msg.into())),
                Body::Reject(msg) => Some(AdminBase::Reject(msg.into())),
                _ => None,
            }
        }
    }
}

fn tag_map(members: &[Member]) -> HashMap<u16, &Member> {
    members.iter().map(|m| (m.tag_num(), m)).collect()
}

fn validate_tag(map: &HashMap<u16, &Member>, tag: u16, name: &str, expected: BasicType) {
    let member = map.get(&tag).unwrap_or_else(|| {
        panic!("Admin message missing tag {tag} ({name}) needed for base conversion")
    });
    assert!(
        member.has_basic_type(expected),
        "Admin message tag {tag} ({name}) has unexpected type, expected {expected:?}",
    );
}

// ---------------------------------------------------------------------------
// HeartbeatBase ↔ Heartbeat
// Fields: test_req_id (tag 112, String, optional)
// ---------------------------------------------------------------------------
fn generate_heartbeat(members: &[Member]) -> TokenStream {
    let map = tag_map(members);
    validate_tag(&map, 112, "TestReqID", BasicType::String);

    let extra_fields_default = if members.len() > 1 {
        quote! { ..Default::default() }
    } else {
        quote! {}
    };

    quote! {
        impl<'a> From<&'a Heartbeat> for HeartbeatBase<'a> {
            fn from(msg: &'a Heartbeat) -> Self {
                HeartbeatBase {
                    test_req_id: msg.test_req_id.as_deref().map(Cow::Borrowed),
                }
            }
        }

        impl From<HeartbeatBase<'_>> for Heartbeat {
            fn from(base: HeartbeatBase<'_>) -> Heartbeat {
                Heartbeat {
                    test_req_id: base.test_req_id.map(Cow::into_owned),
                    #extra_fields_default
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TestRequestBase ↔ TestRequest
// Fields: test_req_id (tag 112, String, required)
// ---------------------------------------------------------------------------
fn generate_test_request(members: &[Member]) -> TokenStream {
    let map = tag_map(members);
    validate_tag(&map, 112, "TestReqID", BasicType::String);

    let extra_fields_default = if members.len() > 1 {
        quote! { ..Default::default() }
    } else {
        quote! {}
    };

    quote! {
        impl<'a> From<&'a TestRequest> for TestRequestBase<'a> {
            fn from(msg: &'a TestRequest) -> Self {
                TestRequestBase {
                    test_req_id: Cow::Borrowed(&msg.test_req_id),
                }
            }
        }

        impl From<TestRequestBase<'_>> for TestRequest {
            fn from(base: TestRequestBase<'_>) -> TestRequest {
                TestRequest {
                    test_req_id: base.test_req_id.into_owned(),
                    #extra_fields_default
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ResendRequestBase ↔ ResendRequest
// Fields: begin_seq_no (tag 7, SeqNum), end_seq_no (tag 16, SeqNum)
// ---------------------------------------------------------------------------
fn generate_resend_request(members: &[Member]) -> TokenStream {
    let map = tag_map(members);
    validate_tag(&map, 7, "BeginSeqNo", BasicType::SeqNum);
    validate_tag(&map, 16, "EndSeqNo", BasicType::SeqNum);

    let extra_fields_default = if members.len() > 2 {
        quote! { ..Default::default() }
    } else {
        quote! {}
    };

    quote! {
        impl From<&ResendRequest> for ResendRequestBase {
            fn from(msg: &ResendRequest) -> Self {
                ResendRequestBase {
                    begin_seq_no: msg.begin_seq_no,
                    end_seq_no: msg.end_seq_no,
                }
            }
        }

        impl From<ResendRequestBase> for ResendRequest {
            fn from(base: ResendRequestBase) -> ResendRequest {
                ResendRequest {
                    begin_seq_no: base.begin_seq_no,
                    end_seq_no: base.end_seq_no,
                    #extra_fields_default
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SequenceResetBase ↔ SequenceReset
// Fields: gap_fill_flag (tag 123, Boolean, optional), new_seq_no (tag 36, SeqNum)
// ---------------------------------------------------------------------------
fn generate_sequence_reset(members: &[Member]) -> TokenStream {
    let map = tag_map(members);
    validate_tag(&map, 123, "GapFillFlag", BasicType::Boolean);
    validate_tag(&map, 36, "NewSeqNo", BasicType::SeqNum);

    let extra_fields_default = if members.len() > 2 {
        quote! { ..Default::default() }
    } else {
        quote! {}
    };

    quote! {
        impl From<&SequenceReset> for SequenceResetBase {
            fn from(msg: &SequenceReset) -> Self {
                SequenceResetBase {
                    gap_fill_flag: msg.gap_fill_flag,
                    new_seq_no: msg.new_seq_no,
                }
            }
        }

        impl From<SequenceResetBase> for SequenceReset {
            fn from(base: SequenceResetBase) -> SequenceReset {
                SequenceReset {
                    gap_fill_flag: base.gap_fill_flag,
                    new_seq_no: base.new_seq_no,
                    #extra_fields_default
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// LogoutBase ↔ Logout
// Fields:
//   session_status / session_status_raw (tag 1409, Int enum, optional, FIX 5.0SP1+)
//   text                                (tag 58,   String,   optional)
// ---------------------------------------------------------------------------
fn generate_logout(members: &[Member], version: Version) -> TokenStream {
    let map = tag_map(members);
    validate_tag(&map, 58, "Text", BasicType::String);

    let has_session_status = map.contains_key(&1409);
    if has_session_status {
        validate_tag(&map, 1409, "SessionStatus", BasicType::Int);
    }
    if has_session_status && version < Version::FIX50SP1 {
        panic!("SessionStatus (tag 1409) in Logout is not valid before FIX 5.0SP1");
    }

    // --- Incoming: session_status as newtype field ---
    let incoming_session_status = if has_session_status {
        quote! {
            msg.session_status.as_ref().map(|v| SessionStatusField::from(*v))
        }
    } else {
        quote! { None }
    };

    // --- Outgoing: session_status from newtype via From ---
    let outgoing_session_status = if has_session_status {
        quote! {
            session_status: base.session_status.map(fields::SessionStatus::from),
        }
    } else {
        quote! {}
    };

    let base_field_count = 1 + has_session_status as usize;
    let extra_fields_default = if members.len() > base_field_count {
        quote! { ..Default::default() }
    } else {
        quote! {}
    };

    quote! {
        impl<'a> From<&'a Logout> for LogoutBase<'a> {
            fn from(msg: &'a Logout) -> Self {
                LogoutBase {
                    session_status: #incoming_session_status,
                    text: msg.text.as_deref().map(Cow::Borrowed),
                }
            }
        }

        impl From<LogoutBase<'_>> for Logout {
            fn from(base: LogoutBase<'_>) -> Logout {
                Logout {
                    #outgoing_session_status
                    text: base.text.map(Cow::into_owned),
                    #extra_fields_default
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RejectBase ↔ Reject
// Fields:
//   ref_seq_num       (tag 45,  SeqNum, required)
//   ref_tag_id        (tag 371, Int,    optional, FIX 4.2+)
//   ref_msg_type      (tag 372, String, optional, FIX 4.2+)
//   session_reject_reason / session_reject_reason_raw (tag 373, Int enum, optional, FIX 4.2+)
//   text              (tag 58,  String, optional)
// ---------------------------------------------------------------------------
fn generate_reject(members: &[Member], version: Version) -> TokenStream {
    let map = tag_map(members);

    // Always-present fields
    validate_tag(&map, 45, "RefSeqNum", BasicType::SeqNum);
    validate_tag(&map, 58, "Text", BasicType::String);

    // Version-conditional fields (FIX 4.2+) — validate only if present
    let has_ref_tag_id = map.contains_key(&371);
    if has_ref_tag_id {
        validate_tag(&map, 371, "RefTagID", BasicType::Int);
    }
    let has_ref_msg_type = map.contains_key(&372);
    if has_ref_msg_type {
        validate_tag(&map, 372, "RefMsgType", BasicType::String);
    }
    let has_session_reject_reason = map.contains_key(&373);
    if has_session_reject_reason {
        validate_tag(&map, 373, "SessionRejectReason", BasicType::Int);
    }

    // Sanity: version-conditional field presence
    if (has_ref_tag_id || has_ref_msg_type || has_session_reject_reason) && version < Version::FIX42
    {
        panic!(
            "RefTagID/RefMsgType/SessionRejectReason (tags 371/372/373) are not valid before FIX 4.2"
        );
    }

    // --- Incoming: conditional fields ---
    let incoming_ref_tag_id = if has_ref_tag_id {
        quote! { msg.ref_tag_id }
    } else {
        quote! { None }
    };

    let incoming_ref_msg_type = if has_ref_msg_type {
        quote! { msg.ref_msg_type.as_deref().map(Cow::Borrowed) }
    } else {
        quote! { None }
    };

    let incoming_session_reject_reason = if has_session_reject_reason {
        quote! {
            msg.session_reject_reason.as_ref()
                .map(|v| SessionRejectReasonField::from(*v))
        }
    } else {
        quote! { None }
    };

    // --- Outgoing: conditional fields ---
    let outgoing_ref_tag_id = if has_ref_tag_id {
        quote! { ref_tag_id: base.ref_tag_id, }
    } else {
        quote! {}
    };

    let outgoing_ref_msg_type = if has_ref_msg_type {
        quote! { ref_msg_type: base.ref_msg_type.map(Cow::into_owned), }
    } else {
        quote! {}
    };

    let outgoing_session_reject_reason = if has_session_reject_reason {
        quote! {
            session_reject_reason: base.session_reject_reason
                .map(fields::SessionRejectReason::from),
        }
    } else {
        quote! {}
    };

    let base_field_count = 2
        + has_ref_tag_id as usize
        + has_ref_msg_type as usize
        + has_session_reject_reason as usize;
    let extra_fields_default = if members.len() > base_field_count {
        quote! { ..Default::default() }
    } else {
        quote! {}
    };

    quote! {
        impl<'a> From<&'a Reject> for RejectBase<'a> {
            fn from(msg: &'a Reject) -> Self {
                RejectBase {
                    ref_seq_num: msg.ref_seq_num,
                    ref_tag_id: #incoming_ref_tag_id,
                    ref_msg_type: #incoming_ref_msg_type,
                    session_reject_reason: #incoming_session_reject_reason,
                    text: msg.text.as_deref().map(Cow::Borrowed),
                }
            }
        }

        impl From<RejectBase<'_>> for Reject {
            fn from(base: RejectBase<'_>) -> Reject {
                Reject {
                    ref_seq_num: base.ref_seq_num,
                    #outgoing_ref_tag_id
                    #outgoing_ref_msg_type
                    #outgoing_session_reject_reason
                    text: base.text.map(Cow::into_owned),
                    #extra_fields_default
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// LogonBase ↔ Logon
// Fields:
//   encrypt_method / encrypt_method_raw (tag 98,   Int enum,  required)
//   heart_bt_int                        (tag 108,  Int,       required)
//   reset_seq_num_flag                  (tag 141,  Boolean,   optional, FIX 4.1+)
//   next_expected_msg_seq_num           (tag 789,  SeqNum,    optional, FIX 4.4+)
//   default_appl_ver_id                 (tag 1137, String enum, FIXT 1.1)
//   session_status / session_status_raw (tag 1409, Int enum,  optional, FIX 5.0SP1+)
// ---------------------------------------------------------------------------
fn generate_logon(members: &[Member], version: Version) -> TokenStream {
    let map = tag_map(members);

    // Always-present fields
    validate_tag(&map, 98, "EncryptMethod", BasicType::Int);
    validate_tag(&map, 108, "HeartBtInt", BasicType::Int);

    // Version-conditional fields — validate only if present
    let has_reset_seq_num_flag = map.contains_key(&141);
    if has_reset_seq_num_flag {
        validate_tag(&map, 141, "ResetSeqNumFlag", BasicType::Boolean);
    }
    let has_next_expected = map.contains_key(&789);
    if has_next_expected {
        validate_tag(&map, 789, "NextExpectedMsgSeqNum", BasicType::SeqNum);
    }
    let has_default_appl_ver_id = map.contains_key(&1137);
    if has_default_appl_ver_id {
        validate_tag(&map, 1137, "DefaultApplVerID", BasicType::String);
    }
    let has_session_status = map.contains_key(&1409);
    if has_session_status {
        validate_tag(&map, 1409, "SessionStatus", BasicType::Int);
    }

    // Sanity: version-conditional field presence
    if has_reset_seq_num_flag && version < Version::FIX41 {
        panic!("ResetSeqNumFlag (tag 141) is not valid before FIX 4.1");
    }
    if has_next_expected && version < Version::FIX44 {
        panic!("NextExpectedMsgSeqNum (tag 789) is not valid before FIX 4.4");
    }
    if has_session_status && version < Version::FIX50SP1 {
        panic!("SessionStatus (tag 1409) is not valid before FIX 5.0SP1");
    }
    if version == Version::FIXT11 {
        assert!(
            has_default_appl_ver_id,
            "FIXT 1.1 Logon must have DefaultApplVerID (tag 1137)"
        );
    }

    // --- Incoming: conditional fields ---
    let incoming_reset_seq_num_flag = if has_reset_seq_num_flag {
        quote! { msg.reset_seq_num_flag }
    } else {
        quote! { None }
    };

    let incoming_next_expected = if has_next_expected {
        quote! { msg.next_expected_msg_seq_num }
    } else {
        quote! { None }
    };

    let incoming_default_appl_ver_id = if has_default_appl_ver_id {
        quote! { Some(Cow::Borrowed(msg.default_appl_ver_id.as_fix_str())) }
    } else {
        quote! { None }
    };

    let incoming_session_status = if has_session_status {
        quote! {
            msg.session_status.as_ref().map(|v| SessionStatusField::from(*v))
        }
    } else {
        quote! { None }
    };

    // --- Outgoing: conditional fields ---
    let outgoing_reset_seq_num_flag = if has_reset_seq_num_flag {
        quote! { reset_seq_num_flag: base.reset_seq_num_flag, }
    } else {
        quote! {}
    };

    let outgoing_next_expected = if has_next_expected {
        quote! { next_expected_msg_seq_num: base.next_expected_msg_seq_num, }
    } else {
        quote! {}
    };

    let outgoing_default_appl_ver_id = if has_default_appl_ver_id {
        quote! {
            default_appl_ver_id: base.default_appl_ver_id.map(|v| {
                fields::DefaultApplVerId::from_fix_str(&v)
                    .expect("LogonBase default_appl_ver_id must be a valid DefaultApplVerId")
            }).unwrap_or_default(),
        }
    } else {
        quote! {}
    };

    let outgoing_session_status = if has_session_status {
        quote! {
            session_status: base.session_status.map(fields::SessionStatus::from),
        }
    } else {
        quote! {}
    };

    let base_field_count = 2
        + has_reset_seq_num_flag as usize
        + has_next_expected as usize
        + has_default_appl_ver_id as usize
        + has_session_status as usize;
    let extra_fields_default = if members.len() > base_field_count {
        quote! { ..Default::default() }
    } else {
        quote! {}
    };

    quote! {
        impl<'a> From<&'a Logon> for LogonBase<'a> {
            fn from(msg: &'a Logon) -> Self {
                LogonBase {
                    encrypt_method: Default::default(),
                    encrypt_method_raw: msg.encrypt_method.as_int(),
                    heart_bt_int: msg.heart_bt_int,
                    reset_seq_num_flag: #incoming_reset_seq_num_flag,
                    next_expected_msg_seq_num: #incoming_next_expected,
                    default_appl_ver_id: #incoming_default_appl_ver_id,
                    session_status: #incoming_session_status,
                }
            }
        }

        impl From<LogonBase<'_>> for Logon {
            fn from(base: LogonBase<'_>) -> Logon {
                Logon {
                    encrypt_method: fields::EncryptMethod::from(base.encrypt_method),
                    heart_bt_int: base.heart_bt_int,
                    #outgoing_reset_seq_num_flag
                    #outgoing_next_expected
                    #outgoing_default_appl_ver_id
                    #outgoing_session_status
                    #extra_fields_default
                }
            }
        }
    }
}
