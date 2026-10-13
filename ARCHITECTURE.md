# ARCHITECTURE.md

Intended high-level architecture for **ProxyLab**, a macOS HTTP/HTTPS debugging proxy for developers.

> **Context loading rule:** This file is the compact architecture index. Do **not** load every context file for every task. A Cursor prompt should name only the context files relevant to that step. Follow `AGENTS.md` and `ROADMAP.md` as usual.

## System shape

ProxyLab has three major layers:

1. **Rust core** — networking, TLS interception, HTTP/1.1 + HTTP/2, rules, capture/storage, replay, device LAN exposure, and IPC.
2. **macOS client** — SwiftUI/AppKit presentation and configuration. It does not implement proxy logic.
3. **Device / system integrations** — iOS Simulator tooling, Android Emulator tooling, LAN physical-iPhone support, and the macOS Network Extension.

Traffic converges on the same core pipeline:

```text
Mac Browser / iOS Simulator / Android Emulator / Physical iPhone / any Mac app
        ↓
Proxy Engine
        ↓
HTTP / HTTPS / HTTP/2
        ↓
Rule Engine
        ↓
Capture / Storage / Events
        ↓
IPC
        ↓
macOS UI
```

The Network Extension is an alternate ingress path into the same engine; it does not create a second HTTP/TLS/rules/capture stack.

The macOS app owns the lifecycle of the proxy engine it ships. `ProxyLab.app` launches `Contents/MacOS/proxy-engine`, waits until IPC `diagnostics` reports the explicit listener and the transparent bridge listening, and only then — and only after certificate trust reports ready — points the active network service's HTTP and HTTPS proxy at `127.0.0.1:9090` and starts transparent capture. That check reads the CA the core stored and trusts that exact certificate in the current user's trust settings. The system proxy stays unchanged until Security.framework evaluates that certificate as a trusted anchor. Stop does not remove the trust record. Stop and quit restore that proxy and read the live route back before stopping the process the app launched. A restore that cannot be verified leaves the process running, and quit is refused. The core stays a separate executable with no dependency on Swift; headless runs and tests still start it themselves. A process the app did not launch — including one already bound to `9090` or `9091` — is left alone. SIGKILL, a crash, and power loss cannot run this restore; the ownership file is what the next launch uses. A helper that outlives the app would be required to restore during that gap, and none is installed.

## Repository map

| Area | Location | Context |
|---|---|---|
| macOS app | `macos/ProxyLab/` | `macos-ui.md` |
| Rust core | `core/proxy-engine/`, `core/tls/`, `core/http/`, `core/storage/` | `core.md` |
| Rules | `core/rules/` | `rules-engine.md` |
| IPC | `core/ipc/` | `ipc.md` |
| Diagnostics | `core/diagnostics/` | `ipc.md` |
| cURL + replay | `core/curl/`, replay in `core/proxy-engine/` | `replay.md` |
| HAR | `core/har/` | `har.md` |
| Saved requests | storage + IPC + macOS UI | `saved-requests.md` |
| WebSocket/SSE | `core/proxy-engine/` + stream storage/IPC/UI | `streaming.md` |
| Breakpoints/throttling | `core/proxy-engine/src/controls/` | `breakpoints.md` |
| iOS Simulator | `device-tools/ios/SimulatorTools/` | `device-tools.md` |
| Android Emulator | `device-tools/android/AndroidTools/` | `device-tools.md` |
| Physical iPhone | LAN listener + macOS device UI | `device-tools.md` |
| Network Extension | `macos/ProxyLabTransparentProxy/`, `core/proxy-engine/src/transparent/`, `macos/ProxyLab/Sources/TransparentProxyKit/` | `network-extension.md` |
| Testing | Rust, Swift, device-tool, and integration suites | `testing.md` |

## Context files

- `macos-ui.md` — macOS UI, traffic inspector/navigation, pinned domains.
- `core.md` — core architecture, data flow, HTTP/1.1/HTTP/2, upstream pooling, core/UI separation.
- `rules-engine.md` — matching, precedence, actions, transformations, rule application and persistence.
- `replay.md` — cURL parsing/serialization and replay.
- `har.md` — HAR 1.2 export/import.
- `saved-requests.md` — saved request model, persistence, editing and replay integration.
- `streaming.md` — WebSocket/SSE and stream-message inspection/editing.
- `breakpoints.md` — breakpoints, body staging and throttling.
- `ipc.md` — Unix socket, JSON-lines protocol, operations, events and wire boundary.
- `device-tools.md` — iOS Simulator, physical iPhone/LAN, Android Emulator.
- `network-extension.md` — transparent macOS capture and system-extension architecture.
- `testing.md` — testing strategy and existing coverage.

## Cross-cutting rules

- Rust core is headless and independent of Swift/macOS UI.
- macOS UI communicates with the core through the IPC boundary; it does not access core storage directly.
- Prefer additive IPC changes; public wire/data-model changes must be explicitly reported.
- Do not create a second implementation of an existing protocol pipeline when a feature can reuse the core path.
- Do not silently modify macOS system proxy settings, shell configuration, user firewall/keychain, or developer device state outside the feature's explicit reversible operation. Capture's Start/Stop is that operation for this Mac's HTTP and HTTPS proxy: it saves the previous settings, points the active service at `127.0.0.1:9090`, and restores only services that still have the settings it wrote. Trusting the current CA is capture's other explicit keychain operation: the current user's trust settings for the exact certificate the core is using. It does not write the System keychain, and Stop does not remove the record.
- Keep device-platform tooling independent.
- Rules decide routing/transformation; breakpoints are interactive holds; throttling is live transport interference.
- Capture represents what actually passed through the proxy; do not add original-vs-modified history unless explicitly required.
- Use existing storage/body limits and IPC size limits rather than inventing parallel mechanisms.
- Security-sensitive values such as credentials, headers, bodies, and cURL contents must not be logged.

## Feature context selection for Cursor prompts

Use only the smallest relevant set. Typical examples:

- **Rule change:** `rules-engine.md` + `core.md` only if engine integration changes.
- **WebSocket/SSE:** `streaming.md` + `core.md`; add `breakpoints.md` only if pauses/throttling are touched.
- **Search/traffic inspection:** `macos-ui.md` + `ipc.md`; add `core.md` if storage/query implementation changes.
- **iOS Simulator:** `device-tools.md`; add `macos-ui.md` only for the macOS panel.
- **Android Emulator:** `device-tools.md`; add `macos-ui.md` only for the macOS panel.
- **Physical iPhone/LAN:** `device-tools.md` + `core.md`; add `ipc.md` if wire operations change.
- **Network Extension:** `network-extension.md` + `core.md`; add `macos-ui.md` only for the app panel.
- **Replay/cURL:** `replay.md` + `core.md` only when replay pipeline internals change.
- **HAR:** `har.md` + `ipc.md` if wire operations change.
- **Saved requests:** `saved-requests.md` + `replay.md`; add `ipc.md` for protocol changes.
- **Breakpoints/throttling:** `breakpoints.md` + `core.md`.
- **Tests only:** `testing.md` plus the feature context being tested.

## Important fixed constraints

- Explicit proxy listener: `127.0.0.1:9090`.
- Transparent bridge: `127.0.0.1:9091`.
- Default IPC socket: `~/.proxylab/run/proxylab.sock`, overridable by `PROXYLAB_IPC_SOCKET`.
- LAN exposure is disabled by default and uses one explicitly selected RFC1918 address.
- Do not automatically quit/modify another proxy such as Proxyman during manual verification.
- Do not modify `~/.zshrc`.
- Prefer scratch HOME/database/socket/CA locations for manual verification when practical.
