# ProxyLab macOS UI Context

Load this file for macOS SwiftUI/AppKit UI work, traffic inspection/navigation, pinned domains, or UI architecture.

### macOS UI

- **Technology:** SwiftUI, with AppKit only for application activation and the
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
  `ProxyLab.app` for running outside a terminal.
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
  | `Sources/ProxyLabCore/Views/TrafficDetail/` | One exchange: the headline, the overview, and the two halves |
  | `Sources/ProxyLabCore/Simulator/` | Where the core listens and keeps its CA, and the simulator session |
  | `Sources/ProxyLabCore/Android/` | The emulator session: what was saved, and what is put back |
  | `Sources/ProxyLabCore/Views/Simulator/` | The sidebar's foot: pick a booted simulator, connect it, see where it stands |
  | `Sources/ProxyLabCore/Views/Android/` | The same, one panel down, for a running emulator |
  | `Sources/ProxyLabCore/Views/Device/` | Under it: the local-network switch, and what to type into a phone |
  | `Sources/ProxyLabCore/NetworkExtension/` | Installing and switching transparent capture, and what macOS says about it |
  | `Sources/ProxyLabCore/Views/NetworkExtension/` | The sidebar's last panel: this Mac's own traffic, on or off |
  | `Sources/ProxyLabTransparentProxy/` | The Network Extension itself: which flows it takes, and copying them to the core |
  | `Sources/TransparentProxyKit/` | What the app and the extension agree on: the bridge handshake, the flow policy, the configuration, the status |

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

One exchange is shown on three faces under one headline. The headline is the
method, the URL, the outcome and the protocol versions — the things a person
needs before deciding which half to look at, and the place a mixed exchange
says `HTTP/1.1 → HTTP/2` because the two sides are negotiated separately.
**Overview** puts the two halves beside each other with the timing the core
measured; **Request** is the query read out of the URL, the headers (formatted
or as the raw block), and the body; **Response** is the status, the HTTP
version, the content type, the body size, the headers, and the body. JSON is
laid out as a collapsible tree with a raw toggle; nothing here is an editor.
What is absent is shown as absent: an exchange the origin never answered has a
response face that says why, not an empty one. There is no before/after diff
of a breakpoint edit or a transformation: the store keeps one request and one
response, the traffic that actually went past, and a second historical copy
is out of scope until something else needs it.

Reading a body — decoding it in the character set the message declared, laying
JSON out as formatted text and as a collapsible tree, recognising an image,
deciding that bytes are not text — happens once per selected exchange, off the
main actor, as the exchange is fetched. The pane draws that reading. Nothing
decodes on a redraw, and nothing writes back to the exchange: what is copied
and what is saved are the bytes the core captured.

None of it asks the core anything new. The query is parsed from the stored URL,
the media type from the stored header, the timing from the stored timestamps;
`get_traffic` already carries every one of them.

### Pinned domains

A developer debugging one service reads the same handful of hosts all day, so
the traffic screen keeps them in a sidebar:

```
┌───────────────┐
│ ✓ All Traffic │
│               │
│ PINNED        │
│   api.exa…    │──▶ host filter ──▶ the same list_traffic query
│   auth.exa…   │
└───────────────┘
```

**A pin is a preference, not traffic.** It is a host name this application
keeps in `UserDefaults` under one key, and the core is never told about it:
there is no IPC operation for pinning, no table, and no column. The Rust core
stays authoritative for traffic, rules, and storage; the Swift application owns
this one piece of interface state. `PinnedDomainsStore` is the whole of it, and
it reads and writes through a `PinnedDomainsStorage` protocol so a test never
writes into the developer's own preferences.

**Selecting a pinned domain sets the host filter, and nothing else.** That is
the whole of the navigation: there is no second way of fetching traffic, no
second search, and no filtering done in Swift. Search, method, status, sort and
paging all compose with a pinned domain because they are the same
`TrafficFilters` value and the same `list_traffic` request they always were —
and live traffic for another host is excluded by the host comparison the local
filter already made.

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
