use std::{
    assert_matches, future, thread,
    time::{Duration, Instant},
};

use futures_util::FutureExt;
use tokio::time as tokio_time;

use super::{Sleep, TimerBackend};

#[tokio::test(start_paused = true)]
async fn backends_are_independent_on_the_same_thread() {
    let tokio = TimerBackend::Tokio;
    let busywait = TimerBackend::Busywait;
    assert_matches!(tokio.sleep(Duration::ZERO), Sleep::Tokio(_));
    assert_matches!(busywait.sleep(Duration::ZERO), Sleep::Busywait(_));
    assert_matches!(tokio.sleep_until(tokio.now()), Sleep::Tokio(_));
    assert_matches!(busywait.sleep_until(busywait.now()), Sleep::Busywait(_));

    let before = tokio.now();
    tokio
        .timeout(Duration::from_secs(2), future::pending::<()>())
        .await
        .unwrap_err();
    assert_eq!(tokio.now() - before, Duration::from_secs(2));
    assert_eq!(
        busywait.sleep_until(busywait.now()).now_or_never(),
        Some(())
    );
}

#[test]
fn busywait_backend_can_be_carried_to_another_thread() {
    let backend = TimerBackend::Busywait;
    let mut sleep = backend.sleep(Duration::from_secs(3600));
    thread::spawn(move || {
        // The copied backend and an existing timer work without a runtime.
        assert_matches!(backend.sleep(Duration::ZERO), Sleep::Busywait(_));
        assert_matches!(backend.sleep_until(backend.now()), Sleep::Busywait(_));
        assert!((&mut sleep).now_or_never().is_none());
        sleep.reset_after(Duration::ZERO);
        assert_eq!(sleep.now_or_never(), Some(()));
        assert_matches!(
            backend
                .timeout(Duration::ZERO, future::ready(7))
                .now_or_never(),
            Some(Ok(7))
        );
    })
    .join()
    .unwrap();
}

#[tokio::test(start_paused = true)]
async fn clocks_follow_their_backend_with_paused_tokio_time() {
    let tokio = TimerBackend::Tokio;
    let busywait = TimerBackend::Busywait;
    let before = tokio.now();
    tokio_time::advance(Duration::from_secs(3600)).await;
    assert_eq!(tokio.now() - before, Duration::from_secs(3600));

    let wall_before = Instant::now();
    let observed = busywait.now();
    assert!(observed >= wall_before && observed <= Instant::now());

    let mut sleep = busywait.sleep(Duration::from_secs(3600));
    assert!((&mut sleep).now_or_never().is_none());
    sleep.reset_after(Duration::ZERO);
    assert_eq!(sleep.now_or_never(), Some(()));
    assert_eq!(
        busywait.sleep_until(busywait.now()).now_or_never(),
        Some(())
    );
}

#[tokio::test(start_paused = true)]
async fn reset_after_schedules_from_the_tokio_clock() {
    let mut sleep = TimerBackend::Tokio.sleep(Duration::from_secs(1));
    tokio_time::advance(Duration::from_secs(10)).await;

    sleep.reset_after(Duration::from_secs(5));
    let start = tokio_time::Instant::now();
    sleep.await;
    assert_eq!(start.elapsed(), Duration::from_secs(5));
}

#[tokio::test(start_paused = true)]
async fn sleep_until_takes_a_now_derived_instant() {
    tokio_time::advance(Duration::from_secs(10)).await;

    let backend = TimerBackend::Tokio;
    let sleep = backend.sleep_until(backend.now() + Duration::from_secs(2));
    let start = tokio_time::Instant::now();
    sleep.await;
    assert_eq!(start.elapsed(), Duration::from_secs(2));
}

#[tokio::test(start_paused = true)]
async fn overflowing_durations_keep_the_backend_clock() {
    tokio_time::advance(Duration::from_secs(3600)).await;
    let far_future_duration = Duration::from_secs(86400 * 365 * 30);
    for backend in [TimerBackend::Tokio, TimerBackend::Busywait] {
        let before = backend.now();
        let mut sleep = backend.sleep(Duration::MAX);
        assert!((&mut sleep).now_or_never().is_none());

        // Busywait's construction fallback must not use the paused clock.
        if let Sleep::Busywait(timer) = &sleep {
            assert!(timer.wake_time >= before + far_future_duration);
            assert!(timer.wake_time <= backend.now() + far_future_duration);
        }

        sleep.reset_after(Duration::MAX);
        let deadline = match &sleep {
            Sleep::Tokio(timer) => timer.deadline().into_std(),
            Sleep::Busywait(timer) => timer.wake_time,
        };
        assert!(deadline >= before + far_future_duration);
        assert!(deadline <= backend.now() + far_future_duration);
        assert!((&mut sleep).now_or_never().is_none());

        sleep.reset_after(Duration::ZERO);
        sleep.await;
    }
}

#[tokio::test(start_paused = true)]
async fn timeout_completion_wins_over_an_expired_deadline() {
    for backend in [TimerBackend::Tokio, TimerBackend::Busywait] {
        assert_eq!(
            backend
                .timeout(Duration::ZERO, future::ready(7))
                .await
                .unwrap(),
            7
        );
        backend
            .timeout(Duration::ZERO, future::pending::<()>())
            .await
            .unwrap_err();
        assert!(
            backend
                .timeout(Duration::MAX, future::pending::<()>())
                .now_or_never()
                .is_none()
        );
    }
}
