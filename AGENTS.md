# WOOSH AGENT RUNBOOK

**Generated:** 2026-02-27
**Updated:** 2026-11-02
**Branch:** main

## OVERVIEW

Terminal white noise generator: Rust daemon (audio + IPC) + ratatui TUI client communicating over Unix socket.

## STRUCTURE

```
woosh/
├── src/
│   ├── main.rs         # CLI dispatcher (clap), daemon auto-spawn logic
│   ├── lib.rs          # Library half: used by main.rs and the integration tests
│   ├── config.rs       # config.toml load/create with defaults
│   ├── daemon/         # Audio daemon: IPC server, audio thread, EQ, lifecycle
│   │   └── AGENTS.md   # Daemon internals
│   └── tui/            # ratatui TUI: event loop, Presets + Equalizer screens
│       └── AGENTS.md   # TUI internals
├── tests/
│   ├── ipc_integration.rs          # 18 async tokio integration tests
│   └── daemon_startup_race_test.rs # 2 daemon readiness tests
├── docs/logging.md     # Logging conventions (tracing, RUST_LOG)
├── Cargo.toml          # lib + [[bin]], pedantic clippy
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
| Place sounds / mpv flags | `src/daemon/mpv.rs` MpvSource |
| TUI screen / key | `src/tui/mod.rs` handle_key() + render() |
| TUI state | `src/tui/app.rs` App struct / Screen enum |
| IPC client (TUI side) | `src/tui/client.rs` DaemonClient |
| Daemon startup / shutdown / paths | `src/daemon/mod.rs`, `src/daemon/lifecycle.rs` |
| Config defaults | `src/config.rs` |
| CLI subcommands | `src/main.rs` |

## OS RUNTIME PATHS (critical — NOT hardcoded)

Paths come from `dirs::data_dir()` in `src/daemon/lifecycle.rs`:

- macOS: `~/Library/Application Support/woosh/`
- Linux: `${XDG_DATA_HOME:-~/.local/share}/woosh/`

Files: `woosh.sock`, `woosh.pid`, `woosh.log`, `woosh.ready`

Config comes from `dirs::config_dir()`: `~/Library/Application Support/woosh/config.toml` on macOS, `${XDG_CONFIG_HOME:-~/.config}/woosh/config.toml` on Linux.

**Wrong socket path → nc shows nothing. Always verify with `cargo run -- status` first.**

## IPC PROTOCOL

Newline-delimited text over Unix socket. Commands are case-insensitive. Errors reply `ERROR <reason>\n`.

| Command | Args | Response |
|---------|------|----------|
| `PLAY` | `white\|pink\|brown` | `OK\n` |
| `STOP` | — | `OK\n` |
| `SET_VOLUME` | `0.0–1.0` (clamped; NaN rejected) | `OK\n` |
| `SET_EQ` | `band(0-9) gain_db` (clamped ±12) | `OK\n` |
| `GET_EQ` | — | `EQ g0 g1 ... g9\n` |
| `PLAY_PLACE` | location string | `OK\n`, or `ERROR mpv not found in PATH…` / `yt-dlp` |
| `STOP_PLACE` | — | `OK\n` |
| `SET_PLACE_VOLUME` | `0.0–1.0` | `OK\n` |
| `SET_PLACE_EQ` | `band gain_db` | `OK\n` |
| `GET_PLACE_EQ` | — | `PLACE_EQ g0 ... g9\n` |
| `GET_PLACE_STATUS` | — | `PLACE_STATUS place=loc:state:vol\n` |
| `STATUS` | — | `STATUS synth=p:s:v place=loc:s:v device=ok\|unavailable\n` (preset `p` is `none` before first play; state is `running`/`stopped`) |
| `QUIT` | — | no reply; daemon removes its files, fades out, exits |

## BINARY / LIB HYBRID

Single Cargo package with `[lib]` + `[[bin]]`. Integration tests `use woosh::daemon` directly — this is intentional and required for test coverage.

## CONVENTIONS

- Clippy: `all = "warn"` + `pedantic = "warn"` → `-D warnings` in CI. Every `#[allow]` carries a trailing comment saying why.
- `rustfmt.toml`: stable-only. Do NOT add nightly keys (`wrap_comments`, `imports_granularity`, `group_imports`, `format_code_in_doc_comments`). Run `cargo fmt --all` before committing; CI checks it.
- Cargo.toml uses caret ranges; `Cargo.lock` pins exact versions. Do NOT `cargo update` or raise a range without checking compat.
- EQ gains are NOT persisted to config.toml (reset to flat on daemon restart).
- `OutputStream` is opened lazily when a channel starts and released when no channel is playing or waiting — allows BT headphone handoff. If it cannot be opened, the daemon retries every 5 s and reports `device=unavailable`.
- `idle_timeout_mins` (default 15) in `[daemon]` config: daemon shuts down (same cleanup as `QUIT`) after N minutes with no clients and both channels stopped. `0` disables it.
- Tests never touch the real data directory: `run_ipc_server` takes the socket and ready paths.

## COMMANDS

```bash
cargo check --all-targets          # Type-check (fast)
cargo test --all-targets           # All 35 tests (15 unit + 20 integration)
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
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
- Never call `process::exit` in the daemon — return from `run_ipc_server` so mod.rs cleans up.
- When adding IPC commands: update `tests/ipc_integration.rs` in the same commit.

## FAST TRIAGE

| Symptom | Cause | Fix |
|---------|-------|-----|
| nc shows nothing | Wrong socket path or daemon dead | `cargo run -- status` first |
| `daemon is already running` | Stale PID | `cargo run -- stop`, retry |
| `path must be shorter than SUN_LEN` in woosh.log | `$HOME` too long for a Unix socket path (104 bytes on macOS) | Use a shorter `HOME` (e.g. a symlink) when testing with a fake home |
| Place sound silent, `place … stopped` soon after `PLAY_PLACE` | mpv rejected its args or yt-dlp failed | Run the mpv command from `mpv.rs` by hand |
| Clippy pedantic error | Unnecessarily nested or-patterns | Use `Char('q' \| 'Q')` not two arms |
| IPC test hangs before first request | Server not yet bound | Wait for the ready file (see `spawn_server` in tests) |

# Workflow

  * Whenever a unit of work is completed, always update @ROADMAP.md, and if necessary, @SPEC.md, the root folder's AGENTS.md, or the AGENTS.md file in the folder where code changes were made.
