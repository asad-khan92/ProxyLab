# ProxyLab IPC Context

Load this file for the Unix socket, JSON-lines protocol, IPC operations/events, search/filter wire contract, or Swift/Rust boundary details.

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
