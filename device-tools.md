# ProxyLab Device Tools Context

Load this file for iOS Simulator, physical iPhone/LAN exposure, Android Emulator, device CA/proxy configuration, or device-session UI work.

## The iOS Simulator

```
● iPhone 17 Pro                 the sidebar row
  Connected · CA trusted

  Routing    Explicit Proxy     opened from the row
  Captured through This Mac
  [ Disconnect ]
```

The row does not name `127.0.0.1:9090`. Explicit Proxy, Transparent Capture,
and Not Routed are the three paths, and Explicit Proxy is still this Mac's
HTTP and HTTPS proxy when it points at the explicit listener. Connect is unchanged.

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
that routes a simulator is this Mac's own proxy configuration. Capture's Start
sets that, after `127.0.0.1:9090` is listening, and Stop puts back the HTTP
and HTTPS settings it saved. `ProxyRoute` only reads, through
`SCDynamicStoreCopyProxies`. The write is the capture lifecycle's, not this
panel's, and not `simctl`'s.

### What Connect does

Three things, in order, and it says which of them it got through:

| | |
| --- | --- |
| the core is running | the window's own IPC connection, not a second probe |
| trust the CA | `simctl keychain <udid> add-root-cert`, Apple's documented command |
| how traffic is captured | this Mac's HTTP and HTTPS proxy, when it is `127.0.0.1:9090`; otherwise transparent capture, when that is on |

The third is a check, not a change. Trusting the CA is not a route. When this
Mac's HTTP and HTTPS proxy points at `127.0.0.1:9090`, that is the simulator's
route — including while transparent capture is also on, because the simulator
connects to loopback and the Network Extension does not see that connection.
`127.0.0.1:9091` is the bridge the extension talks to and is not shown here.
Transparent capture is the route only when the system proxy does not point at
the explicit listener. When the certificate is trusted and neither route is in
place, the interface says exactly what is missing. Connecting a simulator does
not write the proxy; Start and Stop do.

Connecting twice is two of the same command: `add-root-cert` exits zero for a
certificate the simulator already trusts, which is the whole of the
idempotency. There is no "is it installed?" query, because `simctl` has none
and inventing one would mean reading the simulator's trust store behind
Apple's back.

### What Disconnect does

It ends the session the app is keeping, and says what it did not undo. Connecting
did not change a proxy setting, so disconnecting does not put one back. Capture's
Stop restores the Mac proxy it saved. The CA stays trusted, because
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

No physical iPhone and no Android. This panel does not install or switch the
Network Extension — This Mac does that — and it does not configure this Mac's
proxy. Capture does, on Start and Stop. Nothing boots, erases, resets or shuts
down a simulator. Nothing in this panel is installed into the macOS
keychain. Capture trusts the same certificate file for this user, and a
simulator's trust store is a separate record: connecting a simulator does
not satisfy that check. The proxy
engine is untouched by the simulator panel: it runs `simctl` and reads one
system dictionary. Whether the system proxy points at the explicit listener,
and whether transparent capture is on, are read from the route and from the
capture lifecycle the window already has.

## The physical iPhone

```
● Physical iPhone
  Ready on 192.168.1.42:9090     the LAN address, not the simulator route
```

The row follows the network observer. While there is no usable address it
says it is waiting for a local network. Enabling access is `enable_lan`; the
certificate steps open from the detail.

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
true five minutes ago is a list of addresses that no longer bind. The app
watches the network path and asks again when it changes. While local network
access is on, an address that has gone is unbound and the new one is bound;
when none is usable the LAN listener is closed and the panel waits, and
`127.0.0.1:9090` keeps serving this Mac. Local network access that is off is
not turned on by that change. What is left out is loopback, an interface that
is not up and running, a point-to-point interface (a VPN's `utun`), and a
link-local `169.254` address — none of them is a Wi-Fi a phone is on.

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
management, and no app installed on it. Turning this on adds a listening
socket. It does not by itself change this Mac's system proxy — that is
capture's Start and Stop, and it points at `127.0.0.1:9090`, not at the LAN
address. The LAN address is what a phone types in. There is no authentication
on the LAN listener either: the boundary ProxyLab has is the IPC socket's file
permissions, and that boundary does not stretch over a network. So the
interface says, in a sentence and not a coloured dot, that any device on this
Wi-Fi can use the proxy while it is on.

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

The sidebar row names the emulator and says Connected or Needs CA setup. It
does not show `10.0.2.2`; that alias stays the address Connect writes.

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
