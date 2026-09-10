# ProxyLab

ProxyLab is a macOS HTTP/HTTPS debugging proxy for developers. It is intended
to sit between your clients — a Mac browser, an iOS simulator, an Android
emulator, or a physical iPhone — and the network, capturing traffic so you can
inspect requests and responses, rewrite or block them with rules, and replay
them while debugging. The proxy core is written in Rust and runs independently
of the macOS user interface, which is built in SwiftUI/AppKit and talks to the
core over a defined IPC boundary.

## Current status

**Project scaffolding.**

No networking, proxy, TLS, HTTP, UI, device-management, or business logic has
been implemented. The repository currently contains the directory structure and
the documentation that defines how the project will be built.

## High-level architecture

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

- **`core/`** — Rust. Proxy networking, HTTP/HTTPS handling, TLS interception,
  rules, capture, storage, and IPC.
- **`macos/ProxyLab/`** — SwiftUI/AppKit. Presentation, user interaction, and
  configuration.
- **`device-tools/`** — Tooling for routing iOS simulator and Android emulator
  traffic through the proxy. Physical iPhone setup comes later.
- **`tests/integration/`** — Cross-module and end-to-end tests.

The proxy engine is intentionally separated from the macOS UI: the core must
build, run, and be tested with no UI present, and the Rust core never depends
on Swift or macOS UI code. See `ARCHITECTURE.md`.

## Development principles

- Read `AGENTS.md` and `ARCHITECTURE.md` before changing anything.
- Work only on the requested task; do not build ahead of the roadmap.
- Keep the proxy core independent of the UI, with one-way dependency flow.
- All UI ↔ core communication crosses the IPC boundary.
- Small, isolated modules with single responsibilities.
- No placeholder implementations for production functionality.
- Every functional change ships with tests, and tests/builds are run before a
  task is called done.
- Dependencies must be justified; public interfaces and data models never
  change silently.

The full set of rules is in `AGENTS.md`. Planned work is in `ROADMAP.md`.
