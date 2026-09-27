# ProxyLab for macOS

The native macOS client: a traffic inspector for the exchanges the Rust core
captures.

It holds no proxy logic. It does not listen on a port, terminate TLS, parse
HTTP, evaluate rules, or own the capture store — it draws what comes over the
IPC socket, and the only things it sends back are the five operations the
protocol defines. If the core is not running there is nothing to show, and the
window says so.

```
ProxyLab.app (SwiftUI)
      │
      │  Unix domain socket, one JSON object per line
      ▼
 proxy-ipc  ──▶  proxy-storage   list, get, delete, clear
      └───────▶  event stream    every exchange the proxy finishes
```

## Running it

The core first, then the app. The app connects on launch, and retries on its
own if the core is not there yet.

```bash
# 1. The Rust core, from the repository root.
cargo run --manifest-path core/Cargo.toml --bin proxy-engine

# 2. The macOS client, from this directory.
swift run ProxyLab
```

`swift run` is the development path. For an application you can double-click,
keep in the Dock, and run with no terminal and no Xcode:

```bash
./Scripts/make-app-bundle.sh ~/Applications
open ~/Applications/ProxyLab.app
```

Xcode opens `Package.swift` directly — there is no `.xcodeproj` to keep in
step with the sources.

## Transparent capture (Network Extension)

The **THIS MAC** panel at the foot of the sidebar captures every app's HTTP and
HTTPS with no proxy setting, through a Network Extension
(`NETransparentProxyProvider`, run as a system extension). It hands each flow
to the core's bridge on `127.0.0.1:9091`, which the core opens on its own at
startup; the explicit proxy on `9090` is unchanged and works with or without it.

It needs a **signed** build. An ad-hoc build runs as before and the panel says
transparent capture is unavailable.

### One-time setup (Apple Developer account)

1. Register two App IDs under your team, with explicit bundle identifiers:
   - `com.proxylab.ProxyLab` — capabilities **Network Extensions** and
     **System Extension**
   - `com.proxylab.ProxyLab.TransparentProxy` — capability **Network Extensions**

   If `com.proxylab.ProxyLab` is taken, pick your own and set
   `PROXYLAB_BUNDLE_ID` to it below; the extension's is always that plus
   `.TransparentProxy`.
2. Make a **macOS App Development** provisioning profile for each, including
   this Mac, and download both.
3. Build, signed, into `/Applications` (macOS installs system extensions from
   nowhere else):

```bash
PROXYLAB_TEAM_ID=AB12CD34EF \
PROXYLAB_SIGNING_IDENTITY="Apple Development: Your Name (XXXXXXXXXX)" \
PROXYLAB_APP_PROFILE=~/Downloads/ProxyLab_Dev.provisionprofile \
PROXYLAB_EXTENSION_PROFILE=~/Downloads/ProxyLab_TransparentProxy_Dev.provisionprofile \
./Scripts/make-app-bundle.sh /Applications
```

The script refuses a profile that is for another App ID or lacks a capability,
before building anything.

### Turning it on

1. Start the core, then open `/Applications/ProxyLab.app`.
2. Press **Turn On** under THIS MAC. The first time, macOS asks you to allow the
   extension in **System Settings › General › Login Items & Extensions ›
   Network Extensions**, and then to allow ProxyLab to add a proxy
   configuration. Capture turns on once both are allowed.
3. Browse. HTTPS is readable once the ProxyLab CA is trusted, exactly as for the
   explicit proxy; nothing installs it for you.

**Turn Off**, or quitting ProxyLab, puts every connection back to direct.
Nothing is turned on at launch. While it is on, apps that pin their
certificates and some macOS services fail, because every HTTPS connection is
intercepted.

What is captured: TCP to ports 80 and 443. Not captured: UDP (so QUIC and DNS),
other ports. Chrome uses QUIC for many sites; turn it off in
`chrome://flags/#enable-quic` to see those requests. See **Transparent capture
on macOS** in `ARCHITECTURE.md` for how flows reach the core and how loops are
prevented.

To remove the extension: `systemextensionsctl list`, then delete ProxyLab.app
from `/Applications`, or remove the configuration under System Settings ›
Network › VPN & Filters.

## Where the socket is

The app and the core work out the socket the same way, in that order:

| | |
| --- | --- |
| `$PROXYLAB_IPC_SOCKET` | if it is set and not empty |
| `~/.proxylab/run/proxylab.sock` | otherwise |

One place in the app knows this — `IPC/IPCSocketLocation.swift` — and it
mirrors `IpcConfig::default_socket` in `core/ipc/src/socket.rs`. The variable
is read from the process environment: set it for one run, or in an Xcode
scheme. Nothing writes a shell configuration file.

```bash
PROXYLAB_IPC_SOCKET=/tmp/proxylab.sock swift run ProxyLab
```

The window's status bar shows the socket it is reading from.

## Layout

```
macos/ProxyLab/
├── Package.swift
├── Sources/
│   ├── ProxyLab/App/          the entry point, and nothing else
│   ├── ProxyLabTransparentProxy/   the Network Extension (a system extension)
│   ├── TransparentProxyKit/   what the app and the extension agree on
│   └── ProxyLabCore/
│       ├── IPC/               the socket, the framing, the client
│       ├── Models/            the wire models and the protocol messages
│       ├── Services/          the view model, and what it shows
│       └── Views/             the window, the list, the inspector
├── Tests/ProxyLabCoreTests/
├── Resources/Info.plist       what makes the executable an application
├── Resources/*.entitlements   what a signed build is signed with
├── Resources/TransparentProxy/   the extension's Info.plist and entitlements
└── Scripts/make-app-bundle.sh
```

Two targets because a test target cannot reliably import an executable that
carries `@main`: everything but the entry point is in `ProxyLabCore`, which
the tests import.

## Tests

```bash
swift test
```

Nothing has to be installed and nothing has to be running: the socket is behind
a protocol, so the client, the protocol, the models, and the view model are all
tested without one, and the transport is tested against a Unix socket the test
suite binds itself.

There is one suite that does want the real core — it is skipped unless a
running one is pointed at:

```bash
# with the core running
PROXYLAB_IPC_SOCKET=~/.proxylab/run/proxylab.sock swift test
```

## What this app does not do

Map Local, Map Remote, block rules, replay, cURL import and export, pinned
domains, HTTP/2, WebSockets, SSE, and device tooling are not here. See
`ROADMAP.md`: a phase listed there is built when it is asked for.
