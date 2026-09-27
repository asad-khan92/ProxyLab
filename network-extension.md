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
