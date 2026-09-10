# ARCHITECTURE.md

Intended high-level architecture for **ProxyLab**, a macOS HTTP/HTTPS
debugging proxy for developers.

> **Status:** This document describes the *intended* design. Almost none of it
> is implemented yet — see `ROADMAP.md`. It exists so that implementation work,
> when it happens, has a fixed target.

## Components

### macOS UI

- **Technology:** SwiftUI / AppKit
- **Responsibility:** Presentation, user interaction, and configuration.
- Renders captured traffic, inspectors, search and filtering, and rule
  configuration surfaces.
- Holds no proxy logic. It does not parse protocols, terminate TLS, evaluate
  rules, or own the capture store.
- Location: `macos/ProxyLab/`

### Core

- **Technology:** Rust
- **Responsibility:** Proxy networking, HTTP/HTTPS handling, TLS interception,
  rules, capture, storage, and IPC.
- Runs and is testable headlessly, with no UI process present.
- Modules:
  | Path | Responsibility |
  | --- | --- |
  | `core/proxy-engine/` | TCP listeners, connection lifecycle, CONNECT tunneling, upstream connections |
  | `core/tls/` | Root CA, dynamic leaf certificate generation, TLS interception |
  | `core/http/` | HTTP/1.1 and HTTP/2 parsing, framing, and message models |
  | `core/rules/` | Rule matching and mutation (map local, map remote, block) |
  | `core/storage/` | Captured request/response persistence, bodies, timing |
  | `core/curl/` | cURL export and import |
  | `core/ipc/` | IPC boundary: message schema, transport, event stream |

### Device tools

- **Responsibility:** Configuring clients to route through the proxy and trust
  the root CA.
- `device-tools/ios/` — macOS / iOS simulator tooling.
- `device-tools/android/` — Android emulator tooling.
- Physical iPhone setup will be added later.

## Intended data flow

```
Mac Browser / iOS Simulator / Android Emulator / Physical iPhone
        ↓
Proxy Engine
        ↓
HTTP / HTTPS / HTTP2
        ↓
Rule Engine
        ↓
Capture / Storage / Events
        ↓
IPC
        ↓
macOS UI
```

Traffic enters at the proxy engine, is decoded into protocol-level messages,
passes through rule evaluation (which may rewrite, redirect, or block it), is
captured into storage as request/response records with timing, and is surfaced
to the UI as an event stream across the IPC boundary.

## Separation of proxy engine and macOS UI

**The proxy engine is intentionally separated from the macOS UI.** This is a
deliberate structural decision, not an implementation detail.

The reasons:

- **The core must be usable without a UI.** Headless operation, CI runs, and
  integration tests must not require launching an AppKit application.
- **Failure isolation.** A UI crash, hang, or restart must not drop live
  connections or lose captured traffic.
- **Testability.** Protocol correctness, TLS behaviour, and rule evaluation are
  testable in Rust without any UI harness.
- **Portability.** Nothing in the core assumes macOS presentation, so other
  front-ends remain possible without rewriting proxy logic.
- **Performance.** Connection handling and body capture run independently of
  the UI's render loop and are never blocked by it.

### The boundary

- Dependency direction is one-way: **the macOS UI depends on the core; the core
  never depends on Swift or macOS UI code.**
- All communication between UI and core crosses the IPC boundary defined in
  `core/ipc/`. There are no side channels.
- The IPC surface is a public interface. Changing its messages or data models
  is a breaking change and must be declared explicitly (see `AGENTS.md` rule
  15).

## Testing

- Unit tests live beside the module they cover, inside `core/`.
- Cross-module and end-to-end tests live in `tests/integration/`.
