//! Runtime-agnostic timer primitives.
//!
//! This crate is designed to run under any async executor that polls
//! futures correctly - not only the tokio runtime. Direct calls to
//! `tokio::time::*` would tie us to tokio's timer wheel, which only
//! fires when the tokio runtime is driving the task.
//!
//! This module provides replacements for `tokio::time::sleep` and
//! `tokio::time::sleep_until` (the [`Sleep`] type), timeout handling, and the
//! clock they all read. [`TimerBackend`] selects one of two backends for each
//! acceptor or initiator and is passed along to its sessions:
//!
//! - **tokio** (default): defers to `tokio::time::*`. Efficient - uses
//!   the runtime's timer wheel - but only fires under a tokio runtime.
//! - **busywait**: a futures-only fallback that re-arms its waker on
//!   every poll and yields `Pending` until the deadline elapses. Works
//!   under any executor; trades CPU for runtime independence.
//!
//! Every `Instant` the session crate compares against a timer - a deadline
//! handed to [`TimerBackend::sleep_until`], a timestamp whose age is measured -
//! must come from the same backend's [`TimerBackend::now`], never from
//! `Instant::now()` directly. The two clocks agree in production, but tokio's
//! can be paused and advanced under test, and a deadline taken from the wall
//! clock then lags the timer wheel:
//! it is already in the past after the first `advance`, and the timer fires
//! on every poll.

use std::{
    error::Error as StdError,
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};

// Renamed: this module is itself `time`, and `Sleep` is the name of the enum
// defined below - a plain `use tokio::time` would read as self-reference.
use tokio::time as tokio_time;

#[cfg(test)]
mod tests;

/// The clock and timer implementation selected for an acceptor or initiator.
#[derive(Clone, Copy, Debug)]
pub(crate) enum TimerBackend {
    Tokio,
    Busywait,
}

impl TimerBackend {
    /// The current instant on this backend's clock. Use it for deadlines and
    /// timestamps measured by timers created with the same backend.
    //
    // Going through Tokio's Instant honours its paused test clock; outside a
    // runtime it reads the wall clock, so engines can also be constructed in
    // plain synchronous tests.
    pub(crate) fn now(self) -> Instant {
        match self {
            Self::Tokio => tokio_time::Instant::now().into_std(),
            Self::Busywait => Instant::now(),
        }
    }

    /// Wake `duration` from now.
    pub(crate) fn sleep(self, duration: Duration) -> Sleep {
        match self {
            Self::Tokio => Sleep::Tokio(Box::pin(tokio_time::sleep(duration))),
            Self::Busywait => Sleep::Busywait(BusywaitSleep::new(duration)),
        }
    }

    /// Wake at the given deadline, which must derive from this backend's
    /// [`now`](Self::now).
    pub(crate) fn sleep_until(self, deadline: Instant) -> Sleep {
        match self {
            Self::Tokio => Sleep::Tokio(Box::pin(tokio_time::sleep_until(deadline.into()))),
            Self::Busywait => Sleep::Busywait(BusywaitSleep::with_wake_time(deadline)),
        }
    }

    /// Cap `future`'s execution at `duration`. Returns `Err(TimeElapsed)`
    /// on timeout.
    pub(crate) async fn timeout<T>(
        self,
        duration: Duration,
        future: impl Future<Output = T>,
    ) -> Result<T, TimeElapsed> {
        match self {
            Self::Tokio => tokio_time::timeout(duration, future)
                .await
                .map_err(|_| TimeElapsed(())),
            Self::Busywait => BusywaitTimeout::new(future, duration).await,
        }
    }
}

fn far_future(now: Instant) -> Instant {
    // ~30 years from now - no API to get max `Instant`, and larger spans
    // overflow on macOS / FreeBSD. Mirrors tokio's choice.
    now + Duration::from_secs(86400 * 365 * 30)
}

/// Error returned when a [`TimerBackend::timeout`] expires before the wrapped
/// future completes.
#[derive(Debug)]
pub(crate) struct TimeElapsed(());

impl fmt::Display for TimeElapsed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Time elapsed")
    }
}

impl StdError for TimeElapsed {}

// --- Sleep ------------------------------------------------------------------

/// Reset-capable timer. Constructed via [`TimerBackend::sleep`] (relative) or
/// [`TimerBackend::sleep_until`] (absolute); awaiting it completes when the
/// configured deadline elapses. Use [`Sleep::reset_after`] to reschedule
/// without rebuilding the underlying timer registration - cheaper than
/// constructing a fresh `Sleep` on every loop iteration.
///
/// The backend is fixed at construction and also determines the clock used
/// when resetting the timer.
///
/// `Sleep` is `Unpin`, so a caller can hold and await it directly, with no
/// `Pin<Box<...>>` wrapper of its own.
//
// That is why the tokio variant wraps `Pin<Box<tokio::time::Sleep>>`: the
// boxing keeps the pin semantics the inner timer needs off the outer enum.
#[derive(Debug)]
pub(crate) enum Sleep {
    Busywait(BusywaitSleep),
    Tokio(Pin<Box<tokio_time::Sleep>>),
}

impl Sleep {
    /// Reschedule to wake `duration` from now, saturating at a far-future
    /// deadline when the sum is not representable.
    ///
    /// In the tokio variant this moves the entry within the timer wheel
    /// without re-allocating; in the busywait variant it just updates the
    /// deadline field.
    //
    // The saturation is what makes this the only reset the IO loop uses.
    // `Instant`'s own `Add` panics on overflow, and these durations come off
    // the wire: HeartBtInt(108) is an `Int`, so a peer can propose `i64::MAX`
    // seconds and every subsequent reset would kill the session task. Matches
    // what `tokio_time::sleep` and `BusywaitSleep::new` already do on the
    // construction path.
    pub(crate) fn reset_after(&mut self, duration: Duration) {
        let now = match self {
            Self::Busywait(_) => TimerBackend::Busywait.now(),
            Self::Tokio(_) => TimerBackend::Tokio.now(),
        };
        self.reset(now.checked_add(duration).unwrap_or_else(|| far_future(now)));
    }

    fn reset(&mut self, deadline: Instant) {
        match self {
            Sleep::Busywait(b) => b.reset(deadline),
            Sleep::Tokio(t) => t.as_mut().reset(deadline.into()),
        }
    }
}

impl Future for Sleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match &mut *self {
            Sleep::Busywait(b) => Pin::new(b).poll(cx),
            Sleep::Tokio(t) => t.as_mut().poll(cx),
        }
    }
}

// --- BusywaitSleep ----------------------------------------------------------

/// Busywait timer. Re-arms its waker on every poll until `wake_time`
/// is reached. CPU cost is proportional to executor poll rate - only
/// used when no proper timer-aware runtime is available.
///
//
// `pub(crate)` matches the visibility of the enclosing `pub(crate) enum
// Sleep`: the type appears as a field of `Sleep::Busywait`, and a more private
// type there would trip the `private_interfaces` lint ("private type in public
// interface", formerly hard error E0446). Its constructors and fields stay
// private, so the type is still opaque within the crate.
#[derive(Debug)]
pub(crate) struct BusywaitSleep {
    wake_time: Instant,
}

impl BusywaitSleep {
    fn new(duration: Duration) -> BusywaitSleep {
        let now = Instant::now();
        BusywaitSleep {
            wake_time: now.checked_add(duration).unwrap_or_else(|| far_future(now)),
        }
    }

    fn with_wake_time(wake_time: Instant) -> BusywaitSleep {
        BusywaitSleep { wake_time }
    }

    fn reset(&mut self, wake_time: Instant) {
        self.wake_time = wake_time;
    }
}

impl Future for BusywaitSleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.wake_time > Instant::now() {
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}

// --- BusywaitTimeout (private) ----------------------------------------------

/// Future returned by [`TimerBackend::timeout`] in busywait mode. Polls the
/// wrapped future before the delay, so completion wins a same-poll race with
/// the timeout.
struct BusywaitTimeout<T> {
    future: T,
    delay: BusywaitSleep,
}

impl<T> BusywaitTimeout<T> {
    fn new(future: T, duration: Duration) -> BusywaitTimeout<T> {
        BusywaitTimeout {
            future,
            delay: BusywaitSleep::new(duration),
        }
    }
}

impl<T: Future> Future for BusywaitTimeout<T> {
    type Output = Result<T::Output, TimeElapsed>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: nothing below moves out of `this`. `delay` is `Unpin` (it
        // wraps just an `Instant`), so handing out `&mut this.delay` is fine.
        let this = unsafe { self.get_unchecked_mut() };
        // SAFETY: `future` is structurally pinned - `BusywaitTimeout` has no
        // `Drop` impl and no manual `Unpin` impl, so it is `Unpin` only when
        // `T` is, and `&mut this.future` never escapes this method.
        let future = unsafe { Pin::new_unchecked(&mut this.future) };
        if let Poll::Ready(v) = future.poll(cx) {
            return Poll::Ready(Ok(v));
        }
        match Pin::new(&mut this.delay).poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(TimeElapsed(()))),
            Poll::Pending => Poll::Pending,
        }
    }
}
