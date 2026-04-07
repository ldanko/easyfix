use std::{
    assert_matches,
    time::{Duration, Instant},
};

use futures_util::FutureExt;
use tokio::time;

use super::{SessionTimers, TimerEvent};
use crate::{
    io::{
        ControlMsg,
        time::{Sleep, TimerBackend},
    },
    test_helpers::EngineBuilder,
};

#[tokio::test(start_paused = true)]
async fn busywait_engine_and_timers_share_the_wall_clock() {
    time::advance(Duration::from_secs(3600)).await;
    let before = Instant::now();
    let (mut engine, _) = EngineBuilder::new()
        .timer_backend(TimerBackend::Busywait)
        .auto_disconnect_after_no_logon_response(Duration::from_secs(60))
        .build();
    let logon_at = engine
        .logon_deadline()
        .unwrap()
        .checked_sub(Duration::from_secs(60))
        .unwrap();
    assert!(logon_at >= before && logon_at <= Instant::now());

    let mut timers = SessionTimers::new(engine.heartbeat_interval(), engine.timer_backend());
    timers.sync(&engine);
    assert_matches!(&timers.logon, Some(Sleep::Busywait(_)));
    assert!(timers.next_event().now_or_never().is_none());

    let before = Instant::now();
    engine.on_control(ControlMsg::Logout {
        session_status: None,
        text: None,
    });
    let logout_at = engine
        .logout_deadline()
        .unwrap()
        .checked_sub(engine.session_settings().auto_disconnect_after_no_logout)
        .unwrap();
    assert!(logout_at >= before && logout_at <= Instant::now());
    timers.sync(&engine);
    assert_matches!(&timers.logout, Some(Sleep::Busywait(_)));
}

#[tokio::test(start_paused = true)]
async fn disarmed_timers_remain_pending_after_activity() {
    let (engine, _) = EngineBuilder::new()
        .heartbeat_interval(None)
        .logged_on()
        .build();
    let mut timers = SessionTimers::new(engine.heartbeat_interval(), engine.timer_backend());
    timers.sync(&engine);
    assert!(timers.next_event().now_or_never().is_none());

    timers.on_input_received();
    timers.on_output_written();
    time::advance(Duration::from_secs(60)).await;
    timers.sync(&engine);
    assert!(timers.next_event().now_or_never().is_none());
}

#[tokio::test(start_paused = true)]
async fn cancelled_waits_preserve_deadlines() {
    let (engine, _) = EngineBuilder::new().logged_on().build();
    let mut timers = SessionTimers::new(engine.heartbeat_interval(), engine.timer_backend());
    timers.sync(&engine);
    let started = time::Instant::now();

    for _ in 0..3 {
        assert!(timers.next_event().now_or_never().is_none());
        time::advance(Duration::from_secs(10)).await;
        timers.sync(&engine);
    }
    assert_matches!(timers.next_event().await, TimerEvent::Output);
    assert_eq!(started.elapsed(), Duration::from_secs(30));
    timers.on_timeout(TimerEvent::Output);
    assert!(timers.next_event().now_or_never().is_none());

    assert_matches!(timers.next_event().await, TimerEvent::Input);
    assert_eq!(started.elapsed(), Duration::from_secs(36));
}

#[tokio::test(start_paused = true)]
async fn ready_timers_are_delivered_without_losing_the_other_event() {
    let (engine, _) = EngineBuilder::new().logged_on().build();
    let mut timers = SessionTimers::new(engine.heartbeat_interval(), engine.timer_backend());
    timers.sync(&engine);
    time::advance(Duration::from_secs(36)).await;
    let started = time::Instant::now();

    let first = timers.next_event().await;
    timers.on_timeout(first);
    let second = timers.next_event().await;
    timers.on_timeout(second);
    assert_matches!(
        (first, second),
        (TimerEvent::Input, TimerEvent::Output) | (TimerEvent::Output, TimerEvent::Input)
    );
    assert_eq!(started.elapsed(), Duration::ZERO);
    assert!(timers.next_event().now_or_never().is_none());
}

#[tokio::test(start_paused = true)]
async fn periodic_deadline_starts_after_timeout_handling() {
    let (engine, _) = EngineBuilder::new().logged_on().build();
    let mut timers = SessionTimers::new(engine.heartbeat_interval(), engine.timer_backend());
    timers.sync(&engine);
    let started = time::Instant::now();

    let event = timers.next_event().await;
    assert_matches!(event, TimerEvent::Output);
    time::advance(Duration::from_secs(5)).await;
    timers.on_timeout(event);
    timers.on_input_received();

    assert_matches!(timers.next_event().await, TimerEvent::Output);
    assert_eq!(started.elapsed(), Duration::from_secs(65));
}
