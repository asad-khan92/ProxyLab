# ProxyLab cURL and Replay Context

Load this file for cURL import/export, replay, RequestSpec, or replay IPC/UI work.

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
