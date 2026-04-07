use std::{
    cell::Cell,
    io::ErrorKind,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};

use tokio::{io, io::AsyncWrite, sync::mpsc};

use super::{
    harness::{TestHarness, build_harness_with_settings},
    reset_peer::{ResetPeer, running_reset_transport},
};
use crate::settings::SessionSettings;

pub(super) struct GateWriter<W> {
    pub(super) inner: W,
    pub(super) armed: Rc<Cell<bool>>,
    pub(super) entered: mpsc::UnboundedSender<Vec<u8>>,
    pub(super) release: mpsc::UnboundedReceiver<Result<(), ErrorKind>>,
    pub(super) waiting: bool,
    pub(super) approved: bool,
}

impl<W: AsyncWrite + Unpin> AsyncWrite for GateWriter<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if !self.approved && (self.armed.get() || self.waiting) {
            if !self.waiting {
                self.entered.send(buf.to_vec()).unwrap();
                self.waiting = true;
            }
            match self.release.poll_recv(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Ok(()))) => {
                    self.waiting = false;
                    self.approved = true;
                }
                Poll::Ready(Some(Err(kind))) => {
                    return Poll::Ready(Err(io::Error::new(kind, "injected write failure")));
                }
                Poll::Ready(None) => panic!("write gate closed"),
            }
        }
        let result = Pin::new(&mut self.inner).poll_write(cx, buf);
        if result.is_ready() {
            self.approved = false;
        }
        result
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

pub(super) struct WriteController {
    pub(super) armed: Rc<Cell<bool>>,
    pub(super) entered: mpsc::UnboundedReceiver<Vec<u8>>,
    pub(super) release: mpsc::UnboundedSender<Result<(), ErrorKind>>,
}

pub(super) async fn gated_reset_peer(settings: SessionSettings) -> (ResetPeer, WriteController) {
    gated_reset_harness(build_harness_with_settings(settings)).await
}

pub(super) async fn gated_reset_harness(harness: TestHarness) -> (ResetPeer, WriteController) {
    let (server, wire) = io::duplex(65536);
    let (reader, writer) = io::split(server);
    let armed = Rc::new(Cell::new(false));
    let (entered, events) = mpsc::unbounded_channel();
    let (release, actions) = mpsc::unbounded_channel();
    let writer = GateWriter {
        inner: writer,
        armed: armed.clone(),
        entered,
        release: actions,
        waiting: false,
        approved: false,
    };
    let peer = running_reset_transport(harness, wire, reader, writer).await;
    (
        peer,
        WriteController {
            armed,
            entered: events,
            release,
        },
    )
}
