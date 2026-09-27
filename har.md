# ProxyLab HAR Context

Load this file for HAR 1.2 export/import work.

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
