use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll, ready},
    time::{Duration, Instant},
};

use futures_core::Stream;
use pin_project::pin_project;
use tokio::time::interval_at;
use tokio_stream::{StreamExt, adapters::Fuse};

static BUSYWAIT_TIMEOUTS: AtomicBool = AtomicBool::new(false);

#[doc(hidden)]
pub fn enable_busywait_timers(enable_busywait: bool) {
    BUSYWAIT_TIMEOUTS.store(enable_busywait, Ordering::Relaxed);
}

pub async fn timeout<T>(
    duration: Duration,
    future: impl Future<Output = T>,
) -> Result<T, TimeElapsed> {
    if BUSYWAIT_TIMEOUTS.load(Ordering::Relaxed) {
        BusywaitTimeout::new(future, duration).await
    } else {
        tokio::time::timeout(duration, future)
            .await
            .map_err(|_| TimeElapsed(()))
    }
}

#[pin_project(project = TimeoutStreamProj)]
pub enum TimeoutStream<S> {
    Busywait(#[pin] BusywaitTimeoutStream<S>),
    Tokio(#[pin] tokio_stream::adapters::TimeoutRepeating<S>),
}

impl<S: Stream> Stream for TimeoutStream<S> {
    type Item = Result<S::Item, TimeElapsed>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.project() {
            TimeoutStreamProj::Busywait(stream) => stream.poll_next(cx),
            TimeoutStreamProj::Tokio(stream) => {
                let result = ready!(stream.poll_next(cx));
                Poll::Ready(result.map(|r| r.map_err(|_| TimeElapsed(()))))
            }
        }
    }
}

pub fn timeout_stream<S>(duration: Duration, stream: S) -> TimeoutStream<S>
where
    S: Stream,
{
    if BUSYWAIT_TIMEOUTS.load(Ordering::Relaxed) {
        TimeoutStream::Busywait(BusywaitTimeoutStream::new(stream, duration))
    } else {
        // skip first tick that would otherwise get timeout to trigger immediately
        // during first poll operation
        let timeout_interval_start = tokio::time::Instant::now()
            .checked_add(duration)
            .expect("timeout value too long");
        TimeoutStream::Tokio(
            stream.timeout_repeating(interval_at(timeout_interval_start, duration)),
        )
    }
}

#[derive(Debug)]
pub struct TimeElapsed(());

impl fmt::Display for TimeElapsed {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("Time elapsed")
    }
}

impl std::error::Error for TimeElapsed {}

impl From<TimeElapsed> for std::io::Error {
    fn from(_err: TimeElapsed) -> std::io::Error {
        std::io::ErrorKind::TimedOut.into()
    }
}

struct Sleep {
    wake_time: Instant,
}

impl Sleep {
    fn new(duration: Duration) -> Sleep {
        Sleep {
            wake_time: Instant::now()
                .checked_add(duration)
                .expect("sleep time too long"),
        }
    }

    fn reset(&mut self, duration: Duration) {
        self.wake_time = Instant::now()
            .checked_add(duration)
            .expect("sleep time too long");
    }
}

impl Future for Sleep {
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

#[pin_project]
struct BusywaitTimeout<T> {
    #[pin]
    value: T,
    #[pin]
    delay: Sleep,
}

impl<T> BusywaitTimeout<T> {
    pub fn new(value: T, delay: Duration) -> BusywaitTimeout<T> {
        BusywaitTimeout {
            value,
            delay: Sleep::new(delay),
        }
    }
}

impl<T> Future for BusywaitTimeout<T>
where
    T: Future,
{
    type Output = Result<T::Output, TimeElapsed>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        if let Poll::Ready(value) = this.value.poll(cx) {
            Poll::Ready(Ok(value))
        } else {
            match this.delay.poll(cx) {
                Poll::Ready(()) => Poll::Ready(Err(TimeElapsed(()))),
                Poll::Pending => Poll::Pending,
            }
        }
    }
}

#[pin_project]
pub struct BusywaitTimeoutStream<S> {
    #[pin]
    stream: Fuse<S>,
    #[pin]
    deadline: Sleep,
    duration: Duration,
    poll_deadline: bool,
}

impl<S: Stream> BusywaitTimeoutStream<S> {
    fn new(stream: S, duration: Duration) -> Self {
        BusywaitTimeoutStream {
            stream: stream.fuse(),
            deadline: Sleep::new(duration),
            duration,
            poll_deadline: true,
        }
    }
}

impl<S: Stream> Stream for BusywaitTimeoutStream<S> {
    type Item = Result<S::Item, TimeElapsed>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        match this.stream.poll_next(cx) {
            Poll::Ready(v) => {
                if v.is_some() {
                    this.deadline.reset(*this.duration);
                    *this.poll_deadline = true;
                }
                Poll::Ready(v.map(Ok))
            }
            Poll::Pending => {
                if *this.poll_deadline {
                    ready!(this.deadline.poll(cx));
                    *this.poll_deadline = false;
                    Poll::Ready(Some(Err(TimeElapsed(()))))
                } else {
                    this.deadline.reset(*this.duration);
                    *this.poll_deadline = true;
                    Poll::Pending
                }
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let (lower, upper) = self.stream.size_hint();

        // The timeout stream may insert an error before and after each message
        // from the underlying stream, but no more than one error between each
        // message. Hence the upper bound is computed as 2x+1.

        fn twice_plus_one(value: Option<usize>) -> Option<usize> {
            value?.checked_mul(2)?.checked_add(1)
        }

        (lower, twice_plus_one(upper))
    }
}
