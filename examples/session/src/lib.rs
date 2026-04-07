//! Shared code for the acceptor and initiator example binaries.
//!
//! Both binaries use the same generated FIX message types, settings
//! builders and message builders. This module exposes them so each
//! `src/bin/*.rs` holds only what is role-specific: its own
//! [`Application`](easyfix_session::Application) implementation and the
//! handful of setup lines around it.

use std::{fmt::Write, num::NonZeroU8, time::Duration};

use easyfix_session::{
    SessionId, SessionMessage, SessionSettings, Version,
    basic_types::{Decimal, FixStr, FixString, TimePrecision, UtcTimestamp},
    fix_str,
};
use tokio::{runtime, task};
use tracing_subscriber::fmt;

// ---- Generated FIX message types -----------------------------------------

pub mod messages {
    include!(concat!(env!("OUT_DIR"), "/messages.rs"));
}

pub use messages::{
    Body, BusinessMessageReject, BusinessRejectReason, ExecutionReport, Header, Message,
    NewOrderSingle, Trailer,
};

// ---- Settings builders ---------------------------------------------------

pub fn build_session_settings() -> SessionSettings {
    SessionSettings {
        auto_disconnect_after_no_heartbeat: const { NonZeroU8::new(3).unwrap() },
        auto_disconnect_after_no_logout: Duration::from_secs(5),
        enable_next_expected_msg_seq_num: true,
        ..SessionSettings::default()
    }
}

// ---- Session ID helpers --------------------------------------------------

pub fn create_session_id(sender: &FixStr, target: &FixStr) -> SessionId {
    SessionId::new(Version::FIXT11, sender.to_owned(), target.to_owned())
}

// ---- Message builders ----------------------------------------------------

/// Build a `NewOrderSingle` with the given `ClOrdID`. Header fields are
/// left defaulted - the engine stamps them on the way out.
///
/// `TransactTime<60>` is a body field, so its width is ours to pick; passing
/// the session's own `time_precision` keeps the whole message consistent.
pub fn build_new_order_single(cl_ord_id: &FixStr, time_precision: TimePrecision) -> Box<Message> {
    Box::new(Message {
        header: Header::default(),
        body: Box::new(Body::NewOrderSingle(NewOrderSingle {
            cl_ord_id: cl_ord_id.to_owned(),
            symbol: fix_str!("SYMBOL1").to_owned(),
            transact_time: UtcTimestamp::now(time_precision),
            order_qty: Decimal::ONE_HUNDRED,
            ..NewOrderSingle::default()
        })),
        trailer: Trailer::default(),
    })
}

/// Build an `ExecutionReport` acknowledging a `NewOrderSingle` as a
/// fully-filled order.
pub fn build_exec_report_for(order: &NewOrderSingle, exec_id: u64) -> Box<Message> {
    let mut exec_id_str = String::new();
    let _ = write!(&mut exec_id_str, "EXEC{exec_id:010}");

    Box::new(Message {
        header: Header::default(),
        body: Box::new(Body::ExecutionReport(ExecutionReport {
            order_id: order.cl_ord_id.clone(),
            exec_id: FixString::from_ascii_lossy(exec_id_str.into_bytes()),
            exec_type: messages::ExecType::Trade,
            ord_status: messages::OrdStatus::Filled,
            symbol: order.symbol.clone(),
            side: order.side,
            leaves_qty: Decimal::ZERO,
            cum_qty: order.order_qty,
            cl_ord_id: Some(order.cl_ord_id.clone()),
            order_qty: Some(order.order_qty),
            price: order.price,
            transact_time: Some(order.transact_time),
        })),
        trailer: Trailer::default(),
    })
}

/// Build a `BusinessMessageReject` refusing an application message whose
/// `MsgType` is valid in the dictionary but unsupported by this application.
///
/// `RefSeqNum<45>` and `RefMsgType<372>` point back at the rejected message;
/// the reply carries its own sequence number, so the two must not be confused.
pub fn build_business_message_reject_for(rejected: &Message) -> Box<Message> {
    Box::new(Message {
        header: Header::default(),
        body: Box::new(Body::BusinessMessageReject(BusinessMessageReject {
            ref_seq_num: Some(rejected.header.msg_seq_num),
            ref_msg_type: SessionMessage::msg_type(rejected).as_fix_str().to_owned(),
            business_reject_reason: BusinessRejectReason::UnsupportedMessageType,
            text: Some(fix_str!("message type not supported").to_owned()),
        })),
        trailer: Trailer::default(),
    })
}

// ---- Runtime boilerplate -------------------------------------------------

/// Install `tracing_subscriber`, build a single-threaded tokio runtime
/// with a `LocalSet`, and block on the given future.
pub fn run_on_local_set(f: impl Future<Output = ()>) {
    fmt::init();

    let runtime = runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .expect("build tokio runtime");

    let local_set = task::LocalSet::new();
    local_set.block_on(&runtime, f);
}
