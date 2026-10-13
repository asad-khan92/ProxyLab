//! One client, from the moment it connects to the moment it goes away.
//!
//! ```text
//!            ┌──────────────┐
//! client ───▶│              │───▶ request  ──▶ TrafficService
//!            │  connection  │◀─── response ◀──
//! client ◀───│              │◀─── event    ◀── EventSubscriber
//! client ◀───│              │◀─── pause    ◀── PauseStream
//!            └──────────────┘
//! ```
//!
//! A connection is one task waiting on four things at once: the next line
//! from its client, the next exchange from the traffic stream, the next
//! exchange a breakpoint is holding still, and the signal to stop. Whichever
//! happens first is dealt with, and then it waits again.
//!
//! # A client that goes away releases what it was holding
//!
//! The pause stream lives here, in the connection, and nowhere else. So a
//! client that disconnects — cleanly, by crashing, or by having its socket
//! broken — drops it as its task ends, and the last one dropped releases
//! every exchange that was being held still. There is no separate timeout to
//! get right and no state to reconcile: a pause cannot outlive the
//! connections that could have answered it.
//!
//! # Nothing here is shared with another client
//!
//! Everything a connection has — its buffer, its subscription, its socket —
//! belongs to that connection. A client that sends nonsense, subscribes and
//! stops reading, or vanishes mid-request affects itself and nothing else,
//! because there is nothing else to affect. The service it calls is a handle
//! to the database, and the database is where the sharing is.
//!
//! # A client that stops reading
//!
//! Writing to a socket nobody is reading eventually blocks, and this task
//! blocks with it. What must not happen is for that to reach the proxy, and
//! it cannot: the traffic stream is bounded and drops events for a subscriber
//! that is behind rather than waiting for it, so a stalled connection costs
//! its own task and a bounded amount of memory. When it reads again it is
//! told how many exchanges it missed.
//!
//! Shutdown is not held up by such a client either: the stop signal is
//! selected on around the write and around an operation that is still
//! running, so a server that is shutting down does not wait for a socket
//! that will never drain or for a replay that has not finished.
//!
//! # What is read
//!
//! One line is one request, up to [`MAX_REQUEST_BYTES`]. A longer line is
//! answered with an error and skipped to its end, which keeps the connection
//! usable without ever holding more than the limit: the server reads past the
//! rest of it without keeping any of it.

use std::io;

use proxy_storage::{EventSubscriber, ExchangeSummary, StreamError, StreamSubscriber};
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::watch;

use crate::breakpoint::{PauseNotice, PauseStream};
use crate::protocol::{
    self, ErrorBody, ErrorCode, Event, Operation, Outcome, Response, Subscribed, Watching,
    MAX_REQUEST_BYTES,
};
use crate::service::TrafficService;

/// Serves one client until it goes away or the server stops.
pub(crate) async fn serve(
    stream: UnixStream,
    service: TrafficService,
    mut stopping: watch::Receiver<bool>,
) {
    proxy_diagnostics::info("ipc", "connect", "connection", "a client connected");
    let (reader, writer) = stream.into_split();
    let mut connection = Connection {
        reader: BufReader::new(reader),
        writer,
        buffer: Vec::new(),
        subscriber: None,
        streams: None,
        pauses: None,
        service,
    };

    match connection.run(&mut stopping).await {
        Ok(()) => {
            proxy_diagnostics::info("ipc", "disconnect", "connection", "a client disconnected");
        }
        // A client that hangs up mid-request is ordinary. Anything else is
        // worth knowing about, and carries no traffic — an I/O error names a
        // syscall, not what was being written.
        Err(error) if is_disconnect(&error) => {
            proxy_diagnostics::info("ipc", "disconnect", "connection", "a client disconnected");
        }
        Err(error) => {
            proxy_diagnostics::warn(
                "ipc",
                "disconnect",
                "connection",
                &format!("a client connection ended: {error}"),
            );
        }
    }
}

/// Everything one client has.
struct Connection {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    /// What has been read of the line in progress.
    ///
    /// Held across waits rather than inside the read, so a read that is
    /// interrupted by an event keeps the part of the line it already has.
    buffer: Vec<u8>,
    subscriber: Option<EventSubscriber>,
    /// Captured WebSocket messages and SSE events, when this client asked
    /// for traffic events. The same subscribe attaches both: a client that
    /// is watching traffic is the one that shows a message timeline.
    streams: Option<StreamSubscriber>,
    /// The exchanges breakpoints are holding still, when this client asked
    /// for them.
    ///
    /// Held here and nowhere else, which is what makes a client going away
    /// release whatever it was holding: this connection's task ending drops
    /// the stream, and the last stream to be dropped frees every pending
    /// pause. See [`PauseStream`].
    pauses: Option<PauseStream>,
    service: TrafficService,
}

/// What happened while waiting.
///
/// One value of this exists at a time, on the stack, and is matched on
/// immediately. Boxing the large variants to even them up would trade that
/// for an allocation per message received.
#[allow(clippy::large_enum_variant)]
enum Step {
    /// The server is stopping.
    Stop,
    /// A complete line arrived.
    Message(Vec<u8>),
    /// A line longer than the limit arrived and was skipped.
    Oversized,
    /// The client hung up.
    Closed,
    /// The proxy finished an exchange.
    Exchange(ExchangeSummary),
    /// This client missed some exchanges.
    Missed(u64),
    /// The traffic stream has ended, which means the proxy is going away.
    StreamClosed,
    /// A WebSocket message or SSE event was captured.
    StreamMessage(proxy_storage::StreamMessageSummary),
    /// This client missed some stream messages.
    StreamMissed(u64),
    /// The stream-message publisher has ended.
    StreamMessagesClosed,
    /// A breakpoint paused an exchange, or stopped holding one.
    Pause(PauseNotice),
    /// This client missed some pauses.
    ///
    /// Nothing is sent for it: a pause that was missed is one this client
    /// will never be shown, and the exchange it was holding is released when
    /// its deadline passes or when this client goes away. Saying so would be
    /// telling a client about an exchange it cannot act on.
    PausesMissed,
    /// The pause stream has ended, which means the proxy is going away.
    PauseStreamClosed,
}

impl Connection {
    /// Reads, answers, and forwards until there is nothing left to do.
    async fn run(&mut self, stopping: &mut watch::Receiver<bool>) -> io::Result<()> {
        loop {
            let subscribed = self.subscriber.is_some();

            // Every branch produces a value and changes nothing, so the
            // futures are all dropped before anything is acted on. That is
            // what lets the handler take the subscription — which one of the
            // branches was borrowing — and replace it.
            let step = tokio::select! {
                biased;
                () = stopped(stopping) => Step::Stop,
                line = read_line(&mut self.reader, &mut self.buffer) => line?,
                event = next_event(&mut self.subscriber) => event,
                stream = next_stream(&mut self.streams) => stream,
                pause = next_pause(&mut self.pauses) => pause,
            };

            match step {
                Step::Stop | Step::Closed => return Ok(()),
                Step::Message(line) => match self.answer(line, subscribed) {
                    // A blank line: nothing was asked, so nothing is said.
                    None => {}
                    Some(Answer::Response(response)) => self.send(stopping, &response).await?,
                    Some(Answer::Traffic(id, operation)) => {
                        let result =
                            unless_stopping(stopping, self.service.execute(operation)).await;
                        self.reply(stopping, &id, result).await?;
                    }
                    Some(Answer::Rule(id, operation)) => {
                        let result =
                            unless_stopping(stopping, self.service.execute_rules(operation)).await;
                        self.reply(stopping, &id, result).await?;
                    }
                    Some(Answer::Lan(id, operation)) => {
                        // Awaited here, like a replay: binding or closing a
                        // listening socket is quick, and no other connection
                        // is held up by this one waiting for it. Shutdown
                        // still wins, so a bind that has not returned cannot
                        // keep a stopping server alive.
                        let result =
                            unless_stopping(stopping, self.service.execute_lan(operation)).await;
                        self.reply(stopping, &id, result).await?;
                    }
                    Some(Answer::Breakpoint(id, operation)) => {
                        let result =
                            unless_stopping(stopping, self.service.execute_breakpoints(operation))
                                .await;
                        self.reply(stopping, &id, result).await?;
                    }
                    Some(Answer::Throttle(id, operation)) => {
                        let result =
                            unless_stopping(stopping, self.service.execute_throttle(operation))
                                .await;
                        self.reply(stopping, &id, result).await?;
                    }
                    Some(Answer::Saved(id, operation)) => {
                        let result =
                            unless_stopping(stopping, self.service.execute_saved(operation)).await;
                        self.reply(stopping, &id, result).await?;
                    }
                    Some(Answer::Diagnostics(id)) => {
                        let result =
                            unless_stopping(stopping, self.service.execute_diagnostics()).await;
                        self.reply(stopping, &id, result).await?;
                    }
                    Some(Answer::Har(id, operation)) => {
                        // A HAR export or import is a file of tens of
                        // megabytes and a walk through the traffic store, so
                        // it is the longest-running operation this server
                        // has. It runs on a blocking thread like every other
                        // database call, and this client waits for its own
                        // answer while no other connection is held up.
                        // Stopping does not wait for it to finish: the
                        // blocking work may still run, but the server does
                        // not stay up for it.
                        let result =
                            unless_stopping(stopping, self.service.execute_har(operation)).await;
                        self.reply(stopping, &id, result).await?;
                    }
                    Some(Answer::Developer(id, operation)) => {
                        // A replay is awaited here, which means this client
                        // is not reading its own socket while its request is
                        // in flight. That is this client's own business: no
                        // other connection is held up, and the traffic
                        // events it is subscribed to are queued for it the
                        // same way they are for a client that is slow for
                        // any other reason. Shutdown abandons the wait, so
                        // a destination that never answers cannot keep the
                        // server from stopping.
                        let result =
                            unless_stopping(stopping, self.service.execute_developer(operation))
                                .await;
                        self.reply(stopping, &id, result).await?;
                    }
                },
                Step::Oversized => {
                    let response = Response::failed(
                        None,
                        ErrorBody::new(
                            ErrorCode::MessageTooLarge,
                            "a request must be no longer than 65536 bytes",
                        ),
                    );
                    self.send(stopping, &response).await?;
                }
                Step::Exchange(summary) => {
                    self.send(stopping, &Event::traffic_added(summary)).await?;
                }
                Step::Missed(missed) => {
                    self.send(stopping, &Event::traffic_lost(missed)).await?;
                }
                // The proxy is gone, so there will be no more traffic. The
                // connection stays up: a client may still read the history.
                Step::StreamClosed => self.subscriber = None,
                Step::StreamMessage(summary) => {
                    self.send(stopping, &Event::stream_message_added(summary))
                        .await?;
                }
                Step::StreamMissed(missed) => {
                    self.send(stopping, &Event::stream_messages_lost(missed))
                        .await?;
                }
                Step::StreamMessagesClosed => self.streams = None,
                Step::Pause(PauseNotice::Paused(paused)) => {
                    self.send(stopping, &Event::breakpoint_paused(paused))
                        .await?;
                }
                Step::Pause(PauseNotice::Released { pause_id, reason }) => {
                    self.send(stopping, &Event::breakpoint_released(pause_id, reason))
                        .await?;
                }
                Step::PausesMissed => {}
                // The proxy is gone, so nothing will be paused again. Dropping
                // the stream is also what stops this client being counted as
                // watching, which is right: there is nothing left to watch.
                Step::PauseStreamClosed => self.pauses = None,
            }
        }
    }

    /// Works out the answer to one line, taking the subscription if it asked
    /// for one.
    ///
    /// `None` means there is nothing to send: a blank line is a client's
    /// keepalive, not a request, and correlating a response to it would be
    /// answering something nobody asked.
    fn answer(&mut self, line: Vec<u8>, subscribed: bool) -> Option<Answer> {
        let watching = self.pauses.is_some();

        if line.iter().all(u8::is_ascii_whitespace) {
            return None;
        }

        let request = match protocol::parse(&line) {
            Ok(request) => request,
            Err(failure) => {
                return Some(Answer::Response(Response::failed(
                    failure.request_id.as_ref(),
                    failure.error,
                )))
            }
        };

        match request.operation {
            Operation::Subscribe if subscribed => Some(Answer::Response(Response::failed(
                Some(&request.id),
                ErrorBody::new(
                    ErrorCode::AlreadySubscribed,
                    "this connection is already receiving traffic events",
                ),
            ))),
            Operation::Subscribe => {
                // Subscribed before the answer is written, so an exchange
                // that happens between the two is one this client receives:
                // a client that has been told it is subscribed has been.
                self.subscriber = Some(self.service.subscribe());
                self.streams = Some(self.service.subscribe_streams());

                Some(Answer::Response(Response::ok(
                    &request.id,
                    Outcome::Subscribe(Subscribed { subscribed: true }),
                )))
            }
            Operation::WatchBreakpoints if watching => Some(Answer::Response(Response::failed(
                Some(&request.id),
                ErrorBody::new(
                    ErrorCode::AlreadySubscribed,
                    "this connection is already receiving paused exchanges",
                ),
            ))),
            Operation::WatchBreakpoints => match self.service.watch_breakpoints() {
                // Attached before the answer is written, so an exchange
                // paused between the two is one this client receives. It is
                // also the moment the proxy starts pausing at all, which is
                // why nothing may be answered before it.
                Some(stream) => {
                    self.pauses = Some(stream);

                    Some(Answer::Response(Response::ok(
                        &request.id,
                        Outcome::WatchBreakpoints(Watching { watching: true }),
                    )))
                }
                None => Some(Answer::Response(Response::failed(
                    Some(&request.id),
                    ErrorBody::new(
                        ErrorCode::Internal,
                        "this server is not managing breakpoints",
                    ),
                ))),
            },
            Operation::Traffic(operation) => Some(Answer::Traffic(request.id, operation)),
            Operation::Rule(operation) => Some(Answer::Rule(request.id, operation)),
            Operation::Developer(operation) => Some(Answer::Developer(request.id, operation)),
            Operation::Lan(operation) => Some(Answer::Lan(request.id, operation)),
            Operation::Breakpoint(operation) => Some(Answer::Breakpoint(request.id, operation)),
            Operation::Throttle(operation) => Some(Answer::Throttle(request.id, operation)),
            Operation::Saved(operation) => Some(Answer::Saved(request.id, operation)),
            Operation::Har(operation) => Some(Answer::Har(request.id, operation)),
            Operation::Diagnostics => Some(Answer::Diagnostics(request.id)),
        }
    }

    /// Writes the answer of an operation that has already been run.
    ///
    /// `None` means shutdown won, and this client is dropped without a
    /// reply: the server is going away. The operation is awaited before
    /// this is called, so the future is not held across the write.
    async fn reply(
        &mut self,
        stopping: &mut watch::Receiver<bool>,
        id: &protocol::RequestId,
        result: Option<Result<Outcome, ErrorBody>>,
    ) -> io::Result<()> {
        let Some(result) = result else {
            return Ok(());
        };
        let response = match result {
            Ok(outcome) => Response::ok(id, outcome),
            Err(error) => Response::failed(Some(id), error),
        };

        self.send(stopping, &response).await
    }

    /// Writes one message, giving up if the server is stopping.
    async fn send<T: Serialize>(
        &mut self,
        stopping: &mut watch::Receiver<bool>,
        message: &T,
    ) -> io::Result<()> {
        let mut line = serde_json::to_vec(message)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        line.push(b'\n');

        tokio::select! {
            () = stopped(stopping) => Ok(()),
            written = self.writer.write_all(&line) => {
                written?;
                self.writer.flush().await
            }
        }
    }
}

/// What answering a line came to.
///
/// Short-lived, like [`Step`], and for the same reason left as it is.
#[allow(clippy::large_enum_variant)]
enum Answer {
    /// A response that is ready to send.
    Response(Response),
    /// An operation that has to go to storage first.
    Traffic(protocol::RequestId, protocol::TrafficOperation),
    /// An operation that changes, or asks about, the rules.
    Rule(protocol::RequestId, protocol::RuleOperation),
    /// An operation about cURL, or one that sends a request again.
    Developer(protocol::RequestId, protocol::DeveloperOperation),
    /// An operation about whether the proxy is also on the local network.
    Lan(protocol::RequestId, protocol::LanOperation),
    /// An operation about the breakpoints, or about an exchange one is
    /// holding still.
    Breakpoint(protocol::RequestId, protocol::BreakpointOperation),
    /// An operation about how the proxy is shaping the traffic it carries.
    Throttle(protocol::RequestId, protocol::ThrottleOperation),
    /// An operation about the requests somebody has kept.
    Saved(protocol::RequestId, protocol::SavedRequestOperation),
    /// An operation that writes a HAR file, or reads one.
    Har(protocol::RequestId, protocol::HarOperation),
    /// The process's own status.
    Diagnostics(protocol::RequestId),
}

/// Runs `work` unless the server stops first.
///
/// Dropping the future is how a replay, a query, or a bind is abandoned.
/// Work that already moved onto a blocking thread keeps running there; what
/// this guarantees is that the connection task, and therefore the server,
/// does not wait for it.
async fn unless_stopping<T>(
    stopping: &mut watch::Receiver<bool>,
    work: impl std::future::Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        biased;
        () = stopped(stopping) => None,
        result = work => Some(result),
    }
}

/// Resolves when the server is stopping.
///
/// The current value is checked before anything is awaited, so this is
/// answered the same way however many times it is asked. That matters
/// because it is asked twice for one stop — once while waiting for the
/// client and once around the write that follows — and a version that only
/// waited for the *next* change would consume the stop in the first of those
/// and then wait forever in the second.
async fn stopped(stopping: &mut watch::Receiver<bool>) {
    while !*stopping.borrow() {
        // A sender that has been dropped is a server that is gone, which is
        // a reason to stop rather than a reason to keep waiting.
        if stopping.changed().await.is_err() {
            return;
        }
    }
}

/// Waits for the next exchange, or forever if this client is not subscribed.
///
/// Cancel safe: the subscriber is not consumed by a wait that is interrupted,
/// so the events it has queued are still there next time.
async fn next_event(subscriber: &mut Option<EventSubscriber>) -> Step {
    let Some(subscriber) = subscriber else {
        // Nothing to wait for, so wait for nothing: the other branches of the
        // select decide what happens.
        return std::future::pending().await;
    };

    match subscriber.recv().await {
        Ok(exchange) => Step::Exchange(ExchangeSummary::of(&exchange)),
        Err(StreamError::Lagged(missed)) => Step::Missed(missed),
        Err(StreamError::Closed) => Step::StreamClosed,
    }
}

/// Waits for the next stream message, or forever if this client is not
/// subscribed.
async fn next_stream(subscriber: &mut Option<StreamSubscriber>) -> Step {
    let Some(subscriber) = subscriber else {
        return std::future::pending().await;
    };

    match subscriber.recv().await {
        Ok(summary) => Step::StreamMessage((*summary).clone()),
        Err(StreamError::Lagged(missed)) => Step::StreamMissed(missed),
        Err(StreamError::Closed) => Step::StreamMessagesClosed,
    }
}

/// Waits for the next pause, or forever if this client is not watching.
///
/// Cancel safe, for the same reason [`next_event`] is: the stream is not
/// consumed by a wait that is interrupted.
async fn next_pause(pauses: &mut Option<PauseStream>) -> Step {
    let Some(pauses) = pauses else {
        return std::future::pending().await;
    };

    match pauses.recv().await {
        Ok(notice) => Step::Pause(notice),
        Err(StreamError::Lagged(_)) => Step::PausesMissed,
        Err(StreamError::Closed) => Step::PauseStreamClosed,
    }
}

/// Reads one line, up to the size limit.
///
/// Cancel safe: what has been read of an unfinished line stays in `buffer`,
/// so a read interrupted by an event carries on where it left off.
///
/// A line over the limit is not buffered. It is read past and dropped, and
/// reported as [`Step::Oversized`], so a client that sends a megabyte costs
/// the server the time to read it and none of the memory to hold it.
async fn read_line(
    reader: &mut BufReader<OwnedReadHalf>,
    buffer: &mut Vec<u8>,
) -> io::Result<Step> {
    let mut oversized = buffer.len() > MAX_REQUEST_BYTES;

    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            buffer.clear();

            return Ok(Step::Closed);
        }

        let (chunk, complete) = match available.iter().position(|byte| *byte == b'\n') {
            Some(end) => (&available[..end], true),
            None => (available, false),
        };
        let consumed = chunk.len() + usize::from(complete);

        if !oversized {
            buffer.extend_from_slice(chunk);
            oversized = buffer.len() > MAX_REQUEST_BYTES;
            if oversized {
                // Nothing of an oversized line is kept, including the part
                // already read: it is not going to be parsed.
                buffer.clear();
            }
        }
        reader.consume(consumed);

        if complete {
            return Ok(if oversized {
                Step::Oversized
            } else {
                Step::Message(std::mem::take(buffer))
            });
        }
    }
}

/// Whether an error is just the client having gone away.
fn is_disconnect(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::UnexpectedEof
    )
}
