# ProxyLab Saved Requests Context

Load this file for saved-request persistence, editing, and UI/replay integration.

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
