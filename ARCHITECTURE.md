<!-- # ARCHITECTURE.md

Intended high-level architecture for **ProxyLab**, a macOS HTTP/HTTPS
debugging proxy for developers.

> **Status:** This document describes the *intended* design. Almost none of it
> is implemented yet — see `ROADMAP.md`. It exists so that implementation work,
> when it happens, has a fixed target.

## Components

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

## The rule engine

```
                     request
                        │
                        ▼
                 ┌─────────────┐   priority, then id:
                 │   RuleSet   │   Modify rules collected on the way,
                 └──────┬──────┘   the first decision ends the walk
                        │
      ┌─────────┬───────┴───────┬──────────────┬──────────────┐
      ▼         ▼               ▼              ▼              ▼
    Allow     Block         MapLocal       MapRemote      Redirect
   upstream  answered      answered from   sent somewhere answered with
             here          a file          else           a Location

            + Modify Request   (applied if it goes upstream)
            + Modify Response  (applied to the origin's answer)
```

`core/rules/` is a pure crate: it depends on nothing of ProxyLab's, opens no
file, makes no network request, and touches no database. It holds the rule
model and one function that matters — given the rules and a request, which
action applies. `proxy-storage` persists rules, `proxy-ipc` manages them, and
`proxy-engine` carries the decisions out. **Deciding and doing are separate**,
which is what makes the whole of the matching testable without a proxy and
keeps a misconfigured `MapLocal` path from turning evaluation into filesystem
I/O on every request.

### Matching

Three optional criteria, combined with `and`; **one that is not set matches
anything**, so a matcher naming nothing matches every request — which is how
"block all of it except these" is written.

| | |
| --- | --- |
| host | exact, case-insensitive, port never compared |
| path | exact, or an explicitly named prefix; case-sensitive; query never compared |
| method | exact, case-insensitive |

Case folding follows what the thing being compared is: DNS names and methods
are case-insensitive, a URL path is not. `PathMatch::Prefix` is a second named
matcher rather than a special character inside an exact one, because `/assets`
meaning "starts with" in one rule and "is exactly" in another is the kind of
ambiguity that makes a rule set impossible to reason about. There is no regex,
no glob, and no matching on headers or bodies.

### Precedence

The rules below were written when every rule was a decision. They still
hold for decisions; **Transformations** below says how the two Modify
actions, which are not decisions, fit around them.

Rules are consulted in order of **priority ascending, then id ascending**, and
the **first enabled rule that matches decides**. Nothing after it is consulted,
so two rules never both apply and there is no question of combining their
actions. Priority is the part a person chooses; the id breaks the tie and
breaks it the same way for ever, because a rule id is a time-ordered UUID
minted when the rule was created — "equal priority, older rule first" needs no
sequence column and cannot disagree with one. A `RuleSet` is sorted when it is
built, not when it is consulted, so nothing depends on the order rows came back
from a database in.

### Actions

**Block** answers the request with the rule's status (`403` unless it names
another) and one fixed sentence, and never contacts the origin.

**Map Local** answers with the contents of one configured file. The path is the
rule's and only the rule's: nothing from the request is joined to it, appended
to it, or used to pick a file beside it, which is the whole of Map Local's
filesystem exposure and why there is no traversal to defend against. The file
is checked when the rule runs — it must exist, be a regular file, be readable,
and be within a bounded size — and a failure is `500` with a sentence that
names no path. The body is read into memory rather than streamed; a sandbox
root, if one is ever wanted, would be a directory the configured path must sit
under, and is deliberately not built yet.

**Map Remote** sends the request elsewhere. Scheme, host, and port always come
from the rule; the path is the request's own unless the rule names one, and
the query likewise. The destination is parsed once, when the rule is written,
into separated parts — so no URL is ever assembled by pasting two strings
together, and a scheme this proxy cannot speak is refused at the moment the
rule is written rather than the moment it would have mattered. Method, body,
and headers are the request's own; only `Host` is rewritten, to name where the
request is actually going.

**Redirect** answers with one of `301`, `302`, `307` or `308` (`302`
unless the rule names another) and a `Location` that is the rule's URL
exactly as written — nothing of the request's own path or query is carried
over, which is the one way it differs from Map Remote — and never contacts
the origin. One redirect is one answer; whatever the client asks for next is
a request of its own. The target is parsed into the same `RemoteTarget` parts
a Map Remote destination is, so the same URLs are refused for the same
reasons.

### Transformations

**Modify Request** changes a request's method, path, query parameters,
headers and body. **Modify Response** changes a response's status, headers
and body. Every edit is literal: set a header, add one, remove one; set a
path, or replace a path prefix with another; set or remove a query
parameter; replace a body with text, or set one field of a JSON object body.
No pattern, no regex, no template, no expression.

**A transformation is not a decision.** It says what a message looks like
when it gets where it is going, not where it goes or who answers it. So the
walk that finds the deciding rule does not stop at one:

```
priority 0   Modify Request   add Authorization   ──▶ collected
priority 5   Allow            /health             ──▶ (did not match)
priority 10  Map Remote       api.example.com     ──▶ the decision; walk ends
priority 20  Modify Request   add X-Debug         ──▶ never consulted
```

- A matching transformation is collected and the walk carries on. The first
  matching decision still ends it, and a request no decision matches is
  allowed, as before.
- A transformation **below** the deciding rule is not applied, because
  nothing below the deciding rule is ever consulted. A decision **above** a
  transformation therefore exempts what it matches: an `Allow` at the top of
  the list is how a transformation is kept off one request.
- Every rule is matched against the request as it arrived. A transformation
  cannot make a request match a rule it did not match; there is one pass, over
  one request.
- Where two transformations change the same thing, **the one higher in the
  list wins**, exactly as it would were they decisions. They are applied in
  the walk's order reversed, so the rule consulted first has the last word.
- A rule set with no transformations in it is evaluated exactly as before.
  `RuleSet::evaluate` still returns the decision; `RuleSet::outcome` is the
  same walk with the transformations kept.

**Where each one happens**, on every route — a plaintext request, a request
inside an intercepted TLS session over HTTP/1.1 or HTTP/2, a transparently
captured flow, and a replay:

```
client ─▶ request breakpoint ─▶ rules ─▶ Modify Request ─▶ Map Remote ─▶ origin
                                  │
                                  └──▶ Block · Map Local · Redirect ─▶ answered here

client ◀─ response breakpoint ◀─ Modify Response ◀─────────────────────── origin
```

- **After a request breakpoint**, so the rules match, and change, the request
  a developer may just have edited. The person is nearest the client, and the
  rules sit between them and the origin, in both directions.
- **Only if the request is sent** — to its own origin, or to the one a Map
  Remote names. A request the proxy answers itself (Block, Map Local,
  Redirect) is recorded as the client sent it: a transformation changes what
  is sent, and nothing was.
- **Before Map Remote**, so a mapped request carries its changes to the new
  destination. A path a Modify rule set is the path Map Remote keeps, unless
  the mapping names a path of its own.
- **A response is changed before a response breakpoint shows it**, so what a
  developer holds still is what the client is about to be given. Only a
  response from an origin is changed — the responses that pass through
  `service::into_client_response`, which is where a response breakpoint stops
  them too — and not one the proxy generated.
- **Throttling is untouched**: the added delay is before the rules, and the
  bandwidth ceilings pace the body that is actually sent.
- **A WebSocket handshake is never transformed**, in either direction: its
  key, its upgrade headers and its `101` are what the two ends agreed on.
- **A replay** is transformed like any other request, and the status it
  reports is the one its exchange is captured with.

**Bodies.** A replacement needs nothing from the body it replaces and does
not read it. A JSON field edit needs all of it: the body is read up to 4 MiB
and for up to 5 seconds, and one that is larger, slower, compressed, or not a
JSON object is sent exactly as it was, with a line in the log. An event
stream is the usual body that is slower; it is forwarded in full and never
edited. An edited JSON body is written back compactly with its keys sorted —
the same JSON, not the same bytes — because the JSON library keeps objects
sorted, and enabling its order-preserving feature would change key order for
every other crate that uses it. Whatever replaces a body is sent with a
`Content-Length` of its own, and a replacement drops `Content-Encoding`,
because the text a rule carries is not compressed. A response to `HEAD`, a
`204` and a `304` have no body to edit.

**What a transformation may not touch.** `Host`, `Content-Length`,
`Transfer-Encoding` and the hop-by-hop headers are refused when the rule is
written: framing is the proxy's, the hop-by-hop headers are removed on every
hop anyway, and where a request goes is Map Remote's. A header value cannot
carry a line break, a query edit cannot carry `&` or `#`, a response status is
`200`–`599`, and a rule carries at most 32 edits of each kind and 32 KiB of
body text — it crosses the IPC boundary in one 64 KiB line, and a larger body
is a file, which is Map Local's.

**No new matchers.** Host, path and method are still the whole of what a rule
matches on. Query and header matchers were considered and left out: the
matcher is shared with breakpoints, whose context carries host, path and
method alone, and a criterion one of them could not see would be half a
feature.

**Nothing is logged but which rule.** A header a rule sets is as likely as
not an `Authorization`; the log says which rules transformed an exchange and
never what they wrote, and the rule list shows a header's name and never its
value.

### Where rules are applied

Before the request goes upstream, on both routes: a plaintext proxy request,
and a request read out of an intercepted TLS session. A `CONNECT` is **not**
evaluated — it carries no path, so matching it against a path rule would be
matching something that is not there, and blocking it would block every
request inside the session rather than the one a rule named. The requests
inside that session are each evaluated on their own.

One consequence is worth stating: the TLS session to the original origin is
established by the `CONNECT`, before any request has been read and therefore
before any rule has been consulted. So a rule that blocks or maps a request
inside an intercepted session stops the *request*, not the connection that was
opened to carry it.

### Rules and captured traffic

A ruled request is traffic like any other. A blocked one, a locally answered
one, and a mapped one each appear in the traffic list with the response the
client actually received, so nothing a rule touched silently disappears. A
mapped request keeps the URL the client asked for and records the destination
it was actually sent to, so both halves of what happened are visible.

No column was added to say which rule applied. The exchange model, its schema,
and the IPC traffic messages are unchanged by this step: what the client
experienced is already recorded accurately, and a field naming the rule is
worth adding when there is a rule interface to show it in.

A transformed exchange is recorded as the traffic that actually went past:
the request as it was sent upstream, and the response as the client was given
it — the same promise a breakpoint edit keeps. There is one request and one
response in the capture, never an original and a modified pair. So a Modify
Request rule re-records the request head once it has applied (`Recorder::
request_edited`), and the body is captured as it goes upstream, which makes a
replaced body captured as the replacement. The inspector therefore has no
original-versus-modified diff: the bytes it shows are the captured ones, and
a second copy of what the client first sent is not stored.

### The rules interface

`Sources/ProxyLabCore/Views/Rules/` is where a rule is written, and it is the
only thing in the app that knows a rule can be written. What it does is the
five rule operations and nothing else: list them, create one, change one,
remove one, and read one back.

**Nothing is decided here.** The editor's checks — a name that is not empty, a
priority that is a number, a path that starts with a slash, a status between
100 and 599 — are the core's own checks made early, so a mistake is shown under
the field that caused it rather than coming back as a refusal after a round
trip. Every save still goes to the core, the core validates the whole rule
again, and a refusal from it is shown the same way: under the field where one
of the core's fixed phrases names a field, and on the form where it does not.
If the two ever disagree, the core is right and the app is the thing that is
wrong.

**Seven actions, in two groups.** The action is chosen from a menu rather
than a segmented control: the five decisions first, and the two Modify
actions after a divider, because they are a different kind of thing and the
editor and the detail pane say so. A Modify rule is written as rows — an
operation, a name, a value — in the same shape as the request editor's header
rows, and a row left empty is left out rather than refused. Its checks are the
core's, made early, as every other field's are.

**A destination is split here because the wire carries it in parts.**
`create_rule` carries a `RemoteTarget` — scheme, host, port, path, query — and
never a URL string, because the core parses a destination once, when the rule
is written, and builds every outgoing URL from the parts. So a URL typed into
the editor is split by `Models/RemoteDestination.swift`, which makes exactly
the refusals `RemoteTarget::parse` makes and for the same reasons. It is the
one piece of rule logic that exists on both sides of the boundary, and it is
there because the boundary's shape requires it, not because the app wanted an
opinion. The core validates the parts it receives regardless.

**The order shown is the order the core will consult.** `list_rules` returns
rules sorted by priority and then by id, and the app never sorts them again.
Moving a rule means writing the priorities that produce the wanted order —
renumbered in steps of ten, and only for the rules whose priority actually
changes — and then asking for the list again. Renumbering rather than swapping
two numbers, because two rules may sit at the same priority and the core breaks
that tie by id, which is not something a person can move a rule past. Spacing
them out is what makes the displayed order one the core agrees with.

**Nothing on screen changes before the core says it changed.** A switch that
flips and then flips back is worse than one that takes a moment, and a rule
that has vanished from a list but not from the proxy is worse than either. So
enabling, moving, and deleting all go to the core first; the editor stays open,
with everything still in it, until a save is confirmed; and a rule removed
while it was being edited is said to be gone rather than written back.

**Map Local never reads the file.** The app asks macOS for a path through
`NSOpenPanel`, checks that the path is absolute and that something readable
that is not a directory is at it, and sends the path. The core opens the file
when the rule runs, and is the only thing that ever does. There is no directory
browser, nothing is copied, and no filesystem operation is exposed through IPC
— a rule carries a path, and a path is not an instruction to do anything.

**Rules are asked for when the rules screen needs them.** Rule changes are not
events and the core says so, so there is no subscription and no polling: the
list is loaded when the screen appears and after anything the app changed. A
rule changed by something else is seen the next time either happens. The
traffic subscription is untouched by any of this, and a rule change never
reloads traffic.

### Persistence and concurrency

Rules live in a `rules` table of their own, added by a schema migration that
leaves the traffic tables untouched. The matcher and the action are JSON —
they are sum types with optional parts and nothing queries inside them — while
`enabled`, `priority`, and `id`, which decide which rule wins, are columns, and
the index is on the order rules are consulted in. Everything read back is
validated: a row edited by hand cannot put an action the pipeline has no
answer for into a running proxy.

A request never touches the database. The engine holds one immutable snapshot
behind a lock that is taken only long enough to clone a pointer; a request
evaluates against the snapshot it took, and a rule change builds a whole new
set and swaps it in. So a rule change is never seen half-applied, a request in
flight finishes under the rules it started with, and no lock is held while a
request is evaluated.

## cURL and replay

```
                   ┌─────────────────┐
                   │    SwiftUI      │
                   │ Traffic detail  │  Copy as cURL
                   │ Import sheet    │  Import cURL, edit, Replay
                   └────────┬────────┘
                            │ IPC
                            ▼
                   ┌─────────────────┐
                   │   proxy-ipc     │
                   └────────┬────────┘
              ┌─────────────┴─────────────┐
              ▼                           ▼
        proxy-curl                  Replayer  (proxy-engine)
     parse / serialize                    │
              │                           ▼
              ▼                    rules ─▶ TLS ─▶ upstream
        RequestSpec                        │
       (proxy-http)                        ▼
                                  capture ─▶ storage ─▶ events ─▶ traffic list
```

Three operations, one request model, and no second HTTP stack.

### The canonical request

`proxy-http`'s `RequestSpec` is what "a request somebody wants sent" means
across the core: the cURL parser produces one, the IPC boundary carries one,
and the replay entry point sends one. Its fields are private and there is one
constructor, so holding one is holding a request that has already been
checked — an absolute `http`/`https` URL with a host, a method that is an HTTP
token, and headers whose values cannot carry the line break that would smuggle
a second request inside the first. The body is bytes and is never assumed to
be text.

Two refusals in it are worth naming. A URL carrying credentials in its
authority is refused rather than quietly stripped, because a URL is the one
part of a request that *is* logged and shown in a traffic list. And
`Proxy-Authorization` is refused, because it authenticates to an upstream
proxy, ProxyLab chains to none, and a credential that silently stopped being
sent would be a different request. The cURL parser turns both into the
`Authorization` header cURL itself would send, where that is what they mean.

### The parser

`proxy-curl` reads a command as **data**. It splits the text with a shell's
*quoting* rules and none of its *expansion*: `$(…)`, backticks, `${…}` and
`$NAME` are errors, and so are pipes, redirections, and anything that makes the
text more than one command. There is no `Command`, no subprocess, and no path
from a parsed command to one — a test in `proxy-engine` reads the sources of
every crate on this path and fails if a `Command` appears in any of them.

`$'…'` is understood, because it is quoting rather than substitution: it is how
a byte string is written down, and so it is how this crate writes a request
body that is not text.

Supported options:

| | |
| --- | --- |
| `-X`, `--request` | the method |
| `-H`, `--header` | a header, repeatable; `Name;` is an empty value and `Name:` removes one |
| `-d`, `--data`, `--data-ascii` | a body, repeatable, joined with `&` |
| `--data-raw` | the same, with `@` meaning nothing special |
| `--data-binary` | the same, sent exactly as given |
| `--url` | the URL |
| `-G`, `--get` | the data goes in the query instead of the body |
| `--compressed` | becomes the `Accept-Encoding` header cURL would send |
| `-k`, `--insecure` | accepted and **reported, not applied** |
| `-u`, `--user` | Basic authentication |
| `-A`, `--user-agent` | the `User-Agent` header |
| `-s`, `-S`, `-v`, `-i`, `-f`, `-#` | ignored and reported: they change cURL's output, not the request |

Everything else is refused **by name**. An option that would change what is
sent — `-L`, `--data-urlencode`, `--form` — is never quietly dropped, because a
request that is not the one the command describes is worse than an error saying
so. `-x`, `--proxy*`, `--socks*`, `--preproxy` and `--noproxy` get a refusal of
their own: ProxyLab does not chain to an upstream proxy. `--data @file` and
`@-` are refused too — importing a command someone else wrote must not read
this machine's files.

`--insecure` cannot be honoured: ProxyLab verifies the destination's
certificate on every connection it makes and there is no way to turn that off.
So it is *reported* rather than silently ignored or turned into a refusal of an
otherwise good command. `--compressed` is applied as a header and reported too,
because the response is then captured compressed.

### The serializer

Deterministic: one request produces one command, byte for byte, every time.
Headers keep the order and the repetitions the message had. Everything is
single-quoted, with `'` written `'\''`; a value that cannot be a readable
single-quoted string — bytes that are not text, a control character — is
written `$'…'`, which this crate also reads back.

Left out: the hop-by-hop headers, plus `Content-Length` and `Host`, which cURL
works out from the body and the URL it is given. Everything else is kept,
`Authorization` and `Cookie` included, because the point of Copy as cURL is to
send the request again and a reproduction that drops the credential reproduces
nothing.

A command written from a capture whose body was truncated carries a `#` comment
saying so, and the IPC result says so in a field.

A request written and read back is the same request — method, URL, headers,
body. The one thing a round trip does not preserve is a body with no
`Content-Type`, which gains cURL's own `application/x-www-form-urlencoded`; that
is cURL's rule rather than this crate's, and a command that wants none says so
the way cURL does, with `-H 'Content-Type:'`.

### Replay

**A replay does not go through the listening socket.** It could — the proxy is
listening — but then ProxyLab would be a client of itself: a second TCP
connection, a `CONNECT` for an HTTPS replay, a second TLS handshake against its
own certificate, and a dependency on the listener's address. None of that is
*request* behaviour; all of it is transport ceremony for carrying a request
from somewhere else, and here there is no somewhere else. So `proxy-engine`'s
`replay` module hands the request to the pipeline at the point a request
arriving over a socket would have been handed to it: decoded, and about to be
evaluated.

Everything below that point is the same code. The rule engine is the one the
listener holds — the engine itself, not a snapshot, so a rule created a moment
ago applies — and `Block`, `Map Local`, and `Map Remote` behave exactly as they
do for a browser's request. The connection comes from the proxy's own
`UpstreamManager` — so a replay reuses a connection the proxy already has and
leaves the one it opened for the traffic that follows — and an `https`
replay's TLS session is opened and verified by the same `Interceptor`. The capture is the proxy's own, with the proxy's own body
limit, feeding the proxy's own storage sink and event publisher.

So a replayed request is traffic: it appears in the traffic list, it is stored,
it is published, and a replay that could not reach its destination is an
exchange with no response and an error saying why — which is what a request the
proxy could not carry has always been.

**Every replay is a new `ExchangeId`.** The exchange a request was copied out
of is read and never written. No column was added to say "this was a replay":
that would be a schema change, an IPC model change, and a UI change for
something the traffic list already shows accurately. A replay is recorded as
having come from `127.0.0.1:0` — this machine, on no port — which is what
actually happened.

**Replay does not authenticate.** There is no login, no OAuth flow, no token
refresh, no credential discovery, and no credential store. `Authorization`,
`Cookie`, and an API key in a header of a service's own naming are request
data: they are copied out exactly, imported exactly, and sent exactly, and an
expired one comes back from the server as the `401` it is.

### The boundary between them

`proxy-engine` depends on `proxy-ipc`, so `proxy-ipc` cannot depend on
`proxy-engine`. The reach is therefore declared as a trait —
`proxy_ipc::RequestReplayer` — and implemented in the engine, the same way
`ExchangeSink` is declared in `proxy-storage` and implemented by the storage
worker. `proxy-ipc` opens no connection to anything, and a core started without
a replayer says it is not sending requests rather than accepting one and doing
nothing with it.

### The three operations

| | | |
| --- | --- | --- |
| `parse_curl` | `curl` | the request the command describes, the parser's notes, or the problem |
| `curl_from_traffic` | `exchange_id` | the command, and whether the captured body was truncated |
| `replay_request` | the request's own fields, flattened | the new `exchange_id`, the status, and the error if there was one |

**A cURL command that does not parse is a result, not an error.** An
`ErrorBody` message is a fixed phrase chosen in `core/ipc/` and may name
nothing a client sent — which is right, and which would make "unsupported cURL
option" unable to say *which* option. So `parse_curl` answers `ok: true` with
either a request or a structured problem, and the problem may name the option.
That name is the only thing out of a client's own command that ever comes back,
and it is checked first: short, printable ASCII, or it is left out.

`replay_request` is also `ok: true` for a replay that reached its destination
and failed. The request *was* replayed; what came back was a failure, and a
failure is a result. Only a request the core will not send at all — an
unusable URL, a header that could split a request in two, a
`Proxy-Authorization` — is an error, with the new `invalid_request` code.

**Nothing about any of this is logged.** A cURL command is exactly where a
bearer token lives, so no generated command, no imported command, and no value
out of either is written anywhere: not to a log, not into an error message, and
not through a `Debug` — every type on this path renders names, counts, and
sizes instead. The one thing the proxy prints about a replay is the line it
prints about every request, which is a method and a URL.

### In the interface

Copy as cURL sits in the traffic detail and in the toolbar, and writes to
`NSPasteboard.general` through a one-method protocol so that a test can say
what was copied without a test run replacing whatever the developer had
copied. The command is the **core's**: the app never assembles one out of the
parts of a row.

The import sheet is one sheet with two faces — a pasted command, and the
request it turned into — because it is one task. It is deliberately not a
request workspace: no tabs, no collections, no environments, no saved history,
and nothing kept when it closes. A body that is not text is shown as a size and
is not editable, because the only edit that field could offer is one that
corrupts it; replaying it unchanged is what a developer actually wants, and
there is a button that says the bytes are being thrown away for whoever does
not.

**A replay does not change what is selected.** The new exchange arrives on the
traffic subscription like any other and takes its place in the list. Selecting
it here would mean asking for an exchange the storage worker may not have
written yet, and answering that race with a spinner or a wrong "this exchange
is gone" is worse than letting the row appear and be clicked.

### What is not here

Neither a WebSocket handshake nor an event stream is replayable, and
neither is offered as a cURL command: one is a connection rather than an
exchange, and the other is a response that does not end. No Postman
collections, no environment variables, no scripting, no request tabs, and no
cookie or credential manager. Replay adds no rule behaviour and changes none.
Upstream proxy chaining and proxy authentication are not implemented, which is
why the options that need them are refused by name rather than accepted and
ignored.

The import sheet keeps nothing, and that is still true. What *is* kept lives
beside it and is a separate feature with a store of its own — see
**Saved requests** below.

## HAR export and import

```text
                 ┌──────────────────────────────┐
                 │           SwiftUI            │
                 │  HAR ▾  Export / Import      │  a path from a panel
                 │  Saved Requests  ⌄           │  view, edit, replay
                 └──────────────┬───────────────┘
                                │ IPC
                                ▼
                       ┌─────────────────┐
                       │    proxy-ipc    │
                       └────────┬────────┘
              ┌─────────────────┼─────────────────┐
              ▼                 ▼                 ▼
         proxy-har        proxy-storage      Replayer
      entry ⇄ exchange    saved_requests    (proxy-engine)
         a .har file      a row + a body
```

HAR and saved requests are two answers to "I want this request again": one
writes the whole session down for somebody else, and one keeps a single
request for tomorrow. They share a step and nothing else — the diagram above
covers both, and **Saved requests** below is the second of them.

### A HAR file is a file, and the core is what opens it

**This is the one pair of operations that names a path rather than carrying
the thing itself.** A HAR document is the captured traffic with its bodies in
it — megabytes of it — and one IPC request line is 64 KiB. So the app asks
macOS for a location through a save or open panel and sends the path; the
core writes or reads the file, and is the only thing that ever does.

It is the shape a Map Local rule already has — the client names a file and
the core opens it — and the boundary is unchanged by it: the socket is still
`0600` in a `0700` directory, so the only client that can name a path is this
user, who could write the file themselves. What *is* new is that the core
writes one, so the path is checked before anything is opened: absolute, not a
directory, and inside a directory that exists. Nothing is created but the
file, and nothing is appended to the path the client gave, so there is no
joining for a traversal to happen in.

**The app has no idea what HAR is.** It never assembles a document out of the
rows it happens to be holding and never parses one; the core is
authoritative for what was captured, so the core writes it down. That is the
same division Copy as cURL has.

### What an entry is

`core/har/` is a pure crate in the shape of `core/curl/`: it holds the HAR 1.2
model, the conversion both ways, and the file either direction lives in. It
depends on `proxy-storage` for the exchange and on nothing else of
ProxyLab's — it opens no database, evaluates no rule, and makes no network
request.

| | |
| --- | --- |
| a text body | `text` is the body's own characters |
| any other body | `text` is base64, and `encoding` says so |
| a truncated capture | `size` is the whole body, `text` is what was kept |
| an exchange with no response | status `0`, and the reason in `comment` |
| `cookies` | written empty: the headers carry them exactly |
| `headersSize` | `-1`: the raw header bytes are not kept |

**Truncation survives the round trip without a field of ProxyLab's own.**
`size` is how long the body *was* and `text` holds as much of it as the
capture limit allowed, so more `size` than bytes is a capture that was cut
short — which is exactly what it was before it was written. Nothing is added
to the format for it, so a file this crate writes is a file any other tool
reads.

**The three timings that are written add up to `time`**, because HAR says
they must: `connect` is how long the connection took to get hold of, `wait`
is the origin's think time, `receive` is the body. Blocked, DNS and SSL stay
`-1` — ProxyLab does not separate them out anywhere, and inventing them would
be a waterfall drawn from numbers nobody measured.

### Importing is reading, not sending

**An import produces records and opens no connection.** A HAR file somebody
sent you may describe requests that should never be made again, and reading
it must not be a way to make them. What comes out is
`TrafficExchange`es — the model the traffic list already draws — so an
imported entry is inspected, searched, filtered, copied as cURL and replayed
by the same code as a captured one. There is no second traffic model and no
importer-shaped view of an exchange.

They are stored, with ids of their own, and their own `startedDateTime` as
the moment they happened, so a colleague's session sorts into the list where
it belongs rather than at the top. What they are *not* is published on the
traffic event stream: that stream carries the exchanges the proxy finished,
and an imported one is not one. So a client reloads its list after an import,
the way it reloads its rules after changing one.

**A file from outside is bounded before it is read**: 64 MiB of file, ten
thousand entries, and each body held to the proxy's own capture limit. What
is left out is counted and reported rather than passed over in silence.

### What is exported

The exchanges somebody selected, or every exchange the filters match — one or
the other, never both, because a request naming ids *and* filters would export
something nobody asked for. A filtered export pages through the store exactly
as the traffic list does and writes each entry straight into the file, so the
size of an export is not the size of this process. It stops at ten thousand
entries and says so.

**Nothing is written under the real name until it is whole.** The bytes go to
a temporary file beside the destination and the destination is created by a
rename, so a failed export leaves either nothing or whatever was already
there — never half a HAR file with the name of a whole one.

**An export carries what the traffic carried**, credentials and cookies
included: the point of the file is that it reproduces the traffic. Which is
exactly why exporting is something a person does by name, through a panel, to
a location they chose — and why the panel says so.

### What is not here

HAR 1.2 alone: nothing writes 1.1 or 1.0, and there are no `pages`, no
`pageref`, and no parsed `cookies` array — the cookie headers are in `headers`
where they were sent. Nothing schedules, diffs, or compares two HAR files, and
nothing exports a WebSocket's frames as HAR: they are captured beside the
handshake for the inspector, and a HAR 1.2 entry has no place for them. An
imported exchange is not marked as imported: it is a record like every other,
which is the decision a replay already made for the same reason.

## Saved requests

A handful of requests somebody wants to send again, and deliberately not a
request workspace: no folders, no collections, no environments, no variables,
no authentication manager, and no scripting.

**Only what is needed to build the request again** is kept: a name, a method,
a URL, the headers, and the body. There is no response, no timing, and no
record of what happened when it was last sent — what happened is an exchange
in the traffic list with an id of its own, like every other request the proxy
has carried.

**A saved request is checked into a sendable one when it is written down.**
`proxy-storage` persists it and `proxy-http` decides what a request may be, so
the two meet at the IPC boundary: saving or editing one builds a `RequestSpec`
from it — the same check a replay makes — and stores the URL that parse
normalized. A URL this proxy could not send to is refused at the moment the
request is written rather than at the moment it would have mattered.

**The body is a `BodyCapture` rather than bytes.** A request saved out of
captured traffic carries the captured body, which the proxy's limit may have
cut short, so it knows how long the body *was* as well as how much of it is
here — and the interface says so rather than quietly sending less than the
original did. A body somebody typed is a whole body and is marked as one.

**Sending one is the replay every other request gets.** There is no
`replay_saved_request`: a client reads the saved request, shows it, lets it be
changed, and asks for the request to be sent through `replay_request` — the
one path through the rules, the TLS, the capture and the storage. A second
way in would be a second replay path to keep in step with all four.

**Persistence is a table of its own**, added by a migration that leaves the
traffic and the rules untouched, with the body placed where a captured one is
placed: nothing, a `BLOB`, or a file in the body directory under
`<saved request id>.saved`. Clearing the captured traffic removes
`<exchange id>.request` and `.response` and leaves a saved request's body
exactly where it is, because a saved request is not traffic.

### In the interface

Saved requests sit at the foot of the traffic sidebar, above the device
panels, for the reason those panels are there: the list above is a selection
of hosts and a saved request is not a host. Their rows are buttons rather than
list selections — the two selections in that window are the host filter and
the exchange being inspected, and this is neither.

The sheet that opens one is the cURL import sheet with a name field on top and
one more button at the bottom, and the editor in the middle is literally the
same view, because it is the same thing: a request somebody is about to send.
Save and Replay are separate on purpose — most edits made before sending are
not worth keeping — and neither closes the sheet, because a saved request is a
thing to send more than once.

Renaming happens in the row. A name is one field, and a sheet for one field is
a sheet to dismiss.

**Nothing on screen changes before the core says it changed**, which is the
rule the rules screen already follows. Saving, renaming, editing and deleting
all go to the core first, and the list is what came back.

### What is not here

No folders, collections, environments, variables, authentication managers, or
scripting. Nothing here authenticates: an `Authorization` header is request
data, copied out exactly, kept exactly, and sent exactly, and an expired one
comes back from the server as the `401` it is — the same position replay
already takes.

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

### How the boundary is carried

```
SwiftUI macOS app
        │
        │  Unix domain socket, one JSON object per line
        ▼
  proxy-ipc server
        │
        ├──▶ proxy-storage   traffic: list, get, delete, clear
        ├──▶ proxy-storage   saved requests: list, get, create, update, delete
        ├──▶ proxy-har       a .har file written from the store, and one read into it
        ├──▶ rule engine     rules: list, get, create, update, delete
        ├──▶ LanControl      whether the proxy is also on the local network
        └──▶ event stream    every exchange the proxy finishes
```

The Swift side of that socket is `Sources/ProxyLabCore/IPC/`. It is the app's
only way to the core, and it is one connection: a single reader decodes each
line and either hands it to the request whose `request_id` it carries or, when
it carries no `ok`, publishes it as an event. That is what lets several
requests be outstanding at once without an event ever being mistaken for an
answer. Socket I/O is asynchronous throughout and never runs on the main
thread; what the views read is a `@MainActor` model the client feeds.

Both screens share that one connection. `TrafficSession` and `RulesSession`
are two protocols over it — the traffic screen cannot reach a rule operation
and the rules screen cannot reach traffic — and `IPCClient` implements both.
Two protocols rather than one because they are two screens' worth of reach and
neither needs the other's, and one connection because a second socket would be
a second thing to reconnect, a second subscription to get wrong, and nothing
gained.

Where the socket is, the Swift side works out exactly as the Rust side does —
`PROXYLAB_IPC_SOCKET`, else `~/.proxylab/run/proxylab.sock` — in one place,
`IPC/IPCSocketLocation.swift`, which mirrors `IpcConfig::default_socket`.

**Transport.** A Unix domain socket, never TCP. The socket is
`~/.proxylab/run/proxylab.sock` by default, or wherever `PROXYLAB_IPC_SOCKET`
points. A socket cannot be reached from another machine, and its file
permissions are the access control: the directory is created `0700` and the
socket `0600`, so the boundary is *this user, on this machine*. There is no
authentication on top of that — a token would be a secret readable by the
same user, which is the same boundary with more moving parts. Authentication
becomes a real question only if a client is ever something other than the
local user.

**Framing and versioning.** One JSON object per line, in both directions.
Every message carries `"version": 1`; a request naming another version is
refused rather than guessed at. A request carries a `request_id`, which its
response echoes, so a client may have several in flight.

**Requests.** `list_traffic` (paged, with host, method, status, search —
including optional regex and case-sensitive flags — content type, HTTP
version, WebSocket, SSE, failed, and time-range filters, read from either end
of the order), `get_traffic`,
`delete_traffic`, `clear_traffic`, and `subscribe`. They are the whole of a
client's reach into ProxyLab: there is no path from a request to a file, to a
query of a client's choosing, or to the proxy's own state.

The developer-tool operations are `parse_curl`, `curl_from_traffic`, and
`replay_request`, added to the same version-1 protocol beside the rest. See
**cURL and replay** below for what each of them is; what matters here is that
they are additive — every operation that was in version 1 before is unchanged,
and so is every message it carries.

The local-network operations are `lan_status`, `enable_lan` and
`disable_lan`, added to the same version-1 protocol and additive in the same
way. All three answer with one shape — whether the proxy is on the local
network, the address it is bound to if it is, the port it listens on either
way, and the addresses this Mac is currently offering. `enable_lan` carries
one address and nothing else. See **The physical iPhone** below.

The saved-request operations are `list_saved_requests`, `get_saved_request`,
`create_saved_request`, `save_request_from_traffic`, `update_saved_request`
and `delete_saved_request`, added to the same version-1 protocol and additive
in the same way. Sending one is not among them: a client reads a saved
request and asks for it through `replay_request`, which is the one replay
path there is. See **Saved requests** above.

The HAR operations are `export_har` and `import_har`, and they are the one
pair that names a *file*: a HAR document does not fit in a 64 KiB request
line in either direction, so the client sends the path a person picked in a
panel and the core writes or reads it. It is the shape a Map Local rule
already has, and the path is checked before anything is opened. See
**HAR export and import** above.

The stream-message operations are `list_stream_messages` and
`get_stream_message`, added to the same version-1 protocol beside the traffic
ones. A list page is summaries — direction, kind, timestamp, size, SSE fields
— oldest first, paged by sequence, with no payloads. One message's payload is
asked for by id, which is the same split the traffic list already has, so a
busy socket cannot fill the IPC pipe. Payloads that exceed the inline body
threshold live in the body directory the way HTTP bodies do. `subscribe`
also attaches the caller to `stream_message_added` / `stream_messages_lost`;
there is no second subscription.

The rule operations are `list_rules`, `get_rule`, `create_rule`,
`update_rule`, and `delete_rule`, added to the same version-1 protocol beside
the traffic ones. A rule is sent as the `rules` crate spells it, for the same
reason an exchange is sent as `proxy-storage` spells it. `create_rule` carries
the rule's fields and the server mints its id and timestamps; `update_rule`
carries a rule id and only the fields to change, which is what makes "turn
this rule off" one field rather than the whole rule read back and sent again —
and what `enabled` is changed through. Everything is checked before anything
is stored: a matcher that could never match, a destination this proxy cannot
speak to, a status that is not one. Each change is written to the database and
then handed to the engine, in that order, so a rule the proxy is applying is
one that has been stored.

Rule changes are **not** published as events. The traffic event stream carries
finished exchanges, and every client of it — including the macOS app — treats
an event type it does not know as a protocol disagreement and reconnects, so
adding `rule_added` to that stream would break the clients this step promises
not to break. A client learns the result of a rule change from the response to
the change it made. A second event channel for rules is a thing to add when
something needs to watch rules it did not write.

So the macOS app reloads its rule list explicitly after every change it makes,
and its traffic subscription is unchanged by any of it. A rule changed by a
second client is seen the next time the rules screen is opened or reloaded —
and an `update_rule` for a rule that is no longer there comes back `not_found`,
which the app shows as "that rule no longer exists" rather than writing the
rule back.

`search` is a substring of captured traffic: host, URL (and so the path),
method, status, both `Content-Type` columns, both header lists, both captured
bodies (the bytes that were kept, including a truncated prefix), SSE event
name / id / data, and WebSocket text payloads. Binary WebSocket payloads
contribute only size and direction. Case folding follows SQLite's `LIKE`
(ASCII letters only) unless `search_case_sensitive` is set. `search_regex`
treats the same pattern as a regular expression and is refused when it will
not compile; there is no query language. Search runs as registered SQLite
functions so a page is still decided in SQL and a file-backed body is read
for the row being considered, then dropped — never collected for every
exchange and sieved in process. `order` is `newest` or `oldest`, and names
which end of the one stored order a page is read from; a cursor names a place
in that order, so a client that changes the order starts its paging again.
The new filter fields are optional, and a request that omits them means what
it has always meant: the newest first, unsearched, every content type and
version.

**Responses.** `{"version", "request_id", "ok", …}` — with `type` and
`result` when it worked, and a structured `error` (a `code` a client matches
on, and a message) when it did not. Error messages are fixed phrases chosen
in `core/ipc/`: nothing a client sent and nothing a captured exchange held is
ever reflected back or logged.

**Subscription.** A subscribed connection receives `traffic_added` events
carrying the same summary the traffic list shows — no headers and no bodies,
because an event is sent for every request the proxy carries. The summary
names the exchange, so a client that wants the rest asks for that one
exchange by id. The same subscription receives `stream_message_added` for a
captured WebSocket message or SSE event on an exchange that is already
listed — again a summary, not a payload — and `stream_messages_lost` when
this subscriber fell behind. A subscriber that cannot keep up on either
stream receives a lost-count rather than a list that is quietly wrong;
events are dropped for it rather than queued, so no client can slow the
proxy down.

**Failure.** A database that cannot be opened leaves the proxy running
without a history. A socket that cannot be bound stops the process: it is the
only way anything reaches ProxyLab from outside, and the usual reason to fail
is that another ProxyLab already has it.


## The iOS Simulator

```
┌ iOS SIMULATOR             ↻ ┐
│ [ iPhone 17 Pro — iOS 26.1 ▾]│   xcrun simctl list devices booted -j
│ ● Connected — via 127.0.0.…  │
│ [ Connect ]  [ Disconnect ]  │   xcrun simctl keychain … add-root-cert
└──────────────────────────────┘   SCDynamicStoreCopyProxies (read)
```

### A simulator has no proxy settings of its own

This is the fact the whole of this feature is shaped around, and it was
established rather than assumed. `simctl help` lists no proxy subcommand; the
`simctl` binary contains no proxy strings at all; CoreSimulator's only "proxy"
symbols are Darwin *notification* proxies. And a booted simulator asked
`CFNetworkCopySystemProxySettings()` — through `simctl spawn`, with a binary
built for the simulator — answers **this Mac's settings verbatim**, down to the
host's own `__SCOPED__` interface names. CFNetwork honours no proxy environment
variable either, so `SIMCTL_CHILD_http_proxy` is not a second way round it.

A simulator does not read a proxy configuration. It reads *ours*.

So there is nothing per-simulator to set, nothing to save before setting it,
and nothing to restore afterwards: **the preservation of a simulator's previous
proxy configuration is satisfied by never having written one.** The only lever
that would route a simulator is this Mac's own proxy configuration, and
ProxyLab does not touch it — `ProxyRoute` reads through
`SCDynamicStoreCopyProxies` and the write half of SystemConfiguration is
deliberately not used, not wrapped, and not reachable from anything the
interface can press.

### What Connect does

Three things, in order, and it says which of them it got through:

| | |
| --- | --- |
| the core is running | the window's own IPC connection, not a second probe |
| trust the CA | `simctl keychain <udid> add-root-cert`, Apple's documented command |
| check the route | is this Mac proxied to `127.0.0.1:9090`? |

The third is a check, not a change. When the route already points at the proxy
the simulator is connected and there is nothing left to do. When it does not,
**the certificate is trusted all the same** and the interface says exactly what
is missing — a half-success reported as a half-success, rather than a green dot
over a simulator whose traffic nothing will capture.

Connecting twice is two of the same command: `add-root-cert` exits zero for a
certificate the simulator already trusts, which is the whole of the
idempotency. There is no "is it installed?" query, because `simctl` has none
and inventing one would mean reading the simulator's trust store behind
Apple's back.

### What Disconnect does

It ends the session the app is keeping, and says what it did not undo. No proxy
setting was changed, so none is put back. The CA stays trusted, because
`simctl keychain` can add a certificate or `reset` the entire keychain and
nothing in between — and erasing a developer's simulator keychain is not the
opposite of adding one certificate to it. Both are said in the interface rather
than left to be discovered.

### Where the port and the certificate come from

`Sources/ProxyLabCore/Simulator/CoreLocations.swift`, which is to the proxy
port and the CA what `IPCSocketLocation` is to the socket: the Rust side's
rules, copied deliberately, in one place. `DEFAULT_LISTEN_ADDR` sets the port;
`CertificateAuthority::default_directory` and `store::CERTIFICATE_FILE` set the
certificate. Nothing else in the app spells either, and there is no preference
for them, because a second configuration source is a way for the app and the
core to disagree about where the proxy is.

When the core's `lan_status` — whose `port` is the port the proxy is actually
listening on — reaches the Swift client, `proxyEndpoint` becomes a value read
from the core and the constant becomes the fallback for a core that is not
answering.

### What is not here

No physical iPhone, no Android, no Network Extension, no transparent proxying,
and no automatic configuration of this Mac's proxy. Nothing boots, erases,
resets or shuts down a simulator. Nothing is installed into the macOS Keychain.
The proxy engine is untouched by all of it: this feature runs two command-line
tools and reads one system dictionary, and reaches the core for exactly one
thing — whether it is running.

## The physical iPhone

```
┌ IPHONE                     ↻ ┐
│ ● Off — only this Mac can …  │   lan_status
│ [ en0 — 192.168.1.42       ▾]│
│ [ Turn On ]      [ Setup… ]  │   enable_lan / disable_lan
└──────────────────────────────┘
```

### It is one address, chosen, and it is off until then

ProxyLab listens on `127.0.0.1:9090` and on nothing else. Enabling local
network access binds a **second** listener, on one address of this Mac that a
person picked, and disabling it closes that listener again. The loopback
listener is bound before any of this exists, is never rebound, and is never
stopped by it.

Nothing turns it on by itself. There is no setting that persists the choice,
no environment variable, and no "remember this": a ProxyLab that has just
started is a ProxyLab only this Mac can reach, every time, whatever was on
when it was last quit. That is deliberate — a LAN listener is an open HTTP
proxy for everything on the same Wi-Fi, and a thing with that consequence is
turned on by someone who meant to, in the session they meant it in.

It is never `0.0.0.0`. There is no parameter that would express it: the
address is one entry from the list the core offers, and `enable_lan` asks the
machine again at the moment of binding rather than taking the client's word,
so loopback, the unspecified address, and an address this Mac no longer has
are all refused.

### Which addresses are offered

`lan::discovery` reads the interfaces through `getifaddrs` each time it is
asked, and keeps none of it: a Mac moves between networks, and a list that was
true five minutes ago is a list of addresses that no longer bind. What is left
out is loopback, an interface that is not up and running, a point-to-point
interface (a VPN's `utun`), and a link-local `169.254` address — none of them
is a Wi-Fi a phone is on.

And anything that is not RFC 1918. That last filter is about safety rather
than usefulness: binding the proxy to a publicly routable address would put an
open HTTP proxy on the Internet, so `10/8`, `172.16/12` and `192.168/16` are
what ProxyLab will offer and the rest is refused rather than warned about. The
consequence, stated plainly: a Mac whose LAN is numbered some other way has no
address on offer, and this feature does not work there.

IPv4 only. An iPhone's manual Wi-Fi proxy setting takes a server address and a
port typed by hand, and an IPv6 address on a LAN is usually link-local with a
zone identifier that means something different on each machine.

### A phone's traffic is traffic

The second listener runs with the same interceptor, the same upstream
connection pool, the same capture and the same rule engine as the first. So
nothing past the accept loop knows a request came from a phone: it is
evaluated against the same rules, intercepted by the same CA, captured into
the same store with the same model, and published on the same event stream as
a request from a browser on this Mac. There is no iPhone traffic model, no
second capture, and no second pipeline.

The one thing the second listener does differently is on the way out. A
listener that stops normally closes the proxy's idle upstream connections,
because a stopping listener is a stopping proxy — which is true of the
loopback one and false of this one, since it can be switched off while the
proxy carries on. `Listener::sharing_upstream` is the whole of that
difference: it says whose pool it is, and the listener that owns it is the one
that closes it.

### How the certificate gets onto the phone

The phone is already sending every request through the proxy, so the proxy
answers one host by itself. `http1::setup` serves `proxylab.test` — under the
`.test` TLD, which RFC 6761 reserves and no registry will delegate, so
answering it takes nothing away and every other host is forwarded exactly as
before. `/` is a page with the steps on it, `/ca.cer` is the CA certificate as
DER with the media type that makes iOS hand it to Settings, and `/ca.pem` is
the same certificate as text.

It is the certificate and only the certificate: the public half, the same
bytes the proxy already presents to every client it intercepts for. The
private key is not read there and has no route that would reach it. The
request is captured like any other, and it is answered before the rules are
consulted — a rule that blocked it would be a phone unable to fetch the
certificate that would let the rest of its traffic be seen at all.

Nothing is installed by any of this. Trusting the certificate is two separate
things a person does on the device — installing the profile, and then turning
full trust on in **Certificate Trust Settings** — and the page and the setup
sheet both say so, because the second one is the step everybody misses.

### What is not here

No automatic configuration of the phone, no USB, no Xcode, no device
management, and no app installed on it. Nothing changes this Mac's system
proxy settings, its firewall, or its keychain — turning this on adds a
listening socket, and that is the whole of what changes on this machine.
There is no authentication on the LAN listener either: the boundary ProxyLab
has is the IPC socket's file permissions, and that boundary does not stretch
over a network. So the interface says, in a sentence and not a coloured dot,
that any device on this Wi-Fi can use the proxy while it is on.

The IPC socket itself is unmoved by all of this. Local network access widens
what can reach the *proxy*; the captured traffic, the rules and the controls
stay reachable only by this user on this machine.


## The Android Emulator

```
emulator ── global http_proxy ──▶ 10.0.2.2:9090 ──▶ 127.0.0.1:9090
                                  │                 the proxy already there
                       the emulator's alias for
                       this Mac's loopback
```

### This one really does connect

The iOS Simulator has no proxy configuration of its own and reads this Mac's.
An emulator has exactly one — `global http_proxy` — and `adb` can read and
write it. So Connect here is a change rather than a check, and everything about
the design is about making that change exactly reversible.

**No second listener, and no LAN exposure.** `10.0.2.2` inside the standard
emulator is an alias for the host's loopback interface, so the proxy the core
already runs on `127.0.0.1` is reachable as it stands. This was verified rather
than assumed: a loopback-only listener on this Mac received an emulator's
request to `10.0.2.2`. Nothing about the Mac's own proxy settings, its
firewall, or the core's listener is touched, and the physical-iPhone LAN
listener is not turned on for an emulator.

**Only the standard emulator is offered it.** `EmulatorCatalog` lists serials
beginning `emulator-` and nothing else, so a physical device (a hardware
serial) and a Genymotion or wirelessly connected one (`host:port`) never appear
— `10.0.2.2` means nothing to either, and offering it would be offering a
wrong answer.

### There are three proxy states, not two

```
null        the key is absent — the emulator has never had one
:0          the key is there and means "no proxy"
host:port   a proxy
```

`null` and `:0` both send traffic direct, and they are **not the same stored
value**. So `unset` is restored with `settings delete` and `disabled` with
`settings put :0`, and the difference survives the round trip. An emulator that
already had a corporate proxy gets *that* proxy back on Disconnect, never a
clear.

The exclusion list beside it is never read and never written, so whatever was
there survives untouched. It is not ProxyLab's to change.

### Saved once, not on every connect

Connecting an emulator that is already connected must not record ProxyLab's own
address as "what was there before" — a later Disconnect would then restore
ProxyLab, which restores nothing. The saved value is written the first time a
serial is connected and left alone afterwards. A connect that finds ProxyLab's
own address already set with nothing saved records *no proxy* instead: an
emulator pointing at ProxyLab is ProxyLab's own leftover from a run that did
not finish, not a configuration anyone chose.

Every write is **read back** before it is reported: `settings put` exits zero
for a write a later read does not agree with, and a proxy reported as set and
not set is the one failure that would waste an afternoon.

### An emulator that goes away

Shutting one down does not clear the setting — it is in the settings database
and survives a reboot. So a vanished emulator is reported plainly and its saved
value is **kept**: start it again, press Disconnect, and the developer's own
setting goes back.

### No shell, ever

`Process` is given an executable and an argument vector. There is no
`/bin/sh -c` and nothing builds a command string. The one place that deserves a
second look is `adb shell`, which joins what follows and hands it to a shell
*on the device* — so the values that reach it are checked instead, and a saved
proxy value that is not a single safe word is refused and reported rather than
sent back.

`adb` itself is looked for at `ANDROID_HOME`, then `ANDROID_SDK_ROOT`, then
Android Studio's own `~/Library/Android/sdk`, and only then on `$PATH` — an
application launched from the Dock does not inherit a login shell's `PATH`, so
the location that does not depend on how ProxyLab was started comes first.
Nothing reads or writes a shell configuration file.

### The CA, and what it does not buy

ProxyLab pushes `ca-cert.pem` to `/sdcard/Download/proxylab-ca.crt` and tells
the developer what to tap. That is all it does, because there is no adb command
that installs a user CA and every other route is one this project refuses:
rooting the emulator, remounting `/system`, or writing into the system
certificate store.

**A user-installed CA is not trusted by every app, and ProxyLab does not
pretend otherwise.** Since Android 7 an app targeting API 24 or later does not
trust user CAs unless it opts in with a network security configuration:

| | |
| --- | --- |
| browsers | usually work |
| a debug build that opts in | works |
| an ordinary release app | will not trust it |
| an app that pins its certificates | never will |

HTTP is captured regardless — the proxy setting is all that needs. HTTPS
interception depends on the app at the other end, and the interface says so
rather than implying the CA makes everything readable. Pinning, and apps that
deliberately reject user CAs, are not something this step tries to defeat.

## Transparent capture on macOS

```
                 explicit                           transparent
                 ────────                           ───────────
  app with a proxy setting                 any app, no setting at all
          │                                          │
          │ HTTP / CONNECT                           │ connect() to 93.184.215.14:443
          ▼                                          ▼
  127.0.0.1:9090                           macOS ─▶ Network Extension
  (the explicit listener)                           (ProxyLabTransparentProxy)
          │                                          │ CONNECT 93.184.215.14:443
          │                                          │ ProxyLab-Source-Pid: 501
          │                                          ▼
          │                                127.0.0.1:9091
          │                                (the transparent bridge)
          │                                          │
          └──────────────┬───────────────────────────┘
                         ▼
     TLS MITM · HTTP/1.1 · HTTP/2 · WebSocket · SSE · rules
     breakpoints · throttling · upstream pool · capture · storage
                         │
                         ▼
                     Internet
```

### Which Apple API, and why

**`NETransparentProxyProvider`**, delivered as a **system extension** and
managed with **`NETransparentProxyManager`**. It is the one Network Extension
provider whose job is exactly this: macOS hands it each TCP (or UDP) flow an
application opens, by rules the provider writes, before any byte leaves the
machine, and the provider either claims the flow or returns it to macOS to
carry as if nothing were installed. Nothing about the Mac's proxy settings,
routes, or DNS changes, and it is not a VPN.

The others were considered and are the wrong shape:

| | |
| --- | --- |
| `NEAppProxyProvider` (per-app VPN) | only applies to apps an MDM profile names |
| `NEPacketTunnelProvider` | packets, not flows: a user-space TCP stack, and a VPN |
| `NEFilterDataProvider` | can look at and drop flows, cannot redirect them |
| `NEDNSProxyProvider` | DNS only |
| setting the system proxy | changes the user's permanent settings, and misses apps that ignore them |

A system extension rather than an app extension because that is the only
packaging macOS offers a transparent proxy outside the Mac App Store.

### The extension holds no proxy logic

`ProxyLabTransparentProxy` decides which flows to take and copies bytes.
It does not terminate TLS, parse HTTP, evaluate rules, or record anything,
and it depends on nothing of the app's and nothing of the core's — only on
`TransparentProxyKit`, which is Foundation. Every flow it claims goes to the
core, and from the bridge onwards it is the same code a request through the
explicit proxy runs: there is no second TLS stack, no second HTTP parser, no
second capture, and no second rule engine.

It asks macOS for **outbound TCP to ports 80 and 443, IPv4 and IPv6, with
loopback excluded**. Everything else — other ports, UDP, and therefore QUIC
and DNS — never reaches it and is carried by macOS directly.

### The bridge speaks `CONNECT`

The extension tells the core where a flow was going with
`CONNECT address:port HTTP/1.1` on `127.0.0.1:9091`, and the core answers
`200 Connection Established` once it has connected to that address. It is
the framing the explicit listener already reads, parsed by the same
`ConnectTarget`, so there is no wire format of ProxyLab's own to keep in step
on two sides of a process boundary. The one header that is ProxyLab's,
`ProxyLab-Source-Pid`, is for loops (below).

It is a **separate listener** — `core/proxy-engine/src/transparent/`, bound
beside the explicit one and sharing its interceptor, pool, capture, rules and
controls — because two things about a captured flow differ from a `CONNECT` a
client wrote, and neither may change what the explicit proxy does:

- **The host is not in the target.** An application connects to an address.
  The name it means is the one in its TLS `ClientHello`, so the bridge reads
  the hello first (`Interceptor::accept_client_named`, rustls's own
  `LazyConfigAcceptor`), mints the certificate for that name, and verifies the
  origin's certificate against the same name — exactly what the application
  would have checked itself. A hello with no SNI is answered and verified as
  the address. The upstream socket goes to the address the application chose;
  the name is not looked up again.
- **The flow need not be TLS.** The first byte decides: `0x16` is a TLS
  handshake record and goes to interception; anything else is read as
  HTTP/1.1, each origin-form request is given the absolute-form target a
  proxy client would have written — from its `Host` header, or the flow's
  address when it has none — and handed to the explicit proxy's own
  `service::proxy`.

An explicit `CONNECT` to an address is still answered as that address, as
it always was. The explicit listener's code path is unchanged.

The bridge is loopback-only and fixed, like the explicit listener, and opens
nothing to anything the explicit listener does not already. It starts with the
core; nothing connects to it until the extension is turned on. A port that
cannot be bound is reported and the core runs without it.

### Loops

The core's own connections to origin servers are TCP flows to port 443 and
the extension sees them. Handed back to the core, each would open another,
which would be handed back too. Three things stop that, and each would be
enough on its own for the case it covers:

1. **Loopback is never claimed.** Excluded in the extension's network rules
   and refused again by `FlowPolicy`. The bridge, the explicit listener, and
   the IPC socket (a Unix socket, which is never a flow at all) are all
   loopback, so ProxyLab's control and bridge traffic cannot enter the path.
   A provider's own connections are also never handed back to it by macOS.
2. **The core recognises its own process.** The extension names the flow's
   source process — the pid out of its audit token — in
   `ProxyLab-Source-Pid`. A flow from the core's own pid is answered
   `508 Loop Detected` *before anything is connected to*, so the recursion
   cannot start. The extension then carries that flow directly and
   remembers the process by pid **and pid version**, so the core's later
   connections are never claimed and a reused pid is not mistaken for it.
   This needs no signing identity and no path, which a `cargo run` binary does
   not have in any stable form.
3. **The bridge refuses to connect to itself.** A destination that resolves
   to one of the core's own listening ports on loopback or the unspecified
   address is refused and recorded as a rejected exchange.

### When the core is not running

Every claimed flow would fail, and a Mac would lose its web traffic to a
debugging tool that is not even open. So a bridge that cannot be reached is
remembered for five seconds, during which the extension claims nothing; the
flow that found out is carried directly by the extension. Nothing is captured
meanwhile, and the app's panel says so in red rather than showing a green
"on" over a Mac whose traffic is passing straight through.

### Lifecycle, and who decides

```
Turn On ─▶ OSSystemExtensionRequest.activationRequest
             │  macOS may ask for approval in System Settings ›
             │  General › Login Items & Extensions › Network Extensions
             ▼
           NETransparentProxyManager.saveToPreferences
             │  macOS may ask to allow ProxyLab to add proxy configurations
             ▼
           connection.startVPNTunnel ─▶ the extension's startProxy

Turn Off ─▶ connection.stopVPNTunnel, and the configuration disabled
Quit     ─▶ connection.stopVPNTunnel
```

**Nothing is turned on at launch.** The app reads what macOS says and
changes nothing; the configuration is never on demand; and quitting the app
stops it. The one thing that continues on its own is a Turn On that stopped to
wait for approval: once macOS reports the extension allowed, the switch that
was pressed finishes — in that session only. The extension stays installed
after Turn Off, and is removed the way any system extension is.

**The app never touches the system proxy.** A transparent proxy is a Network
Extension configuration of its own, listed under System Settings › Network ›
VPN & Filters, and ProxyLab reads and writes only the one whose provider is
its own — another app's transparent proxy on the same Mac is never read,
changed, or stopped.

### The boundaries

| | | |
| --- | --- | --- |
| app → core | the IPC socket | unchanged: traffic, rules, controls, as before |
| app → macOS | SystemExtensions, NetworkExtension | install, save, start, stop |
| app → extension | `NETunnelProviderSession.sendProviderMessage` | `{"type":"status"}`, answered with counts and a sentence |
| extension → core | TCP, `127.0.0.1:9091` | `CONNECT`, then the flow's bytes |

The app does not talk to the core about any of this, so **no IPC message,
wire model, or schema changed**. The extension's status carries how many flows
went where and whether the bridge answered — never a host, a URL, or anything
a flow carried. Captured traffic reaches the app the one way all traffic does:
as exchanges on the IPC socket.

### Signing

The extension and the app's ability to install it rest on restricted
entitlements that macOS honours only with a provisioning profile:
`com.apple.developer.system-extension.install` on the app, and
`com.apple.developer.networking.networkextension` = `app-proxy-provider` on
both. The extension is also sandboxed, with `network.client` and an app
group its Mach service sits under. `Scripts/make-app-bundle.sh` builds and
signs both when given a team, an identity and the two profiles, and otherwise
signs ad hoc with no entitlements — the app then runs exactly as before and
the panel says transparent capture needs a signed build. Only an app in
`/Applications` may install a system extension. Development signing is what is
built; Developer ID signing, notarization and packaging are not.

### What is not here

UDP of any kind, so no QUIC and no DNS; ports other than 80 and 443;
packet-level anything; per-application rules; an iOS Network Extension. A
flow's source application is not recorded in the captured exchange — the
exchange model is unchanged, and a bridged exchange's client address is the
extension's loopback socket. HTTP/2 over cleartext (`h2c` prior knowledge) is
not carried on port 80. Apps that pin certificates, and some macOS services,
fail while capture is on, because every HTTPS connection is intercepted; the
panel says so while it is on. Chrome prefers QUIC for many sites, and QUIC is
UDP, so those requests are not captured unless Chrome falls back to TCP.


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
  rendered offscreen with `ImageRenderer` like the other views.
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
  so a test run never reconfigures a developer's emulator. -->

# ARCHITECTURE.md

Intended high-level architecture for **ProxyLab**, a macOS HTTP/HTTPS debugging proxy for developers.

> **Context loading rule:** This file is the compact architecture index. Do **not** load every context file for every task. A Cursor prompt should name only the context files relevant to that step. Follow `AGENTS.md` and `ROADMAP.md` as usual.

## System shape

ProxyLab has three major layers:

1. **Rust core** — networking, TLS interception, HTTP/1.1 + HTTP/2, rules, capture/storage, replay, device LAN exposure, and IPC.
2. **macOS client** — SwiftUI/AppKit presentation and configuration. It does not implement proxy logic.
3. **Device / system integrations** — iOS Simulator tooling, Android Emulator tooling, LAN physical-iPhone support, and the macOS Network Extension.

Traffic converges on the same core pipeline:

```text
Mac Browser / iOS Simulator / Android Emulator / Physical iPhone / any Mac app
        ↓
Proxy Engine
        ↓
HTTP / HTTPS / HTTP/2
        ↓
Rule Engine
        ↓
Capture / Storage / Events
        ↓
IPC
        ↓
macOS UI
```

The Network Extension is an alternate ingress path into the same engine; it does not create a second HTTP/TLS/rules/capture stack.

## Repository map

| Area | Location | Context |
|---|---|---|
| macOS app | `macos/ProxyLab/` | `macos-ui.md` |
| Rust core | `core/proxy-engine/`, `core/tls/`, `core/http/`, `core/storage/` | `core.md` |
| Rules | `core/rules/` | `rules-engine.md` |
| IPC | `core/ipc/` | `ipc.md` |
| cURL + replay | `core/curl/`, replay in `core/proxy-engine/` | `replay.md` |
| HAR | `core/har/` | `har.md` |
| Saved requests | storage + IPC + macOS UI | `saved-requests.md` |
| WebSocket/SSE | `core/proxy-engine/` + stream storage/IPC/UI | `streaming.md` |
| Breakpoints/throttling | `core/proxy-engine/src/controls/` | `breakpoints.md` |
| iOS Simulator | `device-tools/ios/SimulatorTools/` | `device-tools.md` |
| Android Emulator | `device-tools/android/AndroidTools/` | `device-tools.md` |
| Physical iPhone | LAN listener + macOS device UI | `device-tools.md` |
| Network Extension | `macos/ProxyLabTransparentProxy/`, `core/proxy-engine/src/transparent/`, `macos/ProxyLab/Sources/TransparentProxyKit/` | `network-extension.md` |
| Testing | Rust, Swift, device-tool, and integration suites | `testing.md` |

## Context files

- `macos-ui.md` — macOS UI, traffic inspector/navigation, pinned domains.
- `core.md` — core architecture, data flow, HTTP/1.1/HTTP/2, upstream pooling, core/UI separation.
- `rules-engine.md` — matching, precedence, actions, transformations, rule application and persistence.
- `replay.md` — cURL parsing/serialization and replay.
- `har.md` — HAR 1.2 export/import.
- `saved-requests.md` — saved request model, persistence, editing and replay integration.
- `streaming.md` — WebSocket/SSE and stream-message inspection/editing.
- `breakpoints.md` — breakpoints, body staging and throttling.
- `ipc.md` — Unix socket, JSON-lines protocol, operations, events and wire boundary.
- `device-tools.md` — iOS Simulator, physical iPhone/LAN, Android Emulator.
- `network-extension.md` — transparent macOS capture and system-extension architecture.
- `testing.md` — testing strategy and existing coverage.

## Cross-cutting rules

- Rust core is headless and independent of Swift/macOS UI.
- macOS UI communicates with the core through the IPC boundary; it does not access core storage directly.
- Prefer additive IPC changes; public wire/data-model changes must be explicitly reported.
- Do not create a second implementation of an existing protocol pipeline when a feature can reuse the core path.
- Do not silently modify macOS system proxy settings, shell configuration, user firewall/keychain, or developer device state outside the feature's explicit reversible operation.
- Keep device-platform tooling independent.
- Rules decide routing/transformation; breakpoints are interactive holds; throttling is live transport interference.
- Capture represents what actually passed through the proxy; do not add original-vs-modified history unless explicitly required.
- Use existing storage/body limits and IPC size limits rather than inventing parallel mechanisms.
- Security-sensitive values such as credentials, headers, bodies, and cURL contents must not be logged.

## Feature context selection for Cursor prompts

Use only the smallest relevant set. Typical examples:

- **Rule change:** `rules-engine.md` + `core.md` only if engine integration changes.
- **WebSocket/SSE:** `streaming.md` + `core.md`; add `breakpoints.md` only if pauses/throttling are touched.
- **Search/traffic inspection:** `macos-ui.md` + `ipc.md`; add `core.md` if storage/query implementation changes.
- **iOS Simulator:** `device-tools.md`; add `macos-ui.md` only for the macOS panel.
- **Android Emulator:** `device-tools.md`; add `macos-ui.md` only for the macOS panel.
- **Physical iPhone/LAN:** `device-tools.md` + `core.md`; add `ipc.md` if wire operations change.
- **Network Extension:** `network-extension.md` + `core.md`; add `macos-ui.md` only for the app panel.
- **Replay/cURL:** `replay.md` + `core.md` only when replay pipeline internals change.
- **HAR:** `har.md` + `ipc.md` if wire operations change.
- **Saved requests:** `saved-requests.md` + `replay.md`; add `ipc.md` for protocol changes.
- **Breakpoints/throttling:** `breakpoints.md` + `core.md`.
- **Tests only:** `testing.md` plus the feature context being tested.

## Important fixed constraints

- Explicit proxy listener: `127.0.0.1:9090`.
- Transparent bridge: `127.0.0.1:9091`.
- Default IPC socket: `~/.proxylab/run/proxylab.sock`, overridable by `PROXYLAB_IPC_SOCKET`.
- LAN exposure is disabled by default and uses one explicitly selected RFC1918 address.
- Do not automatically quit/modify another proxy such as Proxyman during manual verification.
- Do not modify `~/.zshrc`.
- Prefer scratch HOME/database/socket/CA locations for manual verification when practical.
