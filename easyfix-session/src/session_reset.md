# Resetting session sequence numbers

Resetting starts a new FIX session: both sequence counters return to 1 and
the stored resend history is discarded. An ordinary reconnect preserves
that state instead (FIX Session Layer Section 4.1).

Choose the reset method and timing with the counterparty. Reset acceptance
is off by default; enabling acceptance never initiates a reset.

## Choosing a method

| Method | Local operation | Counterparty requirement |
|---|---|---|
| Both sessions inactive | `Acceptor::reset_session` or `Initiator::reset_session` on each side | Coordinate both offline resets before reconnecting |
| New connection | Initiator calls `connect_with_reset` | Acceptor enables `accept_reset_on_connect` |
| Active connection | Either side calls `request_running_session_reset` | Receiving side enables `accept_reset_in_session`; agree which side initiates |

The two acceptance settings are independent. `accept_reset_on_connect`
applies only to an acceptor; `accept_reset_in_session` applies to either
role. A peer that does not accept the requested method refuses with Logout
and closes the connection (Session Layer Sections 4.4.2-4.4.3).

An offline reset sends nothing and does not require `ResetSeqNumFlag(141)`.
After both sides reset, the initiator reconnects with `connect`
and sends an ordinary Logon numbered 1 (Session Test Cases Scenario 9).
Resetting only one side can leave the peers using incompatible numbering.

The other methods exchange Logons with `ResetSeqNumFlag(141)=Y` and
`MsgSeqNum(34)=1`. On success, both sides continue from sequence number 2
(Session Layer Sections 4.4.2-4.4.3). The local message implementation must
preserve tag 141. Otherwise the operation returns
`ResetSeqNumFlagNotSupportedInLogon` before starting the reset. Enabling an
applicable acceptance setting also requires this support; construction or
registration fails before `build_storage` is called. Ordinary sessions and
offline resets remain available without tag 141.

`connect`, `run_session` and `session_task` use the stored counters and
history. Their counterparts `connect_with_reset`, `run_session_with_reset`
and `session_task_with_reset` request a reset for that invocation only.
A subsequent ordinary call resumes from storage. Use a reset method on
every connection only when that is the counterparty agreement.

## Reset over an active connection

Agree the timing and initiating side first (Session Layer Section 4.4.2).
Simultaneous initiation by both sides is not supported. Either role can
request the reset:

```text
initiator.request_running_session_reset().await?;
acceptor.request_running_session_reset(&session_id).await?;
```

The call submits a request; `Ok(())` does not confirm execution or completion.
The session ignores a request if it is not established, is already resetting,
or is ending. Application sending pauses when the session begins processing
the request, rather than immediately when the method is called.

Preparation finishes any message recovery and waits for answers to
outstanding keep-alive probes. It then sends a TestRequest with its own
`TestReqID(112)` and waits for the matching Heartbeat. If recovery interrupts
this check, a fresh probe is required afterwards. This probe is sent even
with `HeartBtInt=0` and always requires the matching ID, regardless of
`verify_test_request_id` or the regular heartbeat probe limit.

Once preparation succeeds, the session resets its counters, discards resend
history and sends the reset Logon. Application sends and regular keep-alive
output remain held until acknowledgement. The heartbeat interval already
in force is retained, including zero; a peer's reset Logon proposing a
different interval is refused before resetting storage (Session Layer
Section 4.3.4).

The probe reduces collision risk but cannot guarantee that the peer stops
sending. Traffic racing with the reset Logon can end the connection with
`SeqNumResetFailed`, without implying a protocol violation by the peer.

## Outgoing messages

Queued outgoing application messages remain queued during the reset and
are sent in order with new sequence numbers after acknowledgement. Messages
already serialized and committed for transmission are written before the
session processes an input or control request that could reset storage.

The queue-count limit remains active while sends are held. The queue-age
limit excludes this interval and grants one full `max_outbound_lag` grace
window when sending resumes.

If the connection ends with history enabled and storage still usable,
finalization attempts to number and store remaining outgoing messages for
later recovery. Successful records remain available until reset. This can
advance counters after a reset or a failed attempt; do not assume they still
read 1 or 2 when the connection ends. With `persist_messages=false`, the
remaining queue is discarded without numbering or output callbacks. After a
fatal storage or replay error, finalization discards it without further
storage access or sending.

## Time limits

Preparation uses `running_session_reset_timeout` (30 seconds by default),
starting when the session accepts the request. Recovery and repeated probes
share that budget; repeated requests do not extend it. Expiry sends Logout
and ends the connection with `ResetPreparationTimeout`, before the local
reset has occurred. Ordinary processing and finalization may still advance
counters and store messages. Normal resumption from storage is available.

Acknowledgement uses `auto_disconnect_after_no_logon_response`, starting
when the reset Logon is queued. Writing the Logon spends that budget. If
the peer has not confirmed the reset when it expires, the connection ends
with `SeqNumResetFailed`. Preparation does not consume this separate budget.

A duration the clock cannot represent, such as `Duration::MAX`, removes the
corresponding limit. Deadline checks do not interrupt an ongoing write,
resend batch or incoming-message callback. Input processing started before
the deadline completes, including applying the callback's decision, before
the next check. These limits cannot cancel a stalled application callback.

## Application callbacks

A successful in-session reset uses the existing connection and `Sender`.
It does not repeat `on_session_ready` or call `on_session_end`. The incoming Logon is
delivered to `on_admin_msg_in`; failures that close the connection are
reported through `on_session_end`.

There are two different incoming Logons:

- **A peer's reset request:** the application sees it before the reset is
  applied. Refusing it leaves the previous numbering and history in place,
  apart from normal sequence consumption and outgoing replies.
- **Acknowledgement of our reset:** validation confirms the peer's
  renumbering before the callback. `Accept` completes the exchange. `Reject`
  sends Reject, then Logout unless already sent, and disconnects with
  `ApplicationForcedDisconnect`. `Logout` and `Disconnect` keep their normal
  termination behavior. None of these decisions undo the confirmation.

The session owns the Logon's numbering, reset flag, heartbeat interval and
next-expected sequence number. `on_admin_msg_out` may inspect those fields
but must not change them.

## Recovery after an unconfirmed reset

`SeqNumResetFailed` means the local counters were reset but execution by the
peer was not confirmed. Examples include a refusing Logout, invalid
acknowledgement, timeout, transport failure or an explicit disconnect before
confirmation. This reason takes precedence over the immediate cause because
the peers may now use incompatible numbering.

After the connection has fully ended, choose an agreed recovery method:

- **Initiator:** reconnect with `connect_with_reset` if the acceptor permits
  it, coordinate an offline reset on both sides, or rebuild the initiator
  with storage whose setters restore counters agreed with the peer in
  `build_storage`.
- **Acceptor:** wait for a reset Logon under `accept_reset_on_connect`,
  coordinate the same offline reset on both sides, or remove the inactive
  session and register storage with agreed counters.

Resetting only one side offline does not repair an unconfirmed exchange.
Coordinate both sides before resuming (Session Layer Section 4.1; Session
Test Cases Scenario 9). A `ResetPreparationTimeout` needs no such recovery:
the local reset did not happen. An application refusing a valid reset
acknowledgement also does not make the peer's numbering unconfirmed.

## Storage failures

A failed reset may have partially changed counters or history. Offline
`reset_session` returns `InitiatorError::Storage` or `AcceptorError::Storage`
with the original backend error as its source. A reset failure inside a
running task ends that connection through `on_session_end`; the session sends
no acknowledgement suggesting that the reset succeeded.

The backend/application must restore consistent state before reuse after an
uncertain mutation. Returning the storage object to its owner does not repair
or validate it, and reconnect is not automatically blocked. The session does
not retry, roll back a backend failure, or reset automatically. An earlier
disconnect reason remains authoritative; an unconfirmed local reset still
uses `SeqNumResetFailed` even if storage failure ends the connection.
