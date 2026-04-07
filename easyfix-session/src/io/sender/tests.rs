use std::{
    assert_matches,
    pin::pin,
    thread,
    time::{Duration, Instant},
};

use futures_util::poll;
use tokio::time::{self, timeout};

use super::{SendError, channel};
use crate::io::time::TimerBackend;

#[tokio::test(start_paused = true)]
async fn queues_use_their_own_clock_for_staging_and_age() {
    let (tokio_tx, tokio_rx) = channel::<u32>(TimerBackend::Tokio);
    let (busywait_tx, busywait_rx) = channel::<u32>(TimerBackend::Busywait);
    time::advance(Duration::from_secs(3600)).await;

    let before = Instant::now();
    busywait_tx.clone().send(Box::new(1)).unwrap();
    let staged_at = busywait_tx.shared.queue.borrow().front().unwrap().0;
    assert!(staged_at >= before && staged_at <= Instant::now());
    tokio_tx.send(Box::new(2)).unwrap();

    time::advance(Duration::from_secs(3600)).await;
    assert_eq!(tokio_rx.head_age(), Some(Duration::from_secs(3600)));
    assert!(busywait_rx.head_age().unwrap() <= before.elapsed());
}

#[tokio::test]
async fn held_send_notifications_preserve_messages_without_spinning_or_losing_wakes() {
    let (tx, mut rx) = channel::<u32>(TimerBackend::Tokio);
    assert_eq!(rx.len(), 0);
    tx.send(Box::new(1)).unwrap();
    {
        let mut notification = pin!(rx.wait_for_send());
        assert!(poll!(notification.as_mut()).is_ready());
    }
    assert_eq!(rx.len(), 1);
    {
        let mut notification = pin!(rx.wait_for_send());
        assert!(poll!(notification.as_mut()).is_pending());
        assert!(poll!(notification.as_mut()).is_pending());
        tx.send(Box::new(2)).unwrap();
        assert!(poll!(notification.as_mut()).is_ready());
    }
    assert_eq!(rx.len(), 2);
    assert_eq!(*rx.try_recv().unwrap(), 1);
    assert_eq!(*rx.try_recv().unwrap(), 2);
    {
        let mut notification = pin!(rx.wait_for_send());
        assert!(poll!(notification.as_mut()).is_pending());
    }
    // Cancelling an idle select wait cannot lose the next producer permit.
    tx.send(Box::new(3)).unwrap();
    {
        let mut notification = pin!(rx.wait_for_send());
        assert!(poll!(notification.as_mut()).is_ready());
    }
    assert_eq!(*rx.try_recv().unwrap(), 3);
}

#[tokio::test]
async fn send_delivers_message_to_receiver() {
    let (tx, mut rx) = channel::<u32>(TimerBackend::Tokio);
    tx.send(Box::new(42)).expect("send");
    let msg = rx.recv().await.expect("recv");
    assert_eq!(*msg, 42);
}

/// A `recv` that found the queue empty parks on the notification, and the
/// next `send` wakes it. Every other test here stages before receiving, so
/// this is the one place the wake-up path runs at all - without the
/// `notify_one` in `send` the parked receiver would never return.
#[tokio::test]
async fn send_wakes_a_parked_receiver() {
    let (tx, mut rx) = channel::<u32>(TimerBackend::Tokio);
    let mut recv = pin!(rx.recv());
    assert!(
        poll!(recv.as_mut()).is_pending(),
        "nothing staged: recv parks"
    );

    tx.send(Box::new(5)).expect("send");

    let msg = timeout(Duration::from_secs(1), recv)
        .await
        .expect("send must wake the parked recv")
        .expect("a staged message, not a closed channel");
    assert_eq!(*msg, 5);
}

#[tokio::test]
async fn send_stages_synchronously_in_fifo_order() {
    // No capacity, no await on send: a burst of synchronous sends all stage
    // before the first recv and drain back in FIFO order. (Whether send can
    // block at all is not observable here - a blocking send would hang the
    // test rather than fail it.)
    let (tx, mut rx) = channel::<u32>(TimerBackend::Tokio);
    for i in 0..100 {
        tx.send(Box::new(i)).expect("send");
    }
    assert_eq!(tx.backlog_len(), 100);
    for i in 0..100 {
        assert_eq!(*rx.recv().await.expect("recv"), i);
    }
}

#[test]
fn try_recv_pops_in_order_then_none() {
    let (tx, mut rx) = channel::<u32>(TimerBackend::Tokio);
    tx.send(Box::new(1)).expect("send");
    tx.send(Box::new(2)).expect("send");
    assert_eq!(rx.try_recv().map(|m| *m), Some(1));
    assert_eq!(rx.try_recv().map(|m| *m), Some(2));
    assert!(rx.try_recv().is_none());
}

#[test]
fn len_and_backlog_len_track_staged_messages() {
    let (tx, mut rx) = channel::<u32>(TimerBackend::Tokio);
    assert_eq!(rx.len(), 0);
    tx.send(Box::new(1)).expect("send");
    tx.send(Box::new(2)).expect("send");
    assert_eq!(rx.len(), 2);
    assert_eq!(tx.backlog_len(), 2);
    let _ = rx.try_recv();
    assert_eq!(rx.len(), 1);
    assert_eq!(tx.backlog_len(), 1);
}

#[test]
fn head_age_reports_oldest_staged_message() {
    let (tx, mut rx) = channel::<u32>(TimerBackend::Tokio);
    assert_eq!(rx.head_age(), None);

    tx.send(Box::new(1)).expect("send");
    thread::sleep(Duration::from_millis(10));
    tx.send(Box::new(2)).expect("send");

    // Anchored to the queue front: the first message has aged through the
    // sleep, so a head_age below it would mean the newest push was measured.
    let age = rx.head_age().expect("messages staged");
    assert!(age >= Duration::from_millis(10));

    let _ = rx.try_recv();
    assert!(rx.head_age().is_some());
    let _ = rx.try_recv();
    assert_eq!(rx.head_age(), None);
}

#[tokio::test]
async fn recv_returns_none_after_close_once_drained() {
    // `close()` seals the channel; `recv` first drains the queue, then
    // yields `None`.
    let (tx, mut rx) = channel::<u32>(TimerBackend::Tokio);
    tx.send(Box::new(7)).expect("send");
    rx.close();
    assert_eq!(*rx.recv().await.expect("drain"), 7);
    assert!(rx.recv().await.is_none());
}

#[test]
fn send_after_close_returns_closed_with_message() {
    let (tx, mut rx) = channel::<u32>(TimerBackend::Tokio);
    rx.close();
    let err = tx.send(Box::new(9)).unwrap_err();
    assert_matches!(err, SendError::Closed(m) if *m == 9);
}

#[test]
fn sender_clones_share_one_queue() {
    // A clone is another handle on the same staging queue, not a second
    // queue: sends through either handle land in one FIFO, and both handles
    // report the same backlog.
    let (tx, mut rx) = channel::<u32>(TimerBackend::Tokio);
    let tx2 = tx.clone();
    tx.send(Box::new(1)).expect("send");
    tx2.send(Box::new(2)).expect("send");
    tx.send(Box::new(3)).expect("send");
    assert_eq!(tx.backlog_len(), 3);
    assert_eq!(tx2.backlog_len(), 3);
    assert_eq!(rx.try_recv().map(|m| *m), Some(1));
    assert_eq!(rx.try_recv().map(|m| *m), Some(2));
    assert_eq!(rx.try_recv().map(|m| *m), Some(3));
    assert!(rx.try_recv().is_none());
}

#[test]
fn receiver_drop_flips_closed_and_send_returns_closed() {
    // The `Receiver` `Drop` backstop sets `closed`, so every `Sender` handle -
    // the original and its clones alike - observes `Closed` and recovers its
    // message rather than enqueuing into a queue nobody will drain.
    let (tx, rx) = channel::<u32>(TimerBackend::Tokio);
    let tx2 = tx.clone();
    drop(rx);
    let err = tx.send(Box::new(1)).unwrap_err();
    assert_matches!(err, SendError::Closed(m) if *m == 1);
    let err = tx2.send(Box::new(2)).unwrap_err();
    assert_matches!(err, SendError::Closed(m) if *m == 2);
}
