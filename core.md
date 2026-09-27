# ProxyLab Core Context

Load this file for Rust proxy-engine/core architecture, protocol flow, HTTP/1.1/HTTP/2, upstream pooling, or core/UI separation.

### Core

- **Technology:** Rust
- **Responsibility:** Proxy networking, HTTP/HTTPS handling, TLS interception,
  rules, capture, storage, and IPC.
- Runs and is testable headlessly, with no UI process present.
- Modules:
  | Path | Responsibility |
  | --- | --- |
  | `core/proxy-engine/` | TCP listeners, connection lifecycle, CONNECT tunneling, HTTP/1.1 and HTTP/2 framing, upstream connections |
  | `core/proxy-engine/src/lan/` | Which of this Mac's addresses a phone could reach, and the second listener bound to one |
  | `core/proxy-engine/src/controls/` | Exchanges held still while a developer edits them, and how slow the link pretends to be |
  | `core/proxy-engine/src/http1/setup.rs` | The one host the proxy answers as itself: where a device fetches the CA |
  | `core/proxy-engine/src/transparent/` | The loopback bridge the Network Extension hands captured flows to |
  | `core/tls/` | Root CA, dynamic leaf certificate generation, TLS interception, ALPN |
  | `core/http/` | HTTP message models: the canonical request a replay carries |
  | `core/rules/` | The rule model, matching, and the decision a request produces |
  | `core/storage/` | Captured request/response persistence, bodies, timing |
  | `core/curl/` | cURL export and import: a command read as a request, a request written as one |
  | `core/har/` | HAR 1.2 export and import: an exchange written as an entry, an entry read as one |
  | `core/ipc/` | IPC boundary: Unix socket server, message schema, event stream |

### Device tools

- **Responsibility:** Configuring clients to route through the proxy and trust
  the root CA.
- `device-tools/ios/SimulatorTools/` — iOS Simulator tooling, as a Swift
  package of its own. The macOS client depends on it by path; nothing in it
  depends on the client, and nothing in it knows the Rust core exists — it is
  handed a port and a certificate path.
- `device-tools/android/AndroidTools/` — Android Emulator tooling, a package
  of its own on the same terms. It deliberately does not depend on
  `SimulatorTools`, although the two share the shape of a process runner:
  neither device platform's support should be a reason to rebuild the other's,
  and the shared part is ninety lines of `Process` plumbing.
- A physical iPhone has no tooling here, and cannot: it is a separate device
  on the same Wi-Fi, and nothing on this Mac can reach into its Settings. What
  it needs instead lives in the core — see **The physical iPhone** below.
- Modules:
  | Path | Responsibility |
  | --- | --- |
  | `Sources/SimulatorTools/SimulatorCommand.swift` | Running `xcrun simctl`, and nothing else |
  | `Sources/SimulatorTools/SimulatorCatalog.swift` | Which iOS Simulators are booted |
  | `Sources/SimulatorTools/SimulatorTrustStore.swift` | Making one trust ProxyLab's CA |
  | `Sources/SimulatorTools/ProxyRoute.swift` | What a simulator will connect through, read and never written |
  | `Sources/AndroidTools/AdbCommand.swift` | Running `adb`, by argument vector and never through a shell |
  | `Sources/AndroidTools/AdbLocation.swift` | Finding `adb` without depending on how ProxyLab was started |
  | `Sources/AndroidTools/EmulatorCatalog.swift` | Which Android Emulators are running |
  | `Sources/AndroidTools/EmulatorProxy.swift` | The one setting ProxyLab changes, and how it is put back |
  | `Sources/AndroidTools/CertificateDelivery.swift` | Getting the CA onto the device, and what that does not buy |

## Intended data flow

```
Mac Browser / iOS Simulator / Android Emulator / Physical iPhone / any Mac app
        ↓                                         (Network Extension)
Proxy Engine
        ↓
HTTP / HTTPS / HTTP2
        ↓
Rule Engine
        ↓
Capture / Storage / Events
        ↓
IPC
        ↓
macOS UI
```

Traffic enters at the proxy engine, is decoded into protocol-level messages,
passes through rule evaluation (which may rewrite, redirect, or block it), is
captured into storage as request/response records with timing, and is surfaced
to the UI as an event stream across the IPC boundary.


## HTTP/1.1 and HTTP/2

**Which HTTP travels inside an intercepted connection is settled by ALPN, and
settled separately on each side of the proxy.** A client's TLS session with
ProxyLab agrees on one protocol; ProxyLab's TLS session with the origin agrees
on another. Both configurations offer `h2` and `http/1.1`, in that order of
preference, and neither forces anything: a peer that offers only `http/1.1`,
or offers no ALPN at all, is spoken to in HTTP/1.1.

```
                         ProxyLab
                            |
                     TCP / CONNECT
                            |
                         TLS MITM
                            |
                          ALPN
                            |
              +-------------+-------------+
              |                           |
             h2                       http/1.1
              |                           |
              v                           v
       HTTP/2 connection           HTTP/1.1 connection
              |                           |
       +------+------+                    |
       v      v      v                    v
    Stream 1 Stream 3 Stream 5       one exchange
       |      |      |                    |
       +------+------+--------------------+
                      |
                      v
              canonical request
                      |
                      v
                 rule engine
                      |
                      v
                   upstream
                      |
                  +---+----+
                  v        v
                 h2     HTTP/1.1
                  |        |
                  +---+----+
                      v
              capture -> storage -> events -> UI
```

**A stream is an exchange.** An HTTP/1.1 connection through this proxy carries
one; an HTTP/2 connection carries as many as the client opens, concurrently.
Each has its own `ExchangeId`, its own timing measured from when that stream's
request arrived, its own captured bodies, and its own rule evaluation — so one
stream can be blocked, or fail upstream, while its neighbours are carried. The
shared TLS connection's lifetime is never an exchange's duration.

**The two protocols converge immediately and share everything below the
framing.** There is one canonical outgoing request — origin form, HTTP/1.1
shape, hop-by-hop headers stripped, `Host` naming the server — and every route
through the proxy produces it. `core/proxy-engine/src/origin.rs` is the single
place that knows how that request is spelled for each protocol: for HTTP/2 it
moves `Host` into `:authority` on an absolute request target, and for HTTP/1.1
it leaves it as it stands. An HTTP/2 request arriving from a client is reduced
to that same canonical form first, with `:authority` becoming `Host`. Which
connection it then travels on is `core/proxy-engine/src/upstream/`'s answer —
see **Upstream connections** below. Nothing else in the proxy branches on
protocol.

**The headers HTTP/2 forbids are the headers a proxy must not forward
anyway.** `Connection`, `Keep-Alive`, `Proxy-Connection`, `Transfer-Encoding`,
`TE`, `Trailer` and `Upgrade` are removed by the one existing hop-by-hop
filter, used on requests going out and on responses going back to an HTTP/2
client. There is no second list.

**Nothing here implements HTTP/2.** The framing, HPACK, and flow control are
hyper's, through the `h2` crate it is built on; enabling HTTP/2 was a Cargo
feature on a dependency that was already present, not a second HTTP stack.
Flow control is left on and at its defaults.

**An HTTP/2 connection is told when the proxy is stopping.** It is long-lived
by design, so the accept loop does not wait for one: it hands every connection
a `Shutdown` handle, and an HTTP/2 connection that is still open when the
listener stops finishes its streams and starts no more.

**The captured model did not change.** `HttpVersion` already had `Http2`, and
an HTTP/2 exchange is stored in the same table, in the same columns, as every
other. No migration, no new column, and no HTTP/2-specific UI.


## Upstream connections

`core/proxy-engine/src/upstream/` owns every connection ProxyLab holds open to
an origin server. One `UpstreamManager` is built when the proxy starts and is
shared by everything that sends a request upstream — a forwarded plaintext
request, a request read out of an intercepted TLS session, a Map Remote
destination, and a replay.

```
                       UpstreamManager
                             │
               ┌─────────────┴─────────────┐
               ▼                           ▼
       HTTP/1.1 pool                HTTP/2 connections
  one exchange per connection    many streams per connection
  several connections per origin   one connection per origin
```

**The two protocols are not pooled the same way, because they are not the same
thing.** HTTP/1.1 carries one exchange at a time, so a connection is taken
*out* of the pool for the duration of a request and put back when its response
body has been read to the end; several connections to one origin exist side by
side. HTTP/2 carries as many concurrent streams as the origin allows, so one
connection per origin is registered and every request takes a clone of its
sender. They are kept in different places rather than keyed apart, and which
one a connection is was settled by ALPN when it was opened.

**This is what stops concurrent requests to an HTTP/1.1 origin from queuing.**
Four streams of an intercepted HTTP/2 connection whose origin speaks only
HTTP/1.1 find an empty pool, open four connections, and run side by side.

**The pool key is the origin, and for TLS the name that was verified**:
transport (plaintext or TLS), host, port, and — for TLS — the host sent in SNI
and checked against the certificate. A session established for one host is
never handed to a request for another, and a request a rule mapped somewhere
else keys on where it actually went rather than on the URL the client asked
for. The TLS *configuration* is not a field because it cannot vary: a manager
holds one `Interceptor` for its whole life, and a second configuration means a
second manager.

**A connection goes back only when it is fit to be reused**: the response body
read to its end, no `Connection: close`, no HTTP/1.0 response that did not ask
to be kept, and the HTTP library not already having closed it. A body that
broke off, or that nobody read, drops its connection instead.

**One retry, and only when the request provably never left.** A pooled
connection can be dead without anything local knowing, so requests are offered
with hyper's `try_send_request`, which hands the request *back* when it was
never written to the socket. That hand-back is the whole rule: a request that
came back is tried once more on a connection opened for it, and a request that
may have reached the origin fails the exchange instead. Nothing is retried on
the strength of its method being "idempotent".

**Bounded, and swept rather than timed.** At most eight idle connections per
origin and sixty-four in total, each dropped after forty-five seconds idle.
Expiry is swept on access — every checkout and release walks the pool, which
is small by construction — so there is no background task to leak or shut
down. Stopping the proxy closes every idle connection; connections a request
is still using end when that request does.

**Pooling is invisible to everything above it.** Rules are evaluated before a
connection is asked for, so a blocked or locally answered request never touches
the pool. Every request is still its own `TrafficExchange` with its own id,
bodies, and timing, and no connection identity appears anywhere in the captured
model or the database.


## Separation of proxy engine and macOS UI

**The proxy engine is intentionally separated from the macOS UI.** This is a
deliberate structural decision, not an implementation detail.

The reasons:

- **The core must be usable without a UI.** Headless operation, CI runs, and
  integration tests must not require launching an AppKit application.
- **Failure isolation.** A UI crash, hang, or restart must not drop live
  connections or lose captured traffic.
- **Testability.** Protocol correctness, TLS behaviour, and rule evaluation are
  testable in Rust without any UI harness.
- **Portability.** Nothing in the core assumes macOS presentation, so other
  front-ends remain possible without rewriting proxy logic.
- **Performance.** Connection handling and body capture run independently of
  the UI's render loop and are never blocked by it.

### The boundary

- Dependency direction is one-way: **the macOS UI depends on the core; the core
  never depends on Swift or macOS UI code.**
- All communication between UI and core crosses the IPC boundary defined in
  `core/ipc/`. There are no side channels.
- The IPC surface is a public interface. Changing its messages or data models
  is a breaking change and must be declared explicitly (see `AGENTS.md` rule
  15).
