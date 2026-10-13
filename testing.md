# ProxyLab Testing Context

Load this file when deciding what tests to add/run, test boundaries, integration coverage, or manual-vs-automated verification.

## Testing

- Unit tests live beside the module they cover, inside `core/`.
- Cross-module and end-to-end tests live in `tests/integration/`.
- Pinned domains are tested three ways: the rules a pin follows against an
  in-memory storage double, the storage that actually ships against one
  preference domain of its own that is emptied before and after every test, and
  the navigation against the traffic model — that selecting a domain produces
  the same `list_traffic` the Host menu does, and composes with search, method,
  status, sort and paging rather than replacing any of them. One suite runs the
  same assertions against a running core, and is skipped unless
  `PROXYLAB_IPC_SOCKET` names one; the one test there that empties a traffic
  store needs `PROXYLAB_TEST_ALLOW_CLEAR=1` as well, so pointing the suite at a
  developer's own core cannot lose their traffic.
- The macOS client's tests are in `macos/ProxyLab/Tests/` and run with
  `swift test`. They need nothing installed and nothing running: the socket
  sits behind a protocol, so the protocol, the models, the client, and the
  view model are all tested without one, and the real transport is tested
  against a Unix socket the suite binds itself. One suite does talk to a
  running core, and is skipped unless `PROXYLAB_IPC_SOCKET` names one.
- The rules screen is tested the same way, and the two things that are not a
  socket are behind protocols for the same reason: `RulesSession` stands in for
  the core, and `LocalFileChooser` stands in for `NSOpenPanel`, so a cancelled
  pick, a file that is not there, and a folder chosen instead of a file are all
  cases a test can write.
- The rules views are rendered offscreen with `ImageRenderer` rather than
  driven by UI automation. A body that traps or lays out to nothing fails when
  it is drawn, and drawing it needs no window and nobody watching. They are
  smoke tests and deliberately not snapshot comparisons: a test that fails
  because a font shipped with a new macOS is a test that gets deleted.
- Transformations are tested in three layers. The ordering — a
  transformation above the decision applied, one below it not, an `Allow`
  above one exempting it, the higher of two conflicting ones winning, ties
  broken by age — and the JSON the new actions are written as are tested in
  `core/rules/tests/transformations.rs`, with the pure edits (query, path,
  JSON field) beside them in `rules/src/modify.rs`. Applying them to hyper
  messages is tested in `proxy-engine/src/modify.rs`. And
  `core/proxy-engine/tests/rule_transformations.rs` runs real listeners and
  origins: method, path, query, headers and body reaching the origin changed,
  the capture recording what was sent and what was given, a redirect never
  contacting the origin, a blocked request recorded unchanged, a transformed
  request mapped elsewhere with its changes, HTTPS, an HTTP/2 stream, a
  transparently captured flow, a replay, a response breakpoint showing the
  transformed response, and a request edited at a breakpoint being the one the
  rules change. `websocket.rs` asserts a handshake is never transformed; the
  IPC and storage suites assert the new actions are created, refused by name,
  and survive being stored.
- The cURL parser, the serializer, and the round trip between them are tested
  in `core/curl/` with no proxy, no socket, and no network: a command goes in
  and the request it describes comes out. Two of those tests assert that
  parsing a command whose effect would be a file on disk leaves no file on
  disk, and one reads the sources of every crate on the replay path and fails
  if a `Command` appears in any of them.
- HAR is tested in `core/har/` the same way, with no proxy, no socket and no
  database: an exchange goes in and an entry comes out, and back. What is
  asserted is what another tool would look for — every field HAR 1.2 requires
  written, the timings adding up to the duration, a body that is not text
  written as base64 and read back byte for byte, and a truncated capture
  coming back truncated with the size it had. The two date functions are
  checked against each other for every day of a whole Gregorian cycle, since
  a calendar written by hand is either exact or wrong. Reading is tested
  against a document another tool would write — no timings, a version spelled
  `h2`, fields this build has never heard of — because that is the usual case,
  and against a file that is not a HAR document at all, which is refused
  without its contents reaching the message.
- Saved requests are tested in three places for the three things they are.
  What one *is* — a name, a body that may be less than the request sent, an
  edit that replaces the body wholesale — is tested in `core/storage/` beside
  the model, with no database; that they survive being stored, renamed,
  edited and removed, with their bodies inline or in files and untouched by
  the traffic being cleared, is tested against a real SQLite file in a
  temporary directory; and the six operations are tested in `core/ipc/` over
  a real socket, including the refusals — a URL this proxy could not send to,
  a header that could split a request in two, a name that is not one.
- The join is tested in `core/proxy-engine/tests/har_session.rs`, against a
  real listener, a real interception, a real database, a real socket, a real
  file, and a real origin. Traffic the proxy actually carried is exported,
  the file is checked field by field against what HAR 1.2 requires, the
  history is cleared, the file is read back, and the exchanges are inspected
  through the same traffic operations the inspector uses. A captured request
  is saved, renamed, replayed — reaching the origin, credential included —
  edited, replayed again, and deleted. One test asserts the thing an import
  must never do: the origin the entries name is not contacted at all.
- Local network access is tested in
  `core/proxy-engine/tests/lan_exposure.rs` against a real second listener, a
  real IPC socket, and an address this machine actually has — there is no way
  to fake a bind. The switch is driven over the socket by hand, as a client
  would; what is asserted is that a fresh proxy is on loopback alone, that
  `enable_lan` produces a socket that answers at the chosen address, that a
  rule set on the loopback listener applies to LAN traffic too, that
  `disable_lan` leaves nothing listening and loopback still working, and that
  loopback, `0.0.0.0`, a routable address and an address this Mac does not
  have are each refused without changing anything. A machine with no usable
  LAN address has nothing to bind, and those tests return rather than fail:
  what they would be reporting is the network, not the code. The address
  filtering itself is a pure function over `(name, flags, address)` and is
  tested beside it with no network at all.
- HTTP/2 is tested in `core/proxy-engine/tests/http2_interception.rs` against
  a real listener, a real `CONNECT`, real ALPN on both sides, and in-process
  HTTP/2 clients and origins on OS-assigned ports. All four protocol
  combinations are covered, because the two sides are decided separately. The
  HTTP/2 ends of the harness use the HTTP library rather than writing frames by
  hand — hand-rolling HPACK to check HPACK proves nothing — while the HTTP/1.1
  ends still write bytes by hand, and each end offers over ALPN only the
  protocol it can actually speak. ALPN negotiation itself is tested in
  `core/tls/tests/tls_sessions.rs`, from both ends of one handshake.
- Upstream connection pooling is tested in
  `core/proxy-engine/tests/upstream_pooling.rs` against an instrumented origin
  that keeps its connections open and counts: how many were opened, which
  requests came down which one, and how many it was answering at once. That is
  the only honest way to test a pool, since a proxy that opens a fresh
  connection per request looks identical from the client's side. Concurrency is
  proved with a barrier inside the origin rather than with a stopwatch — no
  request is answered until the expected number are waiting at it, so a proxy
  that serialised them fails on the timeout rather than on a measurement that
  happened to be fast enough. Nothing in the suite sleeps to decide whether a
  test passed.
- Replay is tested in `core/proxy-engine/` against real in-process origin
  servers — plaintext and TLS — on OS-assigned ports, through the same
  `Replayer` the IPC boundary reaches. What is asserted is that a replay is
  ordinary traffic: the rules decide it, the capture records it under the same
  limits, storage keeps it as a new row, and the exchange it was started from
  is byte for byte unchanged.
- The three IPC operations are tested in `core/ipc/` over a real socket, with
  the proxy stood in for by a recording replayer — the same boundary the engine
  implements. Nothing there opens a connection, because what is under test is
  the bytes on the wire.
- Breakpoints are tested in both halves, and the split is the same one. In
  `core/ipc/` a recording `BreakpointControl` stands in for the proxy, and what
  is proved is the wire: the operations, the two events that answer nothing,
  the refusals, and that the last client disconnecting is what makes the core
  let go. In `core/proxy-engine/` there is no stand-in at all — a real
  listener, a real origin, and a client in a task of its own, because a paused
  request is a client waiting for an answer that is not coming yet. What is
  proved there is that the edited request is what the origin receives, that a
  cancelled one never reaches it, and that dropping the watcher frees what was
  held. Throttling is tested against the same real proxy, with every timing
  assertion written as a floor rather than a window: a loaded machine makes a
  throttled transfer slower and never faster.
- The detail pane is tested in three layers. What a body *is* — its media
  type, its character set, whether it is JSON, an image, or bytes that are not
  text — is decided by `BodyInspection`, away from any view, so the decision is
  tested as a decision. The pane itself is rendered offscreen with
  `ImageRenderer` against the awkward exchanges: no response, a blocked
  request, a Map Local answer, mixed protocol versions, an unstandardised
  status code, a body that will not decode, an image that will not draw, and a
  capture at the limit. One suite runs the same reading against a running core
  over its own socket — every stored exchange fetched, read, and drawn — and is
  skipped unless `PROXYLAB_IPC_SOCKET` names one. Nothing in any of them prints
  a header, a body, or a URL.
- The saved requests interface is tested without a socket and without a
  panel: `SavedRequestsSession` stands in for the core, and `HarFileChooser`
  stands in for `NSSavePanel` and `NSOpenPanel`, so a test run never asks for
  a panel and never writes a file. What is asserted is the request that was
  made and what was shown afterwards — that a rename sends the name and
  nothing else, that an edit sent to be replayed is not written down, that a
  request the core refused leaves the list alone, and that a cancelled panel
  sends nothing and says nothing. The two new views are rendered offscreen
  with `ImageRenderer`, like the rules views and for the same reason.
- The interface's half is tested without a clipboard and without a core:
  `Clipboard` stands in for `NSPasteboard`, so a test run never replaces what
  the developer had copied; `BodyDestination` stands in for `NSSavePanel`, so a
  test run never asks for a panel and never leaves a file behind; and
  `TrafficSession` stands in for the socket. The
  import sheet is rendered offscreen with `ImageRenderer`, like the rules
  views and for the same reason.
- The iOS Simulator tooling is tested in its own package, with `simctl` stood
  in for by a runner that records what it was asked to run. So what is asserted
  is the command — `keychain <udid> add-root-cert <path>`, and nothing beside
  it — and the decisions above it: that a non-iOS runtime is not an iOS
  Simulator, that a device which is not booted is not listed, that a proxy
  configured and switched off is not a proxy, and that half a route does not
  reach one. The device-list fixture is real `simctl list devices booted -j`
  output rather than something invented, and the proxy dictionaries are the
  shape `SCDynamicStoreCopyProxies` actually answers with. One suite runs
  against the real `xcrun simctl` and this Mac's real configuration, and is
  skipped unless `PROXYLAB_TEST_SIMULATOR=1` names it as wanted; everything in
  it reads, so a test run never boots, erases, or changes a simulator. The
  session model — which is where "the core is not running" and "the CA is
  trusted but nothing routes this simulator" become sentences — is tested in
  the macOS package against the same doubles, and the section itself is
  rendered offscreen with `ImageRenderer` like the other views. The compact
  capture-source rows are a presentation of those models and of capture's
  phase: the words on the row, a LAN address that follows the model, and a
  short sidebar drawn offscreen. They do not change what Connect does.
- App-owned capture is tested in the macOS package without a bundled engine
  and without installing an extension. The process, the readiness check, the
  system proxy, and macOS are stand-ins. What is asserted is the order:
  launch, then readiness, then certificate trust, then the system proxy, then
  transparent capture; the proxy is not configured when the listener is not
  ready or trust is not; Stop and quit restore it and require the live route
  to agree before stopping the owned process; a cancelled or unverified
  restore leaves that process running and refuses quit; a crash restores it
  and, when the restore cannot be verified, keeps quit blocked; a proxy left
  behind by a dead session is put back after the listener is ready, and a
  restore that does not land leaves that listener running; a proxy someone
  else changed is not overwritten; a port that is already taken is not killed; approval does not hang or submit a second activation;
  an installed extension starts without one; and the toolbar and This Mac
  read the same status. Certificate trust is tested against a stand-in
  keychain. Those tests write certificates only in a temporary directory;
  they do not install a certificate into this Mac's keychains. A simulator
  told to trust the same file does not make the Mac trust check succeed.
  Which service is primary is decided from a service
  id, and a service named Wi-Fi is not selected for its name. LAN address
  changes are decided the same way: the old listener is replaced, a missing
  address is not advertised, and local network access that is off is not
  turned on by an interface change. One test launches `/bin/sleep` to show
  Stop signals only the process it started. Packaging checks that
  `Contents/MacOS/proxy-engine` is in the bundle and executable.
- Transparent capture is tested in three places, one per process. The
  bridge is tested in `core/proxy-engine/tests/transparent_capture.rs` against
  a real bridge listener, real TLS, and real HTTP/1.1, HTTP/2 and HTTPS
  origins, with the extension stood in for by a client that sends what it
  sends — a `CONNECT` naming an *address*, then a hello naming a host. What is
  asserted is that the certificate and the verification follow the name, that
  an origin certificate for another name is still refused, that plain HTTP is
  forwarded by its `Host`, that rules and throttling apply, and that a flow from
  the proxy's own process, a flow to the bridge itself, and anything that is
  not a `CONNECT` are refused without anything being connected to. The
  named handshake itself is tested in `core/tls/tests/tls_sessions.rs`. What
  the app and the extension agree on — the handshake bytes, a host that could
  break the request line, which flows are claimed, loopback however it is
  spelled, a reused pid, an unreachable core — is tested in
  `Tests/TransparentProxyKitTests` with no extension at all. The panel's model
  is tested against a stand-in for macOS: that refreshing changes nothing,
  that an unsigned build asks nothing, that approval finishes the Turn On that
  was waiting for it and no other, and that "on" with no core says nothing is
  being captured. The provider itself runs only in a signed, approved
  extension, and is verified by hand.
- The Android Emulator tooling is tested in its own package, with `adb` stood
  in for by a small simulation rather than a table of canned replies: it holds
  a settings value, `settings put` changes it, `settings get` reads it back.
  That is what makes saving a proxy and putting it back testable as behaviour —
  a fake that always answered the same thing would pass a restore that restored
  nothing. So the assertions are the ones that matter: an absent setting is
  deleted rather than set to `:0`, a pre-existing corporate proxy comes back
  rather than being cleared, connecting twice does not record ProxyLab as what
  was there before, and disconnecting twice is one restore and then nothing.
  The device-list fixture is real `adb devices -l` output. One suite runs
  against the real `adb` and whatever emulator is running, and is skipped
  unless `PROXYLAB_TEST_ANDROID=1` names it as wanted; everything in it reads,
  so a test run never reconfigures a developer's emulator.
