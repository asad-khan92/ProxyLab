# AGENTS.md

Operating rules for any human or AI agent working on **ProxyLab**.

These rules are binding. They exist to keep the project small, reviewable, and
architecturally stable while it grows.

## Development Rules

1. **Read `AGENTS.md` and `ARCHITECTURE.md` before modifying the project.**
   Do not begin work without understanding the current intended architecture.

2. **Work only on the explicitly requested task.**
   Scope is defined by the request, not by what seems useful nearby.

3. **Never implement future roadmap features unless explicitly requested.**
   `ROADMAP.md` describes intent, not permission. A phase listed there is not
   an invitation to build it.

4. **Never redesign the architecture without explicit approval.**
   Architectural change requires a discussion and an update to
   `ARCHITECTURE.md` first.

5. **Do not modify unrelated files.**
   No opportunistic refactors, reformatting, or cleanup outside the task.

6. **Every functional change must include appropriate tests.**
   Unit tests live beside the module; cross-module tests live in
   `tests/integration/`.

7. **Do not add dependencies without explaining why they are needed.**
   State the purpose, the alternative considered, and why the alternative was
   rejected. Prefer the standard library.

8. **Prefer small, isolated modules with clear responsibilities.**
   One module, one job. If a module needs a paragraph to describe, split it.

9. **Keep the networking/proxy core independent from the macOS UI.**
   The core must be buildable, runnable, and testable with no UI present.

10. **The Rust core must not depend on Swift/macOS UI code.**
    Dependency flows in one direction only: UI depends on core, never the
    reverse.

11. **The macOS UI communicates with the core through a defined IPC boundary.**
    No side channels, no shared mutable files, no reaching around IPC.

12. **Do not use placeholder implementations for production functionality.**
    No stubs, fake returns, `TODO`-shaped behaviour, or simulated results in
    code paths meant to be real. If something cannot be implemented, say so.

13. **Before declaring a task complete, run all relevant tests/builds.**
    A task is not done because the code was written. It is done when it builds
    and its tests pass.

14. **Report exactly what files were changed and what was tested.**
    List the files. List the commands run. Report failures honestly.

15. **Never silently change public interfaces or data models.**
    Changes to IPC messages, wire formats, persisted schemas, or public APIs
    must be called out explicitly in the change report.
