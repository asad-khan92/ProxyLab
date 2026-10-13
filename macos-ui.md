# ProxyLab macOS UI Context

Load this file for macOS SwiftUI/AppKit UI work, traffic inspection/navigation, pinned domains, or UI architecture.

### macOS UI

- **Technology:** SwiftUI, with AppKit for application activation, quit, and the
  file picker a Map Local rule needs.
- **Responsibility:** Presentation, user interaction, and configuration.
- Renders captured traffic, inspectors, search and filtering, and rule
  configuration surfaces.
- Holds no proxy logic. It does not parse protocols, terminate TLS, evaluate
  rules, or own the capture store.
- Location: `macos/ProxyLab/`
- Built as a Swift package, not an Xcode project: `swift build` and
  `swift test` are the whole of the build, and Xcode opens `Package.swift`
  directly. `Scripts/make-app-bundle.sh` wraps the executable in
  `ProxyLab.app`, embeds the release `proxy-engine` at
  `Contents/MacOS/proxy-engine`, and embeds the Network Extension under
  `Contents/Library/SystemExtensions`. With no signing variables the bundle
  is ad-hoc signed and launches as before; with the team, identity, and two
  profiles set, the extension is development-signed and then the app.
  Notarization is not part of that script. See `macos/ProxyLab/README.md`.
- Modules:
  | Path | Responsibility |
  | --- | --- |
  | `Sources/ProxyLab/App/` | The entry point, and nothing else |
  | `Sources/ProxyLabCore/IPC/` | The Unix socket, line framing, and the protocol client |
  | `Sources/ProxyLabCore/Models/` | The wire models, mirroring `proxy-storage` and `rules`, and the protocol messages |
  | `Sources/ProxyLabCore/Services/` | What the window is showing, and how a body, a row, and a rule are presented |
  | `Sources/ProxyLabCore/Views/` | The traffic list, the exchange inspector, and the rules screen |
  | `Sources/ProxyLabCore/Views/Pinned/` | The sidebar: all traffic, and the domains worth coming back to |
  | `Sources/ProxyLabCore/Views/Rules/` | The rule list, the rule detail pane, and the rule editor |
  | `Sources/ProxyLabCore/Views/Breakpoints/` | The breakpoint list, the throttling controls, and the exchange being held |
  | `Sources/ProxyLabCore/Views/Curl/` | The import sheet: a pasted cURL command, and the request it describes |
  | `Sources/ProxyLabCore/Views/Saved/` | The requests worth keeping: the sidebar section, and the sheet that edits one |
  | `Sources/ProxyLabCore/Views/TrafficDetail/` | One exchange: the summary, then Request, Response, and Messages |
  | `Sources/ProxyLabCore/Simulator/` | Where the core listens and keeps its CA, and the simulator session |
  | `Sources/ProxyLabCore/Android/` | The emulator session: what was saved, and what is put back |
  | `Sources/ProxyLabCore/Views/Simulator/` | A booted simulator's setup, opened from its capture-source row |
  | `Sources/ProxyLabCore/Views/Android/` | The same, for a running emulator |
  | `Sources/ProxyLabCore/Views/Device/` | Local network access for a physical iPhone, opened from its row |
  | `Sources/ProxyLabCore/Capture/` | Launching the bundled proxy engine, and Start / Stop / quit for capture |
  | `Sources/ProxyLabCore/SystemProxy/` | Saving and restoring the active service's HTTP and HTTPS proxy |
  | `Sources/ProxyLabCore/Network/` | Watching the network path, and when the LAN listener moves |
  | `Sources/ProxyLabCore/NetworkExtension/` | Installing and switching transparent capture, and what macOS says about it |
  | `Sources/ProxyLabCore/Views/NetworkExtension/` | This Mac's detail: proxy, transparent capture, and CA trust. It is not a second Start |
  | `Sources/ProxyLabCore/Views/CaptureSources/` | The compact capture-source rows, and the detail each row opens |
  | `Sources/ProxyLabCore/Views/CaptureSourcesToolbar.swift` | Toolbar buttons that open those same details |
  | `Sources/ProxyLabCore/Views/Diagnostics/` | Where the core, listeners, IPC, storage, Network Extension, and device tools stand. A sheet from the window toolbar |
  | `Sources/ProxyLabTransparentProxy/` | The Network Extension itself: which flows it takes, and copying them to the core |
  | `Sources/TransparentProxyKit/` | What the app and the extension agree on: the bridge handshake, the flow policy, the configuration, the status |

The window toolbar also carries the capture control: **Starting…**,
**Listening**, **Stopping…**, **Stopped**, **Needs Approval**, **Needs CA Trust**,
or **Error**,
with **Start** or **Stop**. That control and the This Mac panel read one
capture lifecycle. Launching the app starts capture. After both listeners are
ready, and after certificate trust reports ready, Start saves the active
network service's HTTP and HTTPS proxy and points both at `127.0.0.1:9090`,
then turns transparent capture on. The trust check reads the core's
current CA and, when that exact certificate is not already a trusted
anchor, asks this user to trust it. Until that evaluation succeeds, Start
does not enable the system proxy. Cancelling or failing that step leaves
the proxy unchanged and the engine running. While the check runs the
status reads **Checking CA trust…**, then **Needs CA Trust** while this
user is deciding, or **Installing ProxyLab CA…**. A refusal reads **CA trust cancelled** or
**CA trust failed**. Starting means work is still moving; a prompt is never
left on Starting. Stop
restores the settings ProxyLab saved, reads the live route back, turns
transparent capture off, and only then stops the proxy engine this app
launched. Stop does not remove the CA trust record. A restore that does not verify, including a cancelled administrator
approval, leaves that engine running. Command-Q is the same shutdown. While
it runs, the control reads **Restoring network settings before quitting…**,
or **ProxyLab must restore this Mac's proxy before quitting.** when macOS
is asking for approval. It is refused until the live route no longer points
at ProxyLab. The refusal offers Retry and does not offer quitting anyway.
An explicit Stop
is remembered until Start or quit, so a reconnect does not turn capture back
on. This Mac's row reads that same lifecycle. It does not have its own Start or Stop.

The window has three screens, switched by one segmented control in its
toolbar: **Traffic**, **Rules**, and **Breaks**. The control is in the toolbar
rather than in a sidebar column, because a column holding three words would
push the panes along without saying anything a person could not read in one
control. The traffic screen does have a sidebar, and it holds something worth a
column: the domains a developer keeps coming back to. Every screen reads the
same connection: one `IPCClient`, one socket, one subscription. The traffic
model owns that connection's lifecycle — it is what connects, subscribes, and
reconnects — and the other two make their requests on the same client. There
is one notification stream and one consumer of it, so the traffic model hands
the two notifications that are about a held exchange to the breakpoints model
rather than anything reading the stream twice.

Diagnostics is a sheet from that same toolbar, not a fourth screen. It reads
the connection, the device models, and the Network Extension model the traffic
screen already has, and asks the core for `diagnostics` rather than keeping a
second copy of that state.

The traffic inspector lists the exchanges the core captured, narrows that list
by search, method, status, host, content type, HTTP version, WebSocket, SSE
and failed requests, reads it from either end of the order, shows one exchange
— request and response, headers, captured bodies, timing — deletes one, and
clears them all. Search is a substring or an optional regular expression of
URL, host, path, method, status, headers, captured bodies, and stream-message
text; it runs in the core so a list page is still summaries. It subscribes for
new traffic rather than polling, and inserts an arriving exchange as a row
rather than re-fetching the list, except when a search is active: headers and
bodies are not on the event, so a match that lives only there is picked up by
reloading the current query after the typing (or the arrival) settles. Every
one of those is an IPC operation; there is nothing the interface can ask the
core that is not one of them.

One exchange is shown under a compact summary: the method, the host, the path,
and how it ended — status, protocol, duration, and content type. The two
protocol versions stay visible together when they differ, as `HTTP/1.1 → HTTP/2`,
because the two sides are negotiated separately. Below that are **Request**,
**Response**, and, for a WebSocket handshake or an event stream, **Messages**.
Request is an overview, the headers, the query, the body, the cookies, the
authorization, and a message reconstructed from what was stored. Response is
an overview, the headers, the body, the cookies, and the same kind of
reconstruction. A short list of those sections sits beside the one that is
open. Messages is the captured timeline. JSON opens as pretty text, and can
be switched to a collapsible tree or to the presented text; nothing here is
an editor.
What is absent is shown as absent: an exchange the origin never answered has
a failure summary — a stage, a destination, a duration, and the core's curated
sentence — rather than an empty response. Authorization is summarized as a
scheme and a masked credential. The header list still shows the values the
message carried. The reconstructed message says it was assembled from the
stored fields, because the wire bytes are not what was kept. There is no
before/after diff of a breakpoint edit or a transformation: the store keeps
one request and one response, the traffic that actually went past, and a
second historical copy is out of scope until something else needs it.

Reading a body happens once per selected exchange, off the main actor, as the
exchange is fetched. A declared Content-Encoding is removed first — gzip
(and its `x-gzip` alias), deflate, and brotli — and only then does the reading
decode characters in the character set the message declared, lay JSON out as
formatted text and as a collapsible tree, recognise an image, or decide that
the bytes are not text. The pane draws that reading. Nothing decodes on a
redraw, and nothing writes back to the exchange. The stored body stays the
bytes the proxy captured, so a save writes those bytes, Content-Encoding
included. Pretty and the tree are a layout of the presented JSON. Raw shows
the presented text, and copying that copies the text that was read.

None of it asks the core anything new. The query is parsed from the stored URL,
the media type from the stored header, the timing from the stored timestamps;
`get_traffic` already carries every one of them.

### Pinned domains and captured domains

A developer debugging one service reads the same handful of hosts all day, and
also needs every host the capture has already named. The traffic screen keeps
both in one sidebar:

```
┌───────────────┐
│ ✓ All Traffic │
│               │
│ PINNED        │
│   api.exa…    │──▶ host filter ──▶ the same list_traffic query
│               │
│ DOMAINS       │
│   api.exa…    │
│   cdn.exa…    │
└───────────────┘
```

Under the domains, still in that sidebar, the capture sources are a second
list and not a filter:

```
CAPTURE SOURCES
  ● This Mac              Listening
  ● iPhone 17 Pro Max     Connected · CA trusted
  ○ Android               No emulator
  ○ Physical iPhone       Waiting for a local network
```

A row is a name and one state. Selecting it opens that source's setup — a
popover, the same detail the toolbar buttons open — and does not change the
host filter. Ports, `simctl`, `adb`, and the Network Extension stay in that
setup, or in Diagnostics, rather than on the row. This Mac's state is the
capture lifecycle's. A physical iPhone's row names the LAN address the phone
is given, and follows the network observer when that address changes. It is
not the simulator's route.

**All Traffic** clears the host filter. **Pinned** is the short list of hosts
this application keeps. **Domains**, under a collapsible header, is every
destination host this window has seen in a loaded page or a `traffic_added`
event. Selecting any of them sets the host filter and nothing else.

**A pin is a preference, not traffic.** It is a host name this application
keeps in `UserDefaults` under one key, and the core is never told about it:
there is no IPC operation for pinning, no table, and no column. The Rust core
stays authoritative for traffic, rules, and storage; the Swift application owns
this one piece of interface state. `PinnedDomainsStore` is the whole of it, and
it reads and writes through a `PinnedDomainsStorage` protocol so a test never
writes into the developer's own preferences. The domain list is not a second
preference and not a second query: it is the hosts the traffic model has
already been shown, remembered so that narrowing the list to one host does not
hide the others. A host that has not been on a loaded page and has not arrived
as a live event is not in that list yet. Clearing captured traffic clears it.
A pin outlives that.

**Selecting a domain sets the host filter, and nothing else.** That is
the whole of the navigation: there is no second way of fetching traffic, no
second search, and no filtering done in Swift beyond remembering which hosts
exist. Search, method, status, sort and
paging all compose with a selected domain because they are the same
`TrafficFilters` value and the same `list_traffic` request they always were —
and live traffic for another host is excluded by the host comparison the local
filter already made. The sidebar's own domain filter, when the list is long,
narrows which hosts are drawn. It does not narrow traffic.

**A pin is identified by the destination host**, spelled as the traffic model
spells it: no scheme, no port, no path, and no brackets around an IPv6 literal.
Two spellings differing only in ASCII case are one pin, because that is how the
core's own host column compares. It is deliberately the destination the proxy
connected to rather than the host a row displays — the two can differ, and the
destination is the column `list_traffic` filters on, so pinning anything else
would narrow the list to nothing.

**Pins outlive traffic.** Clearing the core's traffic store empties the list and
leaves the pinned domains exactly where they were, and a pinned domain with
nothing currently matching says so in its own words rather than looking like a
filter that went wrong.

**The narrowing happens in the core.** Search and the filters are parameters
of `list_traffic`, not a sieve the app runs over rows it has already fetched:
the list is one page of an indexed query, so it behaves the same on a hundred
thousand exchanges as on ten. The app holds one `TrafficFilters` value, turns
it into one request, and pages with the cursor the core gives back.

The same filters are evaluated a second time, locally, and only for that
purpose: a `traffic_added` event carries a summary, and the app decides from
that summary alone whether the row belongs in what is shown. It has to agree
with the core's own query exactly — the same exact host, the same status
range, the same ASCII-only case folding `LIKE` does — or a row would appear
that the next reload would drop. Headers, bodies, and stream payloads are not
on the summary. When the search box is not empty, an arrival also schedules
the same debounced reload the typing already uses, so a match that lives only
in a body still appears without asking the core about every exchange.
