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

Open `ProxyLab.app`. The app launches the `proxy-engine` executable inside
its own bundle, waits until that process is ready, and starts transparent
capture. Stop in the toolbar turns capture off and stops that process.
Command-Q does the same shutdown. Nothing in normal use runs Cargo.

`swift run ProxyLab` is the development path for the UI. It is not an app
bundle, so it has no engine to launch; use the bundle below for the real
lifecycle. Xcode opens `Package.swift` directly — there is no `.xcodeproj`
to keep in step with the sources. Headless tests still start the core
themselves.

A developer can ask a running development app to quit, and see whether
`9090` and `9091` are still held by the bundled engine:

```bash
./Scripts/stop-proxylab.sh
```

That script quits ProxyLab.app and signals only a `proxy-engine` inside a
ProxyLab.app bundle. It does not signal any other process.

## Building ProxyLab.app

`Scripts/make-app-bundle.sh` builds a release app and embeds the Network
Extension. The argument is the directory the app is written into. The result
is always `<directory>/ProxyLab.app`. With no argument that directory is
`macos/ProxyLab/.build`.

The script builds the release `proxy-engine` and embeds it at
`Contents/MacOS/proxy-engine`. It does not run that binary, install the
extension, enable it, change the system proxy, or write a shell configuration
file. A previous `ProxyLab.app` in the destination is replaced only after the
new one has been signed and checked.

### Unsigned local build

No signing variables. The app is ad-hoc signed so it launches, the extension
is in the bundle, and it has no provisioning profile and no restricted
entitlements. Transparent capture stays unavailable; the capture control
reports that, and the THIS MAC panel says a signed build is required. The
bundled proxy engine still starts. Everything else works.

```bash
./Scripts/make-app-bundle.sh
open .build/ProxyLab.app
```

Check the bundle without an Apple account:

```bash
./Scripts/verify-app-bundle.sh .build/ProxyLab.app
./Scripts/packaging-checks-test.sh
```

### Signed development build

All four variables, or none. A partial set is an error, and it is reported
before the build starts. Nothing in the repository is a team id, an identity,
or a profile.

| Variable | What it is |
| --- | --- |
| `PROXYLAB_TEAM_ID` | The 10-character Apple Developer team id |
| `PROXYLAB_SIGNING_IDENTITY` | The codesigning identity name `security find-identity -v -p codesigning` prints. The team is that certificate's organizational unit, not the id in parentheses |
| `PROXYLAB_APP_PROFILE` | A macOS App Development profile for the app id |
| `PROXYLAB_EXTENSION_PROFILE` | One for `<app id>.TransparentProxy` |
| `PROXYLAB_BUNDLE_ID` | Optional. Defaults to `com.proxylab.ProxyLab` |

Register those two App IDs under the team. The app needs **Network Extensions**
and **System Extension**. The extension needs **Network Extensions**. If
`com.proxylab.ProxyLab` is already taken, set `PROXYLAB_BUNDLE_ID`; the
extension's id is that value plus `.TransparentProxy`, which is the suffix
`SystemTransparentProxyControl` looks up. Make a **macOS** development profile
for each id, include this Mac, and download both. A wildcard App ID, an iOS
profile, or an expired profile is refused. The signing identity may be a Mac
Developer, Apple Development, or Developer ID Application certificate; its
organizational unit must be the team id. Notarization is not implemented.

The profiles must grant the restricted entitlements: `app-proxy-provider` on
both, System Extension install on the app, and the app group `group.<app id>`
on the extension. App Sandbox and `com.apple.security.network.client` are
unrestricted macOS entitlements (TN3125). The extension is signed with both
from `TransparentProxy.entitlements`, and a signed bundle is checked for them
on the Network Extension itself.

macOS installs a system extension only from `/Applications`:

```bash
PROXYLAB_TEAM_ID=AB12CD34EF \
PROXYLAB_SIGNING_IDENTITY="Mac Developer: Your Name (XXXXXXXXXX)" \
PROXYLAB_APP_PROFILE=~/Downloads/ProxyLab_Dev.provisionprofile \
PROXYLAB_EXTENSION_PROFILE=~/Downloads/ProxyLab_TransparentProxy_Dev.provisionprofile \
./Scripts/make-app-bundle.sh /Applications
open /Applications/ProxyLab.app
```

`AB12CD34EF` above is a placeholder team id, not this project's team. Copy
`PROXYLAB_SIGNING_IDENTITY` from `security find-identity`. The characters in
parentheses are part of the display name; the script reads the team from the
certificate.

### Checking a bundle

```bash
./Scripts/verify-app-bundle.sh /Applications/ProxyLab.app
codesign --verify --deep --strict --verbose=2 /Applications/ProxyLab.app
codesign -d --entitlements - --xml /Applications/ProxyLab.app
codesign --verify --strict --verbose=2 \
    /Applications/ProxyLab.app/Contents/Library/SystemExtensions/com.proxylab.ProxyLab.TransparentProxy.systemextension
codesign -d --entitlements - --xml \
    /Applications/ProxyLab.app/Contents/Library/SystemExtensions/com.proxylab.ProxyLab.TransparentProxy.systemextension
```

`codesign -d --entitlements -` without `--xml` prints an abstract dictionary.
`--xml` writes the property list. `:-` is the older spelling and warns that
the colon is deprecated.

Use the bundle id you actually signed when it is not the default. A signed
build's app entitlements include `com.apple.developer.system-extension.install`
and `app-proxy-provider`. The extension's include `app-proxy-provider`, App
Sandbox, `com.apple.security.network.client`, and the app group
`group.<app id>`. Its Mach service is that group followed by `.TransparentProxy`.
An ad-hoc build has none of those entitlements, which is what the panel is reporting.

### What this packaging does not do

No notarization, no Developer ID distribution, no App Store submission, no
updater, and no installer that changes the Mac for you. The extension is not
enabled by the script. Opening a signed app in `/Applications` starts
transparent capture itself, after its bundled proxy engine is ready. An
ad-hoc build cannot install the extension.

## Transparent capture (Network Extension)

**This Mac** in the traffic toolbar — the same panel is also at the foot of
the sidebar — captures every app's HTTP and HTTPS with no proxy setting,
through a Network Extension
(`NETransparentProxyProvider`, run as a system extension). It hands each flow
to the core's bridge on `127.0.0.1:9091`, which the core opens on its own at
startup; the explicit proxy on `9090` is unchanged and works with or without it.

It needs the **signed development build** above, copied to `/Applications`.
An ad-hoc build runs as before and the panel says transparent capture is
unavailable.

### Turning it on

1. Open `/Applications/ProxyLab.app`. The app starts its bundled proxy engine,
   then transparent capture.
2. The first time, macOS asks you to allow the extension in **System Settings
   › General › Login Items & Extensions › Network Extensions**, and then to
   allow ProxyLab to add a proxy configuration. The toolbar says **Needs
   Approval** until the extension is allowed, and capture continues once it
   is. That request is not repeated while macOS is still waiting.
3. Browse. HTTPS is readable once the ProxyLab CA is trusted, exactly as for the
   explicit proxy; nothing installs it for you.

**Stop**, or quitting ProxyLab, turns transparent capture off and stops the
proxy engine the app launched. The extension stays installed. While capture
is on, apps that pin their certificates and some macOS services fail, because
every HTTPS connection is intercepted.

What is captured: TCP to ports 80 and 443, IPv4 and IPv6, except loopback.
Not captured: UDP (so QUIC and DNS) and any other TCP port. Those flows are
not delivered to the extension, so Diagnostics says they are not counted
rather than showing a zero. HTTP/2 over TLS on port 443 is captured. A TLS
failure is a failed exchange, not a missing one. Chrome and iOS URLSession
use QUIC for many sites; turn QUIC off in the client to see those requests.
See **Transparent capture on macOS** in `network-extension.md`.

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
└── Scripts/                  make-app-bundle.sh, verify-app-bundle.sh, packaging-checks-test.sh
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

The window does not implement proxy behavior. That stays in the Rust core.
See `ARCHITECTURE.md`.
