# ProxyLab Breakpoints and Throttling Context

Load this file for request/response breakpoints, WebSocket message pauses, body staging, or throttling.

## Breakpoints and throttling

```
                       ┌─────────────┐
 request ─────────────▶│  Controls   │────────────▶ upstream
                       │             │
                       │ breakpoints │  hold it still, and edit it
                       │ throttle    │  make the link slower than it is
                       └─────────────┘
 response ◀───────────────────────────────────────
```

`core/proxy-engine/src/controls/` holds both, and they are together because
they are the same kind of thing — live, deliberate interference with traffic
the proxy is carrying, turned on from the UI and off again — and because they
are threaded through the same places. One `Controls` is built when the proxy
starts, shared with the LAN listener, and handed to each connection the way
the capture and the rule engine are.

**Neither is a rule, and neither is persisted.** A rule is a standing decision
the core makes by itself on every matching request; a breakpoint decides
nothing — it stops, shows the exchange, and waits to be told. So they are
separate operations reaching a separate part of the server, and a ProxyLab
that has just started holds nothing still and carries traffic at the speed of
the network it is on. What the two share is the matcher: `RuleMatcher` already
means "this host, this path, this method", and a second one spelled the same
way would be two to keep in step.

### Nothing pauses unless somebody is watching

This is the whole of the disconnect story, and it is a property rather than a
timeout. A pause is created only while at least one client has asked to
receive them — `watch_breakpoints` — and the stream a client holds releases
every pending pause when the last one is dropped. A UI that crashes, is quit,
or loses its socket therefore cannot leave a request held: the connection
going away *is* what frees it, so there is no reconciliation to get wrong and
no state to repair on reconnection.

The case that property does not cover is a client that stays connected and
stops answering, and that is what `PAUSE_DEADLINE` is for: two minutes, after
which the exchange carries on unchanged. A stopping proxy releases everything
too, by the same route — the pause selects on the same `Shutdown` handle every
long-lived connection does.

### Where each half stops

A request breakpoint stops the request **before the rules are consulted**, and
deliberately: the request a developer edited is the request ProxyLab is
carrying, so it is the one the rules match against, the one the capture
records, and the one the origin receives. An edit that happened after the
rules had decided would be an edit the traffic list disagreed with.

A response breakpoint stops the response inside `into_client_response`, which
is the one place every route that answers a client from an origin passes
through. So one implementation covers HTTP/1.1, an HTTP/2 stream, a mapped
request's answer, and a WebSocket handshake the origin refused.

Cancelling produces a `502` carrying one fixed phrase, and the exchange is
still captured — with no response, and an error saying it was cancelled. A
request that vanished from the traffic list would be the proxy hiding what it
did.

A WebSocket text-message breakpoint stops one assembled text message before
it is written in its direction. Cancel drops that message; the socket stays
open. Binary messages and the handshake are not held.

### Reading a body without buffering the stream

`StagedBody` is what makes a breakpoint possible on a body that is a stream.
It reads frames up front until the body ends, until 32 KiB of it has arrived,
or until a two-second window passes — whichever is first — and then hands on
everything it read followed by everything it did not. Staging therefore
changes what the far end receives in no way at all.

The time limit is what makes a response breakpoint safe to put on a
server-sent event stream: reading stops when the window passes, the pause
appears with whatever had arrived, and the rest of the stream is delivered
after it rather than waited for. A body that hit either limit is *incomplete*,
and that is the whole of why an edit is refused for it — the bytes shown are
not all the bytes there are, so replacing them would silently throw the rest
away. Continuing without a body edit forwards everything, in full.

### What throttling shapes

Three fixed numbers and a switch: a delay added once per exchange, a ceiling
on the request body going upstream, and a ceiling on the response body coming
back. Nothing adapts, nothing is scheduled, and there are no presets.

`Throttled` holds each data frame back and hands it on in pieces sized from
the ceiling itself — about a twentieth of a second's worth — so a slow link is
a steady trickle rather than a stall and a burst. Frames that are not data
pass straight through. A WebSocket stops being a body at the `101`, so it is
paced by the same arithmetic applied to a socket instead of to a frame, and a
tunnel with no ceiling still runs `copy_bidirectional` exactly as it always
did.

The ceiling is kept **per body, not per proxy**: two downloads at 32 KiB/s get
32 KiB/s each. What a developer is simulating is one device's link to one
server, and a shared budget across every connection would behave like nothing
any real network does.

Bandwidth is not applied to a response ProxyLab generated itself — a Block
rule's refusal, a Map Local file — because those never crossed a network.
Added latency is, because the request did travel to the proxy.
