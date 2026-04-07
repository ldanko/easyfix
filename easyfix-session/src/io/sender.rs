use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    error::Error as StdError,
    fmt,
    rc::Rc,
    time::{Duration, Instant},
};

use tokio::sync::Notify;

use crate::io::time::TimerBackend;

// ---------------------------------------------------------------------------
// Shared staging state
// ---------------------------------------------------------------------------

/// State shared between a [`Sender`] and its [`Receiver`].
///
/// The staging queue holds **un-serialized, un-sequenced** `Box<M>` paired
/// with the `Instant` they were accepted. Sequencing, `SendingTime`
/// stamping, serialization and persistence all happen later, in the session
/// task, at transmit time - never here. This is what keeps `SendingTime`
/// honest under a slow consumer.
struct Shared<M> {
    timer_backend: TimerBackend,
    queue: RefCell<VecDeque<(Instant, Box<M>)>>,
    closed: Cell<bool>,
    notify: Notify,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Error returned by [`Sender::send`] when the session has ended.
///
/// Carries the unsent message so the producer can re-route, buffer, or log
/// it - closing the disconnect race losslessly.
pub enum SendError<M> {
    /// The session task has closed the channel; the unsent message is
    /// handed back.
    Closed(Box<M>),
}

// `M` carries no `Debug`/`Display` bound, so these are implemented by hand
// (a `#[derive]` would impose `M: Debug`). Callers rely on `Debug` via
// `.expect()` / `?err` logging.
impl<M> fmt::Debug for SendError<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SendError::Closed(_) => f.debug_tuple("Closed").finish_non_exhaustive(),
        }
    }
}

impl<M> fmt::Display for SendError<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "channel closed")
    }
}

impl<M> StdError for SendError<M> {}

// ---------------------------------------------------------------------------
// Sender
// ---------------------------------------------------------------------------

/// Outbound message sender for a FIX session.
///
/// [`send`](Sender::send) is **synchronous and never blocks or awaits**: it
/// pushes the message into a per-session staging queue and wakes the session
/// task. The message is sequenced, stamped with `SendingTime`, serialized,
/// and sent later, in the session task, at transmit time. History is stored
/// when enabled by the session settings. Cloning shares the same staging queue.
///
/// `Sender<M>` is `Rc`-backed and therefore `!Send`: the producer that calls
/// `send` must live on the **same** single-threaded `LocalSet` as the
/// session task.
///
/// # Header contract
///
/// Leave session-managed header fields defaulted so the engine can allocate
/// `MsgSeqNum` and stamp `SendingTime` at the real transmit moment. Presetting
/// a non-default `SendingTime` on the enqueued message opts out of
/// transmit-time stamping and re-introduces stale time for that message.
/// A producer-supplied `MsgSeqNum` leaves the session's outgoing counter
/// unchanged. The producer must keep its numbering consistent; with history
/// enabled, reusing a committed number ends the connection with a storage error.
pub struct Sender<M> {
    shared: Rc<Shared<M>>,
}

impl<M> Clone for Sender<M> {
    fn clone(&self) -> Self {
        Sender {
            shared: Rc::clone(&self.shared),
        }
    }
}

impl<M> Sender<M> {
    /// Enqueue a message for transmission. Synchronous; never blocks, never
    /// awaits.
    ///
    /// # Errors
    ///
    /// Returns [`SendError::Closed`] - carrying the unsent message - if the
    /// session task has already closed the channel (TCP loss, peer Logout,
    /// slow-consumer eviction, shutdown, or a `send` issued from within
    /// `on_session_end`).
    pub fn send(&self, msg: Box<M>) -> Result<(), SendError<M>> {
        if self.shared.closed.get() {
            return Err(SendError::Closed(msg));
        }
        self.shared
            .queue
            .borrow_mut()
            .push_back((self.shared.timer_backend.now(), msg));
        self.shared.notify.notify_one();
        Ok(())
    }

    /// Number of messages accepted but not yet transmitted.
    ///
    /// The application sums this across its sessions to drive source-side
    /// load shedding under global overload (the per-session caps cannot do
    /// that).
    pub fn backlog_len(&self) -> usize {
        self.shared.queue.borrow().len()
    }
}

// ---------------------------------------------------------------------------
// Receiver
// ---------------------------------------------------------------------------

/// Consumer half of the staging queue. Lives inside the session task and is
/// never exposed to the application.
pub(crate) struct Receiver<M> {
    shared: Rc<Shared<M>>,
}

impl<M> Receiver<M> {
    /// Await the next staged message. Mirrors
    /// [`mpsc::UnboundedReceiver::recv`](tokio::sync::mpsc::UnboundedReceiver::recv):
    /// returns `Some` as soon as the queue is non-empty, and `None` once the
    /// session task has [`close`](Receiver::close)d the channel and the queue
    /// is drained.
    //
    // Written as the canonical tokio `Notify` idiom - register the `Notified`
    // future *before* re-checking the queue. The `RefCell` borrow is a
    // statement-scoped temporary - only the popped value is moved into a local
    // - so no borrow is ever held across the suspension point. On the
    // single-threaded runtime the guarantee is even simpler: `notify_one` is
    // only called from `send`, which runs on the same thread, so no
    // notification can arrive during this synchronous section.
    pub(crate) async fn recv(&mut self) -> Option<Box<M>> {
        loop {
            let notified = self.shared.notify.notified();
            let popped = self.shared.queue.borrow_mut().pop_front();
            if let Some((_, msg)) = popped {
                return Some(msg);
            }
            if self.shared.closed.get() {
                return None;
            }
            notified.await;
        }
    }

    /// Wait for a producer notification without removing a staged message.
    pub(crate) async fn wait_for_send(&mut self) {
        self.shared.notify.notified().await;
    }

    /// Non-blocking pop for the synchronous teardown drain. Mirrors
    /// [`mpsc::UnboundedReceiver::try_recv`](tokio::sync::mpsc::UnboundedReceiver::try_recv),
    /// simplified to an `Option`.
    pub(crate) fn try_recv(&mut self) -> Option<Box<M>> {
        self.shared
            .queue
            .borrow_mut()
            .pop_front()
            .map(|(_, msg)| msg)
    }

    /// Age of the oldest un-transmitted message - the lag cap's metric.
    //
    // The one method no `mpsc` can offer, and the reason the staging tier is a
    // custom `VecDeque<(Instant, Box<M>)>` rather than an unbounded channel.
    pub(crate) fn head_age(&self) -> Option<Duration> {
        self.shared.queue.borrow().front().map(|(t, _)| {
            self.shared
                .timer_backend
                .now()
                .saturating_duration_since(*t)
        })
    }

    /// Number of messages currently staged.
    pub(crate) fn len(&self) -> usize {
        self.shared.queue.borrow().len()
    }

    /// Seal the channel: subsequent `send`s return [`SendError::Closed`], and
    /// `recv` returns `None` once the queue is drained.
    //
    // No `notify_one` here, and none is needed: a parked `recv` holds the
    // `&mut self` this method also takes, so the two cannot overlap - the
    // borrow checker, not the call order in `finish_staged_sends`, is what rules
    // out a `recv` left waiting on a closed channel.
    pub(crate) fn close(&mut self) {
        self.shared.closed.set(true);
    }
}

// Backstop: if the session task is torn down without an explicit `close()`
// (a panic inside the loop, or any early-return path before normal teardown),
// dropping the `Receiver` still flips the `closed` flag so a producer holding
// a `Sender` clone observes `SendError::Closed` rather than enqueuing into a
// queue nobody will drain. The explicit `close()` in `finish_staged_sends` is
// still required for its *ordering* (it must precede `on_session_end`, which
// `Drop` cannot guarantee).
impl<M> Drop for Receiver<M> {
    fn drop(&mut self) {
        self.shared.closed.set(true);
    }
}

/// Create a sender/receiver pair sharing one staging queue.
pub(crate) fn channel<M>(timer_backend: TimerBackend) -> (Sender<M>, Receiver<M>) {
    let shared = Rc::new(Shared {
        timer_backend,
        queue: RefCell::new(VecDeque::new()),
        closed: Cell::new(false),
        notify: Notify::new(),
    });
    (
        Sender {
            shared: Rc::clone(&shared),
        },
        Receiver { shared },
    )
}

#[cfg(test)]
mod tests;
