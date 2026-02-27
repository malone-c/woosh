# WOOSH AGENT RUNBOOK

**Generated:** 2026-02-27
**Branch:** main

## OVERVIEW

Terminal white noise generator: Rust daemon (audio + IPC) + ratatui TUI client communicating over Unix socket.

## STRUCTURE

```
woosh/
├── src/
│   ├── main.rs         # CLI dispatcher (clap), daemon auto-spawn logic
│   ├── lib.rs          # Re-exports daemon/config/tui for integration tests
│   ├── config.rs       # ~/.config/woosh/config.toml load/create with defaults
│   ├── daemon/         # Audio daemon: IPC server, audio thread, EQ, lifecycle
│   │   └── AGENTS.md   # Daemon internals
│   └── tui/            # ratatui TUI: event loop, spectrum, EQ screen
│       └── AGENTS.md   # TUI internals
├── tests/
│   ├── ipc_integration.rs          # 11 async tokio integration tests
│   └── daemon_startup_race_test.rs # 2 daemon readiness race tests
├── Cargo.toml          # lib + [[bin]], pinned deps, pedantic clippy
├── rustfmt.toml        # stable-only options (no nightly-only keys)
├── SPEC.md             # Architecture spec
└── ROADMAP.md          # Phase-based roadmap (update after changes)
```

## WHERE TO LOOK

| Task | Location |
|------|----------|
| Add IPC command | `src/daemon/ipc.rs` dispatch() + update tests |
| Change noise algorithm | `src/daemon/audio.rs` NoiseSource |
| EQ math / filter | `src/daemon/eq.rs` peaking_coeffs(), EqProcessor |
| TUI screen / key | `src/tui/mod.rs` handle_key() + render() |
| TUI state | `src/tui/app.rs` App struct / Screen enum |
| IPC client (TUI side) | `src/tui/client.rs` DaemonClient |
| Daemon startup / paths | `src/daemon/lifecycle.rs` |
| Config defaults | `src/config.rs` |
| CLI subcommands | `src/main.rs` |

## OS RUNTIME PATHS (critical — NOT hardcoded)

Paths come from `dirs::data_dir()` in `src/daemon/lifecycle.rs`:

- macOS: `~/Library/Application Support/woosh/`
- Linux: `${XDG_DATA_HOME:-~/.local/share}/woosh/`

Files: `woosh.sock`, `woosh.pid`, `woosh.log`, `woosh.ready`

**Wrong socket path → nc shows nothing. Always verify with `cargo run -- status` first.**

## IPC PROTOCOL

Newline-delimited text over Unix socket. Key commands:

| Command | Args | Response |
|---------|------|----------|
| `PLAY` | `white\|pink\|brown` | `OK\n` |
| `STOP` | — | `OK\n` |
| `SET_VOLUME` | `0.0–1.0` | `OK\n` |
| `SET_EQ` | `band(0-9) gain_db` | `OK\n` |
| `GET_EQ` | — | `EQ g0 g1 ... g9\n` |
| `PLAY_PLACE` | location string | `OK\n` |
| `STOP_PLACE` | — | `OK\n` |
| `SET_PLACE_VOLUME` | `0.0–1.0` | `OK\n` |
| `SET_PLACE_EQ` | `band gain_db` | `OK\n` |
| `GET_PLACE_EQ` | — | `PLACE_EQ g0 ... g9\n` |
| `GET_PLACE_STATUS` | — | `PLACE_STATUS place=loc:state:vol\n` |
| `STATUS` | — | `STATUS synth=p:s:v place=p:s:v\n` |
| `SUBSCRIBE_SAMPLES` | — | `OK\n` then pushed `SAMPLES <hex>\n` every 33ms |
| `QUIT` | — | (daemon exits; no response) |

Sample encoding: f32 → little-endian bytes → 8-char hex (per sample).

## BINARY / LIB HYBRID

Single Cargo package with `[lib]` + `[[bin]]`. Integration tests `use woosh::daemon` directly — this is intentional and required for test coverage.

## CONVENTIONS

- Clippy: `all = "warn"` + `pedantic = "warn"` → `-D warnings` in CI. No suppressions without comment.
- `rustfmt.toml`: stable-only. Do NOT add nightly keys (`wrap_comments`, `imports_granularity`, `group_imports`, `format_code_in_doc_comments`).
- Pinned dep versions in Cargo.toml. Do NOT bump without checking compat.
- EQ gains are NOT persisted to config.toml (reset to flat on daemon restart).

## COMMANDS

```bash
cargo check --all-targets          # Type-check (fast)
cargo test --all-targets           # All 26 tests (15 unit + 11 integration)
cargo run -- daemon --no-daemonize # Run daemon in foreground for dev
cargo run -- status                # Verify daemon reachable
cargo run -- stop                  # Stop daemon

# Manual IPC (compute SOCK per OS first):
if [[ "$(uname)" == "Darwin" ]]; then
  SOCK="$HOME/Library/Application Support/woosh/woosh.sock"
else
  SOCK="${XDG_DATA_HOME:-$HOME/.local/share}/woosh/woosh.sock"
fi
printf 'STATUS\nGET_EQ\n' | nc -U "$SOCK"
```

## ANTI-PATTERNS

- Never assume Linux XDG paths on macOS.
- Never add `--no-verify` or bypass CI hooks.
- Never touch audio thread threading model (rodio `!Send` requires `std::thread`).
- Never reset BiquadState on EQ coefficient change (causes clicks).
- When adding IPC commands: update `tests/ipc_integration.rs` in the same commit.
- After completing work: update `ROADMAP.md` checkboxes and `SPEC.md` protocol docs.

## FAST TRIAGE

| Symptom | Cause | Fix |
|---------|-------|-----|
| nc shows nothing | Wrong socket path or daemon dead | `cargo run -- status` first |
| `daemon is already running` | Stale PID | `cargo run -- stop`, retry |
| Clippy pedantic error | Unnecessarily nested or-patterns | Use `Char('q' \| 'Q')` not two arms |
| IPC test prints nothing | Server not yet bound | Add 20ms sleep after spawn |
