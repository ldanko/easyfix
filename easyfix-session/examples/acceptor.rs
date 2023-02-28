use std::{collections::HashMap, time::Duration};

use chrono::NaiveTime;
use easyfix_messages::{fields::FixString, messages::Header};
use easyfix_session::{
    acceptor::Acceptor,
    application::{AsEvent, FixEvent},
    messages_storage::InMemoryStorage,
    session_id::SessionId,
    settings::{SessionSettings, Settings},
};
use tokio::{runtime::Builder, task::LocalSet};
use tokio_stream::StreamExt;
use tracing::{error, info};
use tracing_subscriber;

async fn acceptor() {
    let settings = Settings {
        host: "127.0.0.1".parse().unwrap(),
        port: 10050,
        sender_comp_id: "n8_fix_test_server".try_into().unwrap(), //: "easyfix-acceptor".try_into().unwrap(),
        sender_sub_id: None,
        heartbeat_interval: Duration::from_secs(10),
        auto_disconnect_after_no_logon_received: Duration::from_secs(3),
        auto_disconnect_after_no_heartbeat: 3,
    };

    let mut acceptor = Acceptor::new(settings.clone(), Box::new(|| InMemoryStorage::new()));
    let begin_string = FixString::from_ascii_lossy(b"FIXT.1.1".to_vec());
    let sender_comp_id = settings.sender_comp_id.clone();
    let fix_string = |s: &str| FixString::from_ascii_lossy(s.as_bytes().to_vec());
    let session_id = |target_id: &str| {
        SessionId::new(
            begin_string.clone(),
            sender_comp_id.clone(),
            fix_string(target_id),
        )
    };
    let mut register_session = |target_id: &str| {
        let session_id = session_id(target_id);
        acceptor.register_session(
            session_id.clone(),
            SessionSettings {
                session_id,
                // session_time: UtcTimeOnly::from_hms(8, 0, 0)..=UtcTimeOnly::from_hms(16, 0, 0),
                //logon_time: UtcTimeOnly::from_hms(7, 30, 0)..=UtcTimeOnly::from_hms(16, 30, 0),
                session_time: NaiveTime::from_hms(0, 0, 0)..=NaiveTime::from_hms(23, 59, 59),
                logon_time: NaiveTime::from_hms(0, 0, 0)..=NaiveTime::from_hms(23, 59, 59),

                send_redundant_resend_requests: false,
                check_comp_id: true,
                check_latency: true,
                max_latency: Duration::from_secs(120),

                reset_on_logon: false,
                reset_on_logout: false,
                reset_on_disconnect: false,
                sender_default_appl_ver_id: FixString::from_ascii_lossy(b"9".to_vec()),
                target_default_appl_ver_id: FixString::from_ascii_lossy(b"9".to_vec()),
                persist: false,
                refresh_on_logon: false,
                enable_next_expected_msg_seq_num: true,
            },
        );
    };

    register_session("client_1");
    register_session("client_2");
    register_session("client_3");
    register_session("16");

    let mut senders = HashMap::new();
    acceptor.start();
    while let Some(mut entry) = acceptor.next().await {
        match entry.as_event() {
            FixEvent::Created(session_id) => info!("Session created: {}", session_id),
            FixEvent::Logon(session_id, sender) => {
                info!("Logon: {}", session_id);
                senders.insert(session_id.clone(), sender);
            }
            FixEvent::Logout(session_id) => {
                info!("Logout: {}", session_id);
                senders.remove(session_id);
            }
            FixEvent::AppMsgIn(mut msg) => {
                info!("App input msg: {:?}", msg.msg_type());
                let session_id = SessionId::from_input_msg(&msg);
                reverse_route(&mut msg.header);
                senders.get(&session_id).unwrap().send(msg).await;
            }
            FixEvent::AdmMsgIn(msg) => info!("Adm input msg: {:?}", msg.msg_type()),
            FixEvent::AppMsgOut(msg, _responder) => {
                info!("App output msg: {:?}", msg.msg_type());
                _responder.do_not_send();
            }
            FixEvent::AdmMsgOut(msg) => info!("Adm output msg: {:?}", msg.msg_type()),
            FixEvent::DeserializeError(session_id, error) => {
                error!("{session_id}: {error}");
            }
        }
        // info!("{:?}", entry.as_event());
    }
    // if let Err(e) = acceptor.run().await {
    //     error!("Error: {e}");
    // }
}

fn reverse_route(header: &mut Header) {
    std::mem::swap(&mut header.sender_comp_id, &mut header.target_comp_id);
    std::mem::swap(&mut header.sender_sub_id, &mut header.target_sub_id);
}

fn main() {
    // install global collector configured based on RUST_LOG env var.
    tracing_subscriber::fmt::init();
    // tracing_subscriber::fmt()
    //     .with_max_level(Level::TRACE)
    //     .json()
    //     .init();
    // tracing_subscriber::fmt()
    //     .with_max_level(Level::TRACE)
    //     .init();

    info!("Hello World!");

    let runtime = Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .unwrap();

    let local_set = LocalSet::new();
    local_set.block_on(&runtime, acceptor());

    //let runtime_mt = Builder::new_multi_thread()
    //    .enable_io()
    //    .enable_time()
    //    .build()
    //    .unwrap();

    //let end = runtime_mt.spawn_local(acceptor());

    //runtime_mt.block_on(async { end.await.unwrap() });
}
