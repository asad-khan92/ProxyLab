# ProxyLab Rules Engine Context

Load this file for rule matching, precedence, actions, transformations, rule application, persistence/concurrency, or Rules UI work.

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

`Sources/ProxyLabCore/Views/Rules/` is where a rule is written. A traffic row can
open the same editor: its context menu asks for one kind of rule, a sheet lists
the rules of that kind so one can be turned on or removed, and New Rule opens
this editor filled in from that exchange. Nothing else in the app writes a rule.
What the editor does is the five rule operations and nothing else: list them,
create one, change one, remove one, and read one back.

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
