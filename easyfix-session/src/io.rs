mod input_stream;
pub(crate) mod sender;
pub(crate) mod time;
mod timer;

#[cfg(test)]
mod tests;

use std::{io, ops::RangeInclusive, time::Instant};

use easyfix_core::{
    basic_types::{FixString, NonZeroSeqNum, SeqNum, SessionStatusField},
    deserializer::DeserializeErrorKind,
    message::{DeserializeError, MsgCat, SessionMessage},
};
use futures_util::StreamExt;
pub(crate) use input_stream::{FirstMessageEvent, InputEvent, InputStream};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    sync::mpsc,
    task,
};
use tracing::{Instrument, error, info, info_span, warn};

use crate::{
    application::{Application, DisconnectReason},
    engine::{FatalError, InputResult, PendingOutput, SendFailure, SessionEngine},
    initiator::SessionStart,
    io::{
        sender::{Receiver, Sender},
        timer::{SessionTimers, TimerEvent},
    },
    messages_storage::MessagesStorage,
};

/// Session lifecycle requests delivered over the control channel.
pub(crate) enum ControlMsg {
    Logout {
        session_status: Option<SessionStatusField>,
        text: Option<FixString>,
    },
    Disconnect,
    ResetRunningSession,
}

/// Expire the reset budget before doing more ordinary work. Returns whether
/// the loop must finish, including an engine decision made before this check.
fn check_reset_timeout<M: SessionMessage>(
    timers: &mut SessionTimers,
    engine: &mut SessionEngine<M>,
) -> bool {
    if !engine.should_leave_loop() && timers.sync_reset_and_check(engine) {
        engine.on_reset_timeout();
    }
    engine.should_leave_loop()
}

#[derive(Debug, Error)]
enum OutputError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("fatal storage or replay error")]
    Fatal(#[from] FatalError),
}

impl OutputError {
    fn disconnect<M: SessionMessage>(&self, engine: &mut SessionEngine<M>) {
        if matches!(self, Self::Io(_)) {
            engine.begin_disconnect(DisconnectReason::IoError);
        }
    }
}

/// Write pending output with a timeout per write, returning whether any bytes
/// were written. A storage error stops output immediately.
async fn flush_output<M, W, S>(
    writer: &mut W,
    storage: &mut S,
    engine: &mut SessionEngine<M>,
) -> Result<bool, OutputError>
where
    M: SessionMessage,
    W: AsyncWrite + Unpin,
    S: MessagesStorage,
{
    if engine.has_fatal_error() {
        return Err(FatalError.into());
    }
    let write_timeout = engine.session_settings().write_timeout;
    let mut bytes_written = false;

    while let Some(pending) = engine.take_pending() {
        let data: &[u8] = match &pending {
            PendingOutput::Stored(seq_num) => match storage.fetch(*seq_num, *seq_num).await {
                Ok(bytes) => bytes,
                Err(err) => {
                    error!(%seq_num, operation = "fetch for send", %err, "storage read failed");
                    return Err(engine.fail_storage("fetch for send", &err).into());
                }
            },
            PendingOutput::Transient { len } => &engine.scratch()[..*len],
        };
        engine
            .timer_backend()
            .timeout(write_timeout, writer.write_all(data))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "TCP write timed out"))??;
        bytes_written = true;
    }
    Ok(bytes_written)
}

/// Flush pending output, then stamp and deliver each queued admin message.
/// Calls `on_admin_msg_out` before each commit; with `manages_admin_output`,
/// only stamping and the callback are performed for those messages.
///
/// Returns whether any bytes were written. Stops on exhausted numbering,
/// before the unstamped message reaches its callback, or on a fatal error.
async fn drain_and_flush_admin_output<M, W, S, A>(
    writer: &mut W,
    storage: &mut S,
    engine: &mut SessionEngine<M>,
    app: &mut A,
) -> Result<bool, OutputError>
where
    M: SessionMessage,
    W: AsyncWrite + Unpin,
    S: MessagesStorage,
    A: Application<M>,
{
    let mut bytes_written = flush_output(writer, storage, engine).await?;
    while let Some(mut admin_msg) = engine.take_admin_output() {
        // Header first - the message span needs the assigned seq num.
        if !engine.fill_header(&mut admin_msg, storage)? {
            // Unnumbered, so it cannot go out and must not reach the callback
            // or the commit - `commit_send` would find a zero seq num. It
            // leaves no trace on the wire, so say so here.
            error!(
                msg_type = %admin_msg.msg_type(),
                "Outgoing sequence numbers exhausted, dropping admin message and \
                 everything queued behind it",
            );
            break;
        }
        let span = info_span!(
            "msg",
            dir = "out",
            msg_type = %admin_msg.msg_type(),
            seq_num = admin_msg.msg_seq_num(),
        );
        bytes_written |= async {
            app.on_admin_msg_out(&mut admin_msg);
            if !engine.session_settings().manages_admin_output
                && let Err(failure) = engine.commit_send(admin_msg, storage)
            {
                match failure {
                    SendFailure::Serialize(failure) => {
                        app.on_serialize_error(failure.msg, &failure.error)
                    }
                    SendFailure::Fatal(error) => return Err(OutputError::Fatal(error)),
                }
            }
            // Flush before the next commit can reuse the session buffer.
            flush_output(writer, storage, engine).await
        }
        .instrument(span)
        .await?;
    }
    Ok(bytes_written)
}

/// Commit and write the messages the application has already staged on
/// `app_rx`, flushing each one before the next is committed. The counterpart
/// of [`drain_and_flush_admin_output`] for application sends.
///
/// Drains at most as many messages as are queued on entry. Stops on exhausted
/// numbering or a fatal error.
///
/// Returns `Ok(true)` if any bytes were written to TCP.
//
// Per-commit flushing keeps scratch output alive until its write completes.
//
// The entry-count bound is what keeps that flush affordable. `flush_output`
// awaits, and on the single-threaded runtime a producer task runs at that await
// and can enqueue again, so an unbounded `while let Some(_) = try_recv()` would
// let a busy producer extend the drain indefinitely and the session would never
// reach its `break`. `finish_staged_sends` keeps that window shut by staying
// entirely synchronous; here the count does it instead.
async fn drain_and_flush_app_sends<M, W, S, A>(
    writer: &mut W,
    storage: &mut S,
    engine: &mut SessionEngine<M>,
    app: &mut A,
    app_rx: &mut Receiver<M>,
) -> Result<bool, OutputError>
where
    M: SessionMessage,
    W: AsyncWrite + Unpin,
    S: MessagesStorage,
    A: Application<M>,
{
    // Output from the previous iteration may still borrow the session buffer.
    let mut bytes_written = flush_output(writer, storage, engine).await?;
    for _ in 0..app_rx.len() {
        let Some(msg) = app_rx.try_recv() else {
            break;
        };
        if !prepare_user_send(engine, app, storage, msg, SessionEngine::commit_send)? {
            error!(
                dropped = app_rx.len() + 1,
                "Outgoing sequence numbers exhausted, abandoning the staged send queue",
            );
            break;
        }
        bytes_written |= flush_output(writer, storage, engine).await?;
    }
    Ok(bytes_written)
}

/// The commit step of [`prepare_user_send`]: either
/// [`SessionEngine::commit_send`] (normal path) or
/// [`SessionEngine::store_for_resend`] (post-loop drain).
type CommitFn<M, S> = fn(&mut SessionEngine<M>, Box<M>, &mut S) -> Result<(), SendFailure<M>>;

/// Stamp a staged message, run its output callback, and call `commit_fn`.
///
/// Returns `Ok(false)` when numbering is exhausted, before the callback or
/// commit. Reports recoverable serialization errors through the application.
/// The caller must stop draining on `Ok(false)` or a fatal error.
fn prepare_user_send<M, S, A>(
    engine: &mut SessionEngine<M>,
    app: &mut A,
    storage: &mut S,
    mut msg: Box<M>,
    commit_fn: CommitFn<M, S>,
) -> Result<bool, FatalError>
where
    M: SessionMessage,
    S: MessagesStorage,
    A: Application<M>,
{
    // Header first - the message span needs the assigned seq num.
    if !engine.fill_header(&mut msg, storage)? {
        error!(
            msg_type = %msg.msg_type(),
            "Outgoing sequence numbers exhausted, dropping staged message",
        );
        return Ok(false);
    }
    let _span = info_span!(
        "msg",
        dir = "out",
        msg_type = %msg.msg_type(),
        seq_num = msg.msg_seq_num(),
    )
    .entered();
    match msg.msg_cat() {
        MsgCat::Admin => app.on_admin_msg_out(&mut msg),
        MsgCat::App => app.on_app_msg_out(&mut msg),
    }
    if let Err(failure) = commit_fn(engine, msg, storage) {
        match failure {
            SendFailure::Serialize(failure) => app.on_serialize_error(failure.msg, &failure.error),
            SendFailure::Fatal(error) => return Err(error),
        }
    }
    Ok(true)
}

/// Archive unsent messages when history is enabled and storage is healthy,
/// then close and empty the staging queue. No application callback is awaited.
fn finish_staged_sends<M, S, A>(
    engine: &mut SessionEngine<M>,
    storage: &mut S,
    app: &mut A,
    app_rx: &mut Receiver<M>,
) where
    M: SessionMessage,
    S: MessagesStorage,
    A: Application<M>,
{
    let mut dropped = 0;
    if engine.session_settings().persist_messages && !engine.has_fatal_error() {
        while let Some(msg) = app_rx.try_recv() {
            match prepare_user_send(engine, app, storage, msg, SessionEngine::store_for_resend) {
                Ok(true) => {}
                Ok(false) | Err(_) => {
                    dropped += 1;
                    break;
                }
            }
        }
    }
    app_rx.close();
    while app_rx.try_recv().is_some() {
        dropped += 1;
    }
    if dropped > 0 {
        warn!(dropped, "discarding unsent messages at session end");
    }
}

/// Notify the application of the final disconnect reason.
/// Call after finishing staged sends and releasing both transport halves.
async fn finalize_session<M, A>(engine: &SessionEngine<M>, app: &mut A)
where
    M: SessionMessage,
    A: Application<M>,
{
    let disconnect_reason = engine
        .disconnect_reason()
        .unwrap_or(DisconnectReason::Disconnected);
    app.on_session_end(engine.session_id(), disconnect_reason)
        .await;
}

/// Deliver a validated input result to the application and apply its response.
/// Check reset acknowledgement and incoming numbering limits after processing.
/// A fatal error stops dispatch without running those final checks.
async fn dispatch_input_result<M, S, A>(
    result: InputResult<M>,
    engine: &mut SessionEngine<M>,
    storage: &mut S,
    app: &mut A,
) -> Result<(), FatalError>
where
    M: SessionMessage,
    S: MessagesStorage,
    A: Application<M>,
{
    if engine.has_fatal_error() {
        return Err(FatalError);
    }
    match result {
        InputResult::Handled => {}
        InputResult::AppMsg(msg) => {
            // Capture the routing fields before handing ownership to the
            // application - the Reject branch in `process_app_input`
            // still needs them after the callback consumes `msg`.
            let ref_seq_num = msg.msg_seq_num();
            let ref_msg_type = msg.msg_type();
            let span = info_span!(
                "msg",
                dir = "in",
                msg_type = %ref_msg_type,
                seq_num = ref_seq_num,
            );
            async {
                let action = app.on_app_msg_in(msg).await;
                engine.process_app_input(ref_seq_num, ref_msg_type, action, storage)
            }
            .instrument(span)
            .await?;
        }
        InputResult::AdminMsg(msg) => {
            let span = info_span!(
                "msg",
                dir = "in",
                msg_type = %msg.msg_type(),
                seq_num = msg.msg_seq_num(),
            );
            async {
                let action = app.on_admin_msg_in(&msg).await;
                engine.process_admin_input(msg, action, storage)
            }
            .instrument(span)
            .await?;
        }
        InputResult::Error(error) => {
            app.on_deserialize_error(&error);
        }
    }
    // Every production entry to the engine's input path funnels through here,
    // and it runs after `process_admin_input` / `process_app_input` - the phase
    // where the incoming counter actually advances. Reacting once, here, is
    // what keeps a numbering-exhausted Logout from being emitted alongside one
    // a handler already sent.
    engine.end_session_if_reset_ack_number_consumed(storage);
    engine.end_session_if_target_numbering_exhausted(storage);
    Ok(())
}

/// Process one message from the active resend range.
///
/// Queue a replay or accumulate a gap according to message type, application
/// policy and history settings. Advance the cursor only when handled;
/// an accumulated gap may be queued first, leaving the current message pending.
/// Returns `Ok(false)` when the range is exhausted. Storage or replay failures
/// are fatal. The caller must flush pending output before the next invocation.
async fn process_one_resend<M, S, A>(
    active_resend: &mut Option<RangeInclusive<SeqNum>>,
    storage: &mut S,
    engine: &mut SessionEngine<M>,
    app: &mut A,
) -> Result<bool, FatalError>
where
    M: SessionMessage,
    S: MessagesStorage,
    A: Application<M>,
{
    if engine.has_fatal_error() {
        return Err(FatalError);
    }
    let Some(range) = active_resend else {
        return Ok(false);
    };

    let seq_num = *range.start();
    let range_end = *range.end();
    if seq_num > range_end {
        *active_resend = None;
        return Ok(false);
    }

    let mut advance = true;

    if !engine.session_settings().persist_messages {
        engine.accumulate_resend_gap(seq_num);
    } else {
        let (Some(seq), Some(end)) = (NonZeroSeqNum::new(seq_num), NonZeroSeqNum::new(range_end))
        else {
            return Err(engine.fail_storage("replay range", &"zero sequence number"));
        };
        let bytes = match storage.fetch(seq, end).await {
            Ok(bytes) => bytes,
            Err(err) => {
                error!(seq_num, range_end, operation = "fetch for replay", %err, "storage read failed");
                return Err(engine.fail_storage("fetch for replay", &err));
            }
        };
        // Deserialize to check if we should gap-fill
        match M::from_bytes(bytes) {
            Ok(msg) => {
                let should_gap_fill = engine.resend_as_gap_fill(&msg) || app.should_gap_fill(&msg);

                if should_gap_fill {
                    engine.accumulate_resend_gap(seq_num);
                } else if engine.has_accumulated_resend_gap() {
                    // Flush the pending gap-fill this iteration and
                    // defer the resend to the next call. Producing
                    // both Transients here would alias the shared
                    // scratch buffer - the second serialize would
                    // overwrite the first before flush_output runs.
                    engine.flush_resend_gap()?;
                    advance = false;
                } else {
                    engine.process_resend_message(seq_num, bytes)?;
                }
            }
            Err(err) => {
                error!(%err, seq_num, operation = "decode replay", "failed to decode stored message");
                return Err(engine.fail_storage("decode replay", &err));
            }
        }
    }

    if advance {
        *active_resend = (seq_num < range_end).then(|| (seq_num + 1)..=range_end);
    }

    Ok(true)
}

/// The role-specific opening move of a session - what goes through the engine
/// before its first admin output is flushed. Everything after that is shared,
/// see [`session_loop`].
pub(crate) enum SessionOpening<M> {
    /// Initiator: stage our own `Logon<A>` and wait for the response in the
    /// main loop.
    SendLogon(SessionStart),
    /// Acceptor: the first message already read from the connection, or the
    /// decode error of an invalid first Logon from a registered peer - fed
    /// into the engine's deserialize-error path, which answers per Test Cases
    /// Scenario 1S(d): (optional) Reject, Logout with Text(58), disconnect.
    FirstMessage(Result<Box<M>, DeserializeError>),
}

/// Session loop shared by both roles.
///
/// Runs the [`SessionOpening`] through the engine, drains and flushes the
/// admin output it produced (the Logon request, the Logon response, or the
/// Scenario 1S(d) Reject/Logout), notifies the application, then enters the
/// main select loop. Every exit goes through [`finalize_session`].
///
/// For [`SessionOpening::FirstMessage`], `input` must be the `InputStream`
/// already used to read that message - this preserves any bytes buffered past
/// it.
///
/// `storage` is borrowed, not consumed, so the caller retains ownership and
/// can return it to its session registry even if this future panics (the
/// acceptor's `session_task` relies on that for panic-safe storage handoff).
#[expect(
    clippy::too_many_arguments,
    reason = "internal session wiring, not public API"
)]
pub(crate) async fn session_loop<M, R, W, S, A>(
    opening: SessionOpening<M>,
    mut input: InputStream<R, M>,
    mut writer: W,
    mut engine: SessionEngine<M>,
    storage: &mut S,
    mut app: A,
    sender: Sender<M>,
    mut app_rx: Receiver<M>,
    mut control_rx: mpsc::Receiver<ControlMsg>,
) where
    M: SessionMessage,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    S: MessagesStorage,
    A: Application<M>,
{
    // --- Pre-loop: the opening move ---

    let opening_result = match opening {
        SessionOpening::SendLogon(start) => engine.send_logon_request(storage, start),
        SessionOpening::FirstMessage(first_msg) => {
            let result = match first_msg {
                Ok(msg) => engine.on_input(msg, storage),
                Err(error) => engine.on_deserialize_error(error, storage),
            };
            match result {
                Ok(result) => dispatch_input_result(result, &mut engine, storage, &mut app).await,
                Err(error) => Err(error),
            }
        }
    };

    // Drain admin output and flush to TCP *before* acting on a disconnect
    // decision and before notifying the application. On the happy path this
    // writes the Logon request or response; on a rejected first Logon the
    // handler has staged the mandated Logout(35=5)/Reject into `admin_output`
    // and latched the disconnect, so flushing first ensures the peer receives
    // it rather than a bare TCP close (Scenario 1S(d); FIX Session Layer
    // §4.3.10).
    if opening_result.is_ok()
        && let Err(err) =
            drain_and_flush_admin_output(&mut writer, storage, &mut engine, &mut app).await
    {
        error!(%err, "failed to flush the opening admin output");
        err.disconnect(&mut engine);
    }

    // A session can be over before it starts: the opening was refused, a
    // closed outgoing numbering restored from storage left the Logon request
    // unstamped, or the flush above failed. Either way there is no session to
    // announce, so end here rather than hand the application a `sender` for a
    // connection that is already over.
    if engine.should_disconnect() {
        finish_staged_sends(&mut engine, storage, &mut app, &mut app_rx);
        drop(input);
        drop(writer);
        return finalize_session(&engine, &mut app).await;
    }

    app.on_session_ready(engine.session_id(), sender).await;

    // --- Main loop ---

    run_session_loop(
        &mut input,
        &mut writer,
        &mut engine,
        storage,
        &mut app,
        &mut app_rx,
        &mut control_rx,
    )
    .await;

    // --- Peer's logout: wait for its close ---
    //
    // The loop leaves on an acknowledged peer Logout without a disconnect
    // decision: the Logout initiator is the one who terminates the transport
    // (FIX Session Layer §4.6, Figure 9), so the connection is now the peer's
    // to close, and we wait for that - bounded, per Test Cases Scenario 13(b).
    if let Some(deadline) = engine.awaiting_peer_close()
        && !engine.should_disconnect()
    {
        await_peer_close(&mut input, &mut engine, &mut control_rx, deadline).await;
    }

    finish_staged_sends(&mut engine, storage, &mut app, &mut app_rx);
    drop(input);
    drop(writer);
    finalize_session(&engine, &mut app).await;
}

/// Wait for the peer to close the connection after its Logout was
/// acknowledged, and name the reason from what ends the wait: the close
/// itself is `RemoteRequestedLogout`, the deadline the matching timeout, and a
/// control command whatever the engine makes of it.
///
/// Nothing the peer sends in the meantime is read as FIX - the exchange is
/// complete (FIX Session Layer §4.6), so the bytes are discarded on the way
/// to the EOF. `NextNumIn` is untouched by construction: a message the peer
/// should not have sent leaves a gap it recovers on reconnect.
//
// A read error is the close by other means: the exchange is what makes this
// a logout, and a transport fault after our acknowledgement does not turn it
// into `IoError`. A write failure on the acknowledgement itself latches a
// disconnect before this wait is entered.
async fn await_peer_close<M, R>(
    input: &mut InputStream<R, M>,
    engine: &mut SessionEngine<M>,
    control_rx: &mut mpsc::Receiver<ControlMsg>,
    deadline: Instant,
) where
    M: SessionMessage,
    R: AsyncRead + Unpin,
{
    let mut close_deadline = engine.timer_backend().sleep_until(deadline);
    loop {
        tokio::select! {
            closed = input.discard_until_closed() => {
                match closed {
                    Ok(()) => info!("peer closed the connection after its logout"),
                    Err(err) => warn!(%err, "connection failed while awaiting the peer's close"),
                }
                engine.begin_disconnect(DisconnectReason::RemoteRequestedLogout);
                return;
            }
            () = &mut close_deadline => {
                engine.on_peer_close_timeout();
                return;
            }
            Some(msg) = control_rx.recv() => {
                engine.on_control(msg);
                if engine.should_disconnect() {
                    return;
                }
            }
        }
    }
}

/// Main session select loop. Shared between acceptor and initiator paths.
async fn run_session_loop<M, R, W, S, A>(
    input: &mut InputStream<R, M>,
    writer: &mut W,
    engine: &mut SessionEngine<M>,
    storage: &mut S,
    app: &mut A,
    app_rx: &mut Receiver<M>,
    control_rx: &mut mpsc::Receiver<ControlMsg>,
) where
    M: SessionMessage,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    S: MessagesStorage,
    A: Application<M>,
{
    let time = engine.timer_backend();
    let mut timers = SessionTimers::new(engine.heartbeat_interval(), time);
    let resend_batch_size = engine.session_settings().resend_batch_size.get();
    let queued_batch_size = engine.session_settings().queued_batch_size.get();
    // Slow-consumer caps. Hoisted into `Copy` locals so the loop body's
    // many `&mut engine` calls don't conflict with a held `&engine` settings
    // borrow.
    let max_outbound_queued_messages = engine.session_settings().max_outbound_queued_messages;
    let max_outbound_lag = engine.session_settings().max_outbound_lag;

    let mut active_resend: Option<RangeInclusive<SeqNum>> = None;
    // Give messages held during recovery or reset one lag-cap grace window
    // to drain after sends resume.
    let mut prev_sends_held = false;
    let mut sends_released_at: Option<Instant> = None;

    'session: loop {
        if engine.has_fatal_error() {
            break 'session;
        }
        check_reset_timeout(&mut timers, engine);
        let mut any_bytes_written = false;
        // Track whether the event-select app-send arm fired this iteration, to
        // scope the single-threaded fairness yield (after the select! block)
        // to that arm only - never input/timeout/control iterations.
        let mut processed_app_send = false;

        // --- Terminating iteration ---
        //
        // Once the engine has decided to end the connection - or has
        // acknowledged the peer's Logout, after which only the peer's close
        // is left to wait for - nothing more goes to the application or on
        // the wire beyond what the peer is owed. This block runs ahead of
        // every other section, writes that, and leaves.
        //
        // App sends first. The dispatch that made the decision ran the
        // application callback *before* the engine reacted, so anything the
        // application staged was staged before the engine's Logout and belongs
        // on the wire ahead of it - `fix_service` answers an anti-flooding
        // penalty with a BusinessMessageReject explaining the breach, then the
        // Logout that enacts it. The two travel different queues (`app_rx`
        // versus `admin_output`), and without this drain `finalize_session`
        // would route the staged message to `store_for_resend`, persisting it
        // for a `ResendRequest` a disconnected peer will never send.
        //
        // The gate mirrors the event-select drain arm exactly. `accepts_app_sends()`
        // keeps a session torn down before Logon completed from putting app
        // messages on the wire ahead of the handshake. The resend conditions
        // keep fresh, non-PossDup sends out of the middle of a replay we still
        // owe the peer (FIX Session Layer §4.8.5) - a backlog left undrained
        // here is not lost, `finalize_session` routes it to `store_for_resend`.
        // The slow-consumer caps `break` straight out of the loop, so an
        // evicted consumer's backlog is not drained here either.
        //
        // The admin drain then writes the Logout - or the Reject/Logout that
        // answers a message parked behind a ResendRequest and re-dispatched
        // once the gap closed - so the peer gets the response it is owed
        // rather than a bare TCP close (Scenario 1S(d)).
        //
        // A failed write may have left a partial frame on the wire. Stop
        // writing immediately, including the courtesy Logout. Latching the
        // failure preserves any earlier disconnect reason and skips waiting
        // for the peer's close if its Logout could not be acknowledged.
        if engine.should_leave_loop() {
            if engine.accepts_app_sends()
                && active_resend.is_none()
                && !engine.has_pending_resends()
                && let Err(err) =
                    drain_and_flush_app_sends(writer, storage, engine, app, app_rx).await
            {
                error!(%err, "failed to flush staged app messages before disconnect");
                err.disconnect(engine);
                break 'session;
            }
            if let Err(err) = drain_and_flush_admin_output(writer, storage, engine, app).await {
                error!(%err, "failed to flush admin output before disconnect");
                err.disconnect(engine);
            }
            break 'session;
        }

        // --- Flush & admin drain ---
        //
        // Flush any output carried over from the previous iteration (e.g. an
        // app send), then drain admin output committing and flushing each
        // message before the next. Flushing per commit keeps at most one
        // committed-but-unflushed message at a time, preserving scratch bytes.
        match drain_and_flush_admin_output(writer, storage, engine, app).await {
            Ok(written) => any_bytes_written |= written,
            Err(err) => {
                error!(%err, "failed to flush admin output");
                err.disconnect(engine);
                break 'session;
            }
        }
        if check_reset_timeout(&mut timers, engine) {
            continue 'session;
        }

        // --- Slow-consumer caps ---
        //
        // Checked here - immediately after the flush above, before any
        // `continue` - so the count cap runs every iteration once logged on,
        // including the resend iterations where the app-drain arm is gated off
        // but the backlog can still grow.
        //
        // Detect held sends resuming and arm the lag-cap grace window.
        // Kept outside the `is_logged_on()` gate so no transition is missed.
        let sends_held =
            active_resend.is_some() || engine.has_pending_resends() || !engine.accepts_app_sends();
        if prev_sends_held && !sends_held {
            sends_released_at = Some(time.now());
        }
        prev_sends_held = sends_held;

        // Gated on `is_logged_on()`: `SlowConsumer` means an *established* peer
        // falling behind. Pre-logon the drain arm is gated off and an
        // initiator's app may pre-send the moment it receives the `Sender` from
        // `on_session_ready` (which runs in `LogonSent`, before `Established`);
        // evicting on that staged backlog would mislabel a slow *peer logon* as
        // a slow consumer. The gate does NOT suppress the count cap during a
        // resend - the state is `Established` there, exactly where the count
        // cap must keep bounding memory.
        if engine.is_logged_on() {
            let over_count =
                max_outbound_queued_messages.is_some_and(|max| app_rx.len() > max.get());
            // The lag cap is meaningful only when app sends can actually drain.
            // Recovery and reset age the head independently of peer speed.
            // Give it one grace window to drain after sends resume.
            let in_send_grace = max_outbound_lag
                .zip(sends_released_at)
                .is_some_and(|(max, t)| time.now().saturating_duration_since(t) < max);
            let over_lag = !sends_held
                && !in_send_grace
                && max_outbound_lag
                    .zip(app_rx.head_age())
                    .is_some_and(|(max, age)| age > max);
            if over_count || over_lag {
                engine.begin_disconnect(DisconnectReason::SlowConsumer);
                break 'session;
            }
        }

        // --- Resend batch (if active) ---
        if active_resend.is_some() {
            if active_resend
                .as_ref()
                .is_some_and(|range| !range.is_empty())
            {
                engine.mark_probe_stale();
            }
            for _ in 0..resend_batch_size {
                // `false` means the range is exhausted, and it always leaves
                // `active_resend` cleared - so any residual gap belongs to
                // the block below, which is the only one that also writes it
                // to TCP. Flushing it here would consume `gap_fill_range` and
                // disarm that block's guard.
                match process_one_resend(&mut active_resend, storage, engine, app).await {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(_) => break 'session,
                }
                match flush_output(writer, storage, engine).await {
                    Ok(written) => any_bytes_written |= written,
                    Err(err) => {
                        error!(%err, "failed to flush resend message");
                        err.disconnect(engine);
                        break 'session;
                    }
                }
            }
            // The range can exhaust with a trailing gap-fill still
            // accumulating: when the final seq num is a gap-fill candidate,
            // `process_one_resend` accumulates it and clears `active_resend`.
            // Both ways out of the batch loop above land here - the count
            // limit (which is the only one the default `resend_batch_size
            // = 1` ever reaches) and the `break`. This is the single place
            // that flushes the residual gap AND writes it to TCP; leaving it
            // orphaned stalls the peer's NextNumIn below the gap and
            // deadlocks recovery (FIX Session Layer §4.8.5; Scenario 8).
            if active_resend.is_none() && engine.has_accumulated_resend_gap() {
                if engine.flush_resend_gap().is_err() {
                    break 'session;
                }
                match flush_output(writer, storage, engine).await {
                    Ok(written) => any_bytes_written |= written,
                    Err(err) => {
                        error!(%err, "failed to flush trailing resend gap-fill");
                        err.disconnect(engine);
                        break 'session;
                    }
                }
            }
            if check_reset_timeout(&mut timers, engine) {
                continue 'session;
            }
        }

        if any_bytes_written {
            timers.on_output_written();
        }

        // --- Queued-message drain (up to batch_size per iteration) ---
        //
        // The one section that can end the session mid-iteration: an
        // application answering a re-dispatched message, a re-dispatched peer
        // Logout, or the incoming numbering running out under it. Every other
        // route - the select arms, the control check - reaches the terminating
        // block at the top of the next iteration on its own. This one stops
        // dispatching the moment the decision is made and jumps there, so no
        // further parked message reaches the application. Those messages are
        // not lost: `next_target` did not advance past them, so the peer
        // resends them after reconnect.
        for _ in 0..queued_batch_size {
            if check_reset_timeout(&mut timers, engine) {
                continue 'session;
            }
            // A gap-closing input may have requested a replay that the loop
            // has not picked up yet. Hold queued input until both that work
            // and any active replay (including its trailing gap-fill) finish.
            if active_resend.is_some() || engine.has_pending_resends() {
                break;
            }
            // Flush each staged reply before dispatching another queued
            // message, so it keeps the numbering in force when it was built.
            // A following Logon may discard the old store and renumber.
            if engine.should_leave_loop() || engine.has_admin_output() {
                break;
            }
            if engine.has_queued_message(storage.next_target_msg_seq_num().get()) {
                engine.mark_probe_stale();
            }
            let result = match engine.next_queued_message(storage) {
                Ok(Some(result)) => result,
                Ok(None) => break,
                Err(_) => break 'session,
            };
            if dispatch_input_result(result, engine, storage, app)
                .await
                .is_err()
            {
                break 'session;
            }
            if check_reset_timeout(&mut timers, engine) {
                continue 'session;
            }
        }
        // A fresh reset Logon can already be buffered on the input stream.
        // Return to the output drain before reading it: pending replies must
        // be stamped before any reset, and must not wait for another event.
        // Continuing also preserves the terminating block's application-send
        // gate if this dispatch ended an unconfirmed reset.
        if engine.should_leave_loop() || engine.has_admin_output() {
            continue 'session;
        }

        timers.sync(engine);

        // Pick up next resend range from engine's queue
        if active_resend.is_none() {
            active_resend = engine.take_pending_resend();
        }
        if check_reset_timeout(&mut timers, engine) {
            continue 'session;
        }
        if engine.reset_ready(active_resend.is_none(), storage) {
            engine.start_reset_probe();
            continue 'session;
        }

        // --- Control check (non-blocking, highest priority) ---
        if let Ok(msg) = control_rx.try_recv() {
            engine.on_control(msg);
            check_reset_timeout(&mut timers, engine);
            continue;
        }

        // If the engine still has resend work pending, skip the select
        // entirely and iterate immediately to process the next batch.
        //
        // Rationale: during an active resend we have no reason to block
        // on the event select:
        //  - user app sends are already gated off the `app_rx.recv()`
        //    branch (sending new messages mid-resend would just provoke
        //    more ResendRequests from the peer);
        //  - the peer that requested the resend is busy consuming our
        //    resend stream, so it is unlikely to send new input to us
        //    before the range completes;
        //  - control messages (Logout, Disconnect) are already picked up
        //    by the control check above, so graceful shutdown still
        //    pre-empts a long resend;
        //  - outbound heartbeat / input-timeout deadlines do not need to
        //    fire while we are actively writing resend traffic - the
        //    peer sees that traffic as liveness.
        //
        // Without this shortcut the event select would block until
        // the output timer expires (one heartbeat interval), because
        // none of its other branches are reachable during resend.
        //
        // The same applies to the out-of-order queue: `queued_batch_size`
        // bounds how much of it the drain above clears per iteration, so with
        // more messages parked than that, blocking here would strand the rest
        // behind a closed gap. The peer's next in-sequence message would then
        // read as too-high and draw a ResendRequest for messages it has
        // already sent. Iterating instead keeps the batch cap doing what it is
        // for - bounding work per pass, not total work - and still runs the
        // top-of-loop flush and slow-consumer caps between batches.
        //
        // The condition asks whether the *next expected* seq num is parked,
        // not whether the queue is non-empty: messages sitting beyond a gap
        // that is still open are not processable, and spinning on them would
        // busy-loop the session.
        if active_resend.is_some()
            || engine.has_queued_message(storage.next_target_msg_seq_num().get())
        {
            continue;
        }

        // --- Event select ---
        // Fair select for all other events
        if check_reset_timeout(&mut timers, engine) {
            continue 'session;
        }
        tokio::select! {
            event = input.next() => {
                // A garbled message must be disregarded (FIX Session Layer
                // §4.5.2) and must NOT reset the keep-alive input deadline -
                // otherwise a garbled-only stream suppresses the input-timeout
                // TestRequest / heartbeat-timeout escalation forever (§4.5.1). A
                // well-framed message that merely fails validation (Reject /
                // Logout) was genuinely received and does reset it.
                let received_non_garbled = !matches!(
                    event,
                    Some(InputEvent::DeserializeError(DeserializeError {
                        kind: DeserializeErrorKind::Garbled(_),
                        ..
                    }))
                );
                if received_non_garbled {
                    timers.on_input_received();
                }
                match event {
                    Some(InputEvent::Message(msg)) => {
                        if check_reset_timeout(&mut timers, engine) {
                            continue 'session;
                        }
                        let result = match engine.on_input(msg, storage) {
                            Ok(result) => result,
                            Err(_) => break 'session,
                        };
                        if dispatch_input_result(result, engine, storage, app).await.is_err() {
                            break 'session;
                        }
                        check_reset_timeout(&mut timers, engine);
                    }
                    Some(InputEvent::DeserializeError(error)) => {
                        if check_reset_timeout(&mut timers, engine) {
                            continue 'session;
                        }
                        let result = match engine.on_deserialize_error(error, storage) {
                            Ok(result) => result,
                            Err(_) => break 'session,
                        };
                        if dispatch_input_result(result, engine, storage, app).await.is_err() {
                            break 'session;
                        }
                        check_reset_timeout(&mut timers, engine);
                    }
                    Some(InputEvent::TooLarge { frame_len }) => {
                        // The farewell Logout is staged here and written by
                        // the terminating block at the top of the next
                        // iteration, like every other engine-decided end.
                        engine.on_oversized_message(frame_len);
                    }
                    Some(InputEvent::IoError(err)) => {
                        error!(%err, "input I/O error");
                        engine.begin_disconnect(DisconnectReason::IoError);
                        break 'session;
                    }
                    None => {
                        info!("input stream closed");
                        engine.begin_disconnect(DisconnectReason::Disconnected);
                        break 'session;
                    }
                }
            }

            event = timers.next_event() => {
                match event {
                    TimerEvent::Input => engine.on_input_timeout(),
                    TimerEvent::Output => engine.on_output_timeout(),
                    TimerEvent::Logout => engine.on_logout_timeout(),
                    TimerEvent::Logon => {
                        warn!("no Logon response from peer, disconnecting");
                        engine.on_logon_timeout();
                    }
                    TimerEvent::Reset => {
                        check_reset_timeout(&mut timers, engine);
                    }
                }
                timers.on_timeout(event);
            }

            msg = async {
                if !engine.accepts_app_sends() {
                    // Reset holds messages in staging, but producers must
                    // still wake the loop to enforce its count cap.
                    app_rx.wait_for_send().await;
                    None
                } else {
                    app_rx.recv().await
                }
            }, if (engine.is_logged_on()
                    && !engine.accepts_app_sends()
                    && max_outbound_queued_messages.is_some())
                || (active_resend.is_none()
                    && !engine.has_pending_resends()
                    && engine.accepts_app_sends()) => {
                if let Some(msg) = msg {
                    if prepare_user_send(engine, app, storage, msg, SessionEngine::commit_send).is_err() {
                        break 'session;
                    }
                    processed_app_send = true;
                }
            }

            Some(msg) = control_rx.recv() => {
                engine.on_control(msg);
                check_reset_timeout(&mut timers, engine);
            }
        }

        // Single-threaded fairness: a non-blocking producer plus unbounded
        // staging means `recv()` can return `Ready` on every iteration during
        // a flood. Yield once after processing a staged message when more
        // remain, so other `LocalSet` tasks make progress between messages.
        // (Hoisted after the select! block - rather than inside the arm - to
        // keep the `app_rx` borrow unambiguous.)
        if processed_app_send && app_rx.len() > 0 {
            task::yield_now().await;
        }
    }
}
