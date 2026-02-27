# WOOSH AGENT RUNBOOK (HIGH-SIGNAL)

This file exists to prevent repeat E2E confusion (especially "nothing printed" IPC tests).

## 0) Ground Truth (from code)

- Runtime paths come from `dirs::data_dir()` in `src/daemon/lifecycle.rs`.
- Socket path is **not hardcoded** to `~/.local/share/woosh` on every OS.
- IPC server command surface is in `src/daemon/ipc.rs` (`PLAY`, `STOP`, `SET_VOLUME`, `PLAY_PLACE`, `STOP_PLACE`, `SET_PLACE_VOLUME`, `GET_PLACE_STATUS`, `SET_EQ`, `SET_PLACE_EQ`, `GET_EQ`, `GET_PLACE_EQ`, `STATUS`, `QUIT`).

## 1) OS Runtime Path Map (critical)

- macOS:
  - `~/Library/Application Support/woosh/woosh.sock`
  - `~/Library/Application Support/woosh/woosh.pid`
  - `~/Library/Application Support/woosh/woosh.log`
- Linux (typical):
  - `${XDG_DATA_HOME:-~/.local/share}/woosh/woosh.sock`
  - `${XDG_DATA_HOME:-~/.local/share}/woosh/woosh.pid`
  - `${XDG_DATA_HOME:-~/.local/share}/woosh/woosh.log`

If you use the wrong socket path, `nc` may appear to "do nothing."

## 2) Deterministic E2E Flow (always works)

### Terminal A
```bash
cd /Users/work/Development/woosh
cargo run -- daemon --no-daemonize
```

### Terminal B
```bash
if [[ "$(uname)" == "Darwin" ]]; then
  SOCK="$HOME/Library/Application Support/woosh/woosh.sock"
else
  SOCK="${XDG_DATA_HOME:-$HOME/.local/share}/woosh/woosh.sock"
fi

ls -l "$SOCK" || { echo "socket missing"; exit 1; }
printf 'PLAY_PLACE paris\nGET_PLACE_STATUS\nSTATUS\nQUIT\n' | nc -U "$SOCK"
```

Expected lines:
- `OK`
- `PLACE_STATUS place=paris:running:0.40` (volume may differ if changed)
- `STATUS synth=<preset>:<state>:<vol> place=paris:running:<vol>`

## 3) Fast Triage Matrix

- Symptom: nothing printed
  - Cause: wrong socket path or daemon not running
  - Checks:
    - `cargo run -- status` (must print STATUS; if error, daemon not reachable)
    - `ls -l "$SOCK"` (must exist as `srwx...`)
- Symptom: `daemon is already running`
  - Cause: stale process from prior test
  - Action: `cargo run -- stop`, then retry
- Symptom: `STATUS` works but `nc` shows nothing
  - Cause: `nc` pointed at different path than CLI runtime path
  - Action: recompute `SOCK` using OS map above

## 4) Protocol Smoke Payloads

Basic:
```bash
printf 'STATUS\nGET_EQ\nGET_PLACE_EQ\nGET_PLACE_STATUS\n'
```

Place control:
```bash
printf 'PLAY_PLACE tokyo\nSET_PLACE_VOLUME 0.7\nSET_PLACE_EQ 3 4.2\nGET_PLACE_STATUS\nGET_PLACE_EQ\nSTOP_PLACE\n'
```

## 5) Non-Negotiable Habits (prevents 90% of waste)

1. Never assume Linux XDG paths on macOS.
2. Verify socket existence before sending IPC commands.
3. Run `cargo run -- status` before manual `nc` tests.
4. End manual sessions with `QUIT` or `cargo run -- stop`.
5. When adding new IPC commands, update `tests/ipc_integration.rs` first or in same change.
6. After completing work, immediately refresh `ROADMAP.md` (task/status checkboxes) and `SPEC.md` (behavior/protocol/docs) so project docs stay current.
