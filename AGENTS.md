# Sculpto implementation

This independent repository contains the GPL-3.0-only implementation. Use
Rust 2024 and the declared toolchain; keep core behavior in Rust and optional
host adapters thin. Private asset payloads and renders never belong in VCS.

Use jj for mutations and readable git diff. Confirm the task feature before
editing/building; after jj describe immediately jj new. This implementation
is published publicly; its managed wrapper stays private. Land on main and
push after the required checks. Use at most two Cargo jobs;
on the shared host, heavy work uses the operator's owned-unit/QUEUE procedure.

`just gate` checks formatting, Clippy with -D warnings and workspace tests.
Parsing changes also run `just test-real /path/to/BodySlide` with the explicit
host fixture. Build with cargo build, not only cargo check.

In the managed `sculpto-rs-wrapper/repos/sculpto-rs` checkout, the wrapper's
AGENTS.md, issue journal, rules and source map govern coordinated work. Keep
their affected bindings/metrics current and run the wrapper's full `just gate`
before integration. Independent source writers use a sibling
`repos/sandbox_sculpto-rs/` workspace container; wrapper writers use the
wrapper's separate sandbox. Standalone build/test does not require the wrapper.
