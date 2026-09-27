# ProxyLab WebSocket/SSE Context

Load this file for WebSocket, SSE, stream-message capture, message inspection/editing, or long-lived stream lifecycle work.

## WebSockets and Server-Sent Events

**Two protocols that break the same assumption, in two different places.** An
HTTP connection through this proxy is a request, a response, and then a
connection that is free again. A WebSocket stops being HTTP after its
handshake; an event stream is a response that does not end. Everything below
follows from those two sentences, and nothing else in the proxy changed to
accommodate them.

```
                     client request
                           |
                           v
                      rule engine
                           |
              +------------+------------+
              |                         |
      Upgrade: websocket           everything else
              |                         |
              v                         v
     connection of its own      pooled connection
     (never pooled)                     |
              |                         v
              v                  response head
        101 from origin                 |
              |                  +------+------+
              v                  |             |
   parse WebSocket frames    text/event-stream  ordinary body
   (forward original bytes)      |             |
              |                  +------+------+
              |                         |
              v                         v
     handshake captured         head announced; events
     plus bounded messages      parsed, capture bounded
```

### WebSocket

**Detection is RFC 6455's own list and nothing looser.** `GET`, HTTP/1.1,
`Connection: Upgrade`, `Upgrade: websocket`, a non-empty `Sec-WebSocket-Key`,
and `Sec-WebSocket-Version: 13`. `Connection: keep-alive` is not
`Connection: Upgrade`, and an upgrade to `h2c` is not a WebSocket; both are
carried as ordinary requests. Misreading one would hand an ordinary request's
connection to a tunnel and never answer it, which is why the check is exact.
`core/proxy-engine/src/websocket/detect.rs`.

**The hop-by-hop filter is what makes an upgrade need its own path.**
`Connection` and `Upgrade` are hop-by-hop, and stripping them is correct for
every other request and fatal for this one. They are put back deliberately, in
the proxy's own words rather than the client's, so the origin is asked for
exactly one thing. The `Sec-WebSocket-Key` is *not* touched: the origin
computes `Sec-WebSocket-Accept` from it and the client checks the answer, so a
proxy that rewrote either end of that would break the handshake it was
carrying.

**A WebSocket's upstream connection is opened for it alone and never pooled.**
`UpstreamManager::open_upgradable` is a separate door into the upstream
manager for exactly this reason: after the `101` the socket is not an HTTP
connection, and returning it to the pool would hand the next request somebody
else's WebSocket. It is never registered, never shared, and never returned.

**After the `101` the socket is still two streams of bytes, and the frames
on them are now named.** The tunnel still forwards what each side sent —
original frame bytes when the parse succeeds, the leftover bytes unchanged
when it does not — so re-framing cannot invent a boundary the two ends did
not agree on. Invalid or unframed bytes fall back to a copy of the remainder,
which is the same answer a tunnel that never parsed would have given, and
what the existing tests that send raw non-frames rely on. Control frames are
forwarded and not captured as messages. Text and binary messages are.

**The handshake is still the exchange.** The request as the client sent it
and the `101` as the origin answered it: one `TrafficExchange`, stored and
published as soon as the `101` is known so the inspector can open the socket
while it is still carrying frames. Each captured message hangs off that row
in `stream_messages` (schema v4): direction, text vs binary, timestamp, size,
and payload. Schema v5 copies request and response `Content-Type` onto the
exchange row so a type filter does not open header JSON for every scan.
Payloads use the same inline-or-file body placement HTTP bodies
already use. Capture stops at 10,000 messages per exchange and at the
ordinary body limit per payload; forwarding does not. A binary payload is
kept as the bytes that arrived. Text that is JSON is still stored as those
bytes; laying it out is a display decision in the inspector.

**A text WebSocket message can be held still**, using the same breakpoint
matcher and the same pause stream HTTP already has. The direction is
`websocket`; the handshake is not editable; binary messages are not. Continue
sends an optional payload replacement; Cancel drops that one message and
leaves the connection open.

**`wss://` is the same path inside an intercepted TLS session, and nothing
about it is weaker.** The client checks ProxyLab's certificate, ProxyLab
verifies the origin's against the same trust anchors with the same host name
check and the same SNI, and the only difference from any other upstream
session is that this one offers `http/1.1` alone over ALPN — the handshake has
no HTTP/2 form this proxy carries.

**Rules are evaluated before any of it.** A blocked WebSocket is answered
`403` and the origin is never contacted; Map Remote settles the destination
before the connection is opened, and the upgrade then proceeds there. No rule
type is WebSocket-specific.

### Server-Sent Events

**An event stream is still an ordinary response whose body does not end.**
The head is written to the client as it arrives, and the body is still a
passthrough: every frame the origin produces is handed on unchanged. What is
new is that the same tap that captures the HTTP body also parses SSE events
the way the existing stream already splits them — `event:`, `id:`, `data:`,
`retry:` — and stores each one on the parent exchange. Parsing is compatible
with that existing split; it does not invent a second SSE implementation.

**The exchange is announced when the response head is known**, so a live
stream can be selected while it is still open. The final publish upserts the
same row rather than inserting a second one, and does not emit a second
`traffic_added`. Capture of events is bounded the same way WebSocket messages
are: 10,000 per exchange and the ordinary body limit per payload. Past either
limit, events are forwarded and not stored. There is no client-to-server SSE
injection, and no buffering of the unbounded stream.

**SSE over HTTP/2 needs nothing of its own.** An HTTP/2 response body streams
through the same tap, and an HTTP/2 connection was never in a pool to be
held out of one.

### Stopping, with these open

**Waiting for every connection to finish stopped being a bounded wait.** The
accept loop used to wait for the connections in flight without limit, which
was correct while every one of them was an exchange that ends. A stream that
does not end makes that "stop when the origin feels like it", so the wait now
has an end: `Listener::SHUTDOWN_GRACE`. A connection still going at it is
dropped, which closes it on both sides, and the count of what was cut short is
reported. A WebSocket needs none of that grace — its tunnel selects on the
same `Shutdown` handle and ends at once — and nothing about an ordinary
request changed, because an ordinary request was already over.
