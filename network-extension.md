# ProxyLab macOS Network Extension Context

Load this file for transparent capture, NETransparentProxyProvider, the 9091 bridge, flow policy, lifecycle, signing, or extension/app boundaries.

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
carry as if nothing were installed. The extension does not change routes or
DNS. Capture separately points the active service's HTTP and HTTPS proxy at
the explicit listener, so apps that honour a proxy — including the iOS
Simulator — use `127.0.0.1:9090`. The extension stays for apps that ignore
that setting. It is not a VPN.

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
and DNS — never reaches it and is carried by macOS directly. UDP is not
added to the rules in order to count it: on current macOS, a UDP flow that
the provider returns is terminated, which would break QUIC rather than
observe it. A UDP flow that arrives anyway is counted and not claimed.

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
meanwhile. This Mac's row follows the capture lifecycle — Listening, Stopped,
Needs Approval — and is not a second switch. Its detail says when traffic is
passing straight through, rather than showing a ready capture over a Mac that
is not recording it.

### Lifecycle, and who decides

```
Launch ─▶ start Contents/MacOS/proxy-engine (the process this app owns)
        ─▶ IPC diagnostics: 127.0.0.1:9090 and 127.0.0.1:9091 are listening
        ─▶ save the active service's HTTP and HTTPS proxy, then set both
           to 127.0.0.1:9090
        ─▶ Turn On (below)
        ─▶ Listening, once the tunnel is connected

Turn On ─▶ an enabled extension at this build's version is already installed:
           skip ahead to the configuration
         ─▶ otherwise OSSystemExtensionRequest.activationRequest
             │  macOS may ask for approval in System Settings ›
             │  General › Login Items & Extensions › Network Extensions
             │  a different version is replaced by that same request
             ▼
           NETransparentProxyManager.saveToPreferences
             │  macOS may ask to allow ProxyLab to add proxy configurations
             ▼
           connection.startVPNTunnel ─▶ the extension's startProxy

Stop    ─▶ restore each service that still has the proxy ProxyLab wrote
        ─▶ if that restore does not land, leave the owned core running
        ─▶ connection.stopVPNTunnel, and the configuration disabled
        ─▶ SIGINT to the owned proxy-engine (its existing shutdown)
        ─▶ SIGTERM, then SIGKILL, only if that same process is still alive

Quit    ─▶ the same Stop, then the app exits
```

Each of those system calls has a bound. A call that does not return becomes
an error, or Stop continues with the snapshot it already has. Waiting for a
person is **Needs Approval** or **Needs CA Trust**, not Starting. Command-Q
does not wait on a status refresh after the proxy is restored and the tunnel
is off. A restore that does not verify leaves the app open and the core
running.

Launch starts capture. The system proxy is not written, and the tunnel is
not switched on, until the bundled core reports both listeners ready. The
proxy is restored before that core is stopped. The configuration is never
on demand. The extension stays installed after Stop and after quit;
installed and capturing are different states, and quit does not uninstall
it.

The first launch may stop at **Needs Approval** until the user allows the
extension. That activation is not submitted again while it is outstanding.
Once macOS reports the extension allowed, the Turn On that was waiting
finishes — in that session only, and not if Stop or quit cancelled it.

An explicit Stop is remembered until the user presses Start or the app
quits, so a reconnect does not turn capture back on.

If `9090` or `9091` is already in use, startup fails and the other process
is not signaled. The app signals only the proxy-engine it launched. If that
process exits on its own, the app restores the proxy it wrote, turns the
tunnel off, and stops claiming that capture is listening. The extension's
own bridge-failure fallback still carries flows directly during that
hand-off.

**Capture writes this Mac's HTTP and HTTPS proxy, and only those.** After
both listeners are ready it saves the active network service's
`HTTPEnable`, `HTTPProxy`, `HTTPPort`, `HTTPSEnable`, `HTTPSProxy`, and
`HTTPSPort`, then sets both protocols to `127.0.0.1:9090`. The service is
the one `State:/Network/Global/IPv4` (then IPv6) names as `PrimaryService`,
not a service called Wi-Fi. A later change of the active service saves that
service's previous settings on their own and applies the same endpoint.
Stop and quit write each saved service back, and only when it still has
the settings ProxyLab wrote. A service someone else changed is left as it
is and reported. The exception list, FTP, SOCKS, and PAC are not read for
writing and are not written. The commit goes through System Configuration
with the administrator right
`system.services.systemconfiguration.network`; macOS may ask for that
password, and the grant is not shared and expires. The previous settings
are stored at `~/.proxylab/run/system-proxy-ownership.json` before the
commit, so a crash of the app or the core can put them back. The next
launch restores that file before it starts the core, which is what asks
for the administrator password. A force-kill or a restart cannot prompt
while ProxyLab is not running, so the proxy stays at `127.0.0.1:9090`
until the app is opened again. The leftover is not saved as the previous
proxy.

The two paths do not capture one request twice. A flow to `127.0.0.1:9090`
is loopback, which the extension's rules and `FlowPolicy` leave alone, so
the simulator's connection to the explicit proxy is not also bridged. The
core's own upstream to port 80 or 443 is seen once: the bridge answers
`508` before it connects or records when the source is the core, the
extension then carries that flow itself, and later flows from that process
are not claimed. A phone connecting to the LAN address on port `9090` is
not a port the extension claims.

The Network Extension configuration is a separate object, listed under
System Settings › Network › VPN & Filters, and ProxyLab reads and writes
only the one whose provider is its own — another app's transparent proxy
on the same Mac is never read, changed, or stopped.

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
both. The extension is also sandboxed, with `network.client` and the app
group `group.<app bundle id>`. Its Mach service name is that group followed
by `.TransparentProxy`. The app itself is not sandboxed.

`Scripts/make-app-bundle.sh` embeds the provider at
`Contents/Library/SystemExtensions/<bundle id>.TransparentProxy.systemextension`.
That suffix is `SystemTransparentProxyControl.extensionSuffix`. With
`PROXYLAB_TEAM_ID`, `PROXYLAB_SIGNING_IDENTITY`, `PROXYLAB_APP_PROFILE`, and
`PROXYLAB_EXTENSION_PROFILE` all set, the script checks the profiles and the
identity, signs the extension, then signs the app. The bundle replaces
anything already at the destination only after `Scripts/verify-app-bundle.sh`
has checked those entitlements. With none of the four set, both are ad-hoc
signed with no entitlements: the app runs as before and the panel says
transparent capture needs a signed build. A partial set of variables is an
error. Only an app in `/Applications` may install a system extension.

The signature is the identity whose certificate organizational unit is the
team, whether that identity is Mac Developer, Apple Development, or Developer
ID Application. Notarization and an App Store or updater pipeline are not.
The script does not enable the
extension or change the system proxy. The variables and the checks are in
`macos/ProxyLab/README.md`.

### What a claimed flow becomes

A TCP flow that is claimed is a `CONNECT` on the bridge. TLS (the first byte
is `0x16`) is intercepted. HTTP/2 is the ALPN result `h2` on that TLS
session, over TCP port 443, and each of its streams is an exchange. Plain
HTTP is HTTP/1.1. A handshake that fails — the CA is not trusted, the
application pins its certificate, or the origin's certificate does not
verify — is stored as a failed exchange. It does not disappear. A connection
that closes before sending a byte is discarded, because there was no request.
HTTP/2 cleartext (`h2c`) on port 80 is not a TLS flow and is not carried as
HTTP/2.

### What is not here

UDP of any kind, so no QUIC and no DNS; ports other than 80 and 443;
packet-level anything; per-application rules; an iOS Network Extension. Those
bypasses are not counted, because the flows are never delivered — the
diagnostics sheet says so, rather than showing a zero that would look like
"no QUIC happened". A UDP flow that is delivered anyway is counted under
Bypassed UDP/QUIC and is not claimed. A flow's source application is not
recorded in the captured exchange — the exchange model is unchanged, and a
bridged exchange's client address is the extension's loopback socket. Apps
that pin certificates, and some macOS services, fail while capture is on,
because every claimed HTTPS connection is intercepted; the panel says so
while it is on, and the failure is a failed exchange. Chrome, and
URLSession, prefer QUIC for many sites. QUIC is UDP, so those requests are
not captured unless the client falls back to TCP.
