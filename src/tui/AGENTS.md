# AGENTS.md — src/tui/

OVERVIEW: Tokio current_thread TUI client; renders ratatui UI, handles keyboard input, mirrors daemon state over one persistent IPC connection.

## WHERE TO LOOK

| File      | Responsibility |
|-----------|----------------|
| mod.rs    | Runtime, event loop (33 ms tick), `sync_from_daemon()`, `handle_key()`, terminal setup/teardown, rendering |
| app.rs    | `App` struct + `Screen` enum; all display state |
| client.rs | `DaemonClient`: persistent connection, reconnect backoff, typed requests, `STATUS` parsing |
| art.rs    | `LOGO_LINE` const (fullwidth "ＷＯＯＳＨ" wordmark) + `DitherBackground` widget |

## SCREENS & KEYS

**Global:** `q` quit TUI (daemon keeps playing); `Q` send `QUIT` (daemon fades out and exits) and quit.

**Presets** (`Screen::Presets`): ↑/↓/j/k navigate; Enter/Space plays preset + switches to Equalizer; `e`/Tab → Equalizer. The list starts on `[defaults] preset` from config.

**Equalizer** (`Screen::Equalizer`): ←/→/h/l select band (0–9); ↑/↓/k/j gain ±1 dB (clamped ±12 dB); r reset all bands; s toggle play/stop (resumes last preset if stopped); `[` / `]` synth volume ∓5 %; Esc/p/Tab/Backspace → Presets.

Not built yet (ROADMAP 3.5.9 / Phase 4): `l` Place Selector, ←/→ volume on Presets, Shift+←/→ place volume, `?` help.

## DAEMON SYNC

- Every tick, `sync_from_daemon()` sends `STATUS` and `GET_EQ` and copies preset, play state, volume, device state and EQ gains into `App`. On failure it sets `app.connected = false`; the status bar shows "disconnected".
- Key handlers update `App` immediately (so two presses within a tick build on each other), then send the request. Request errors are ignored; the next sync shows the truth.
- EQ changes go to both channels (`SET_EQ` + `SET_PLACE_EQ`), which share one EQ in the UI.

## IPC CLIENT NOTES

- `DaemonClient` keeps one connection open. That makes the daemon count the TUI as a client, so the idle watchdog never shuts the daemon down under an open TUI.
- Every command awaits its one-line reply (`REPLY_TIMEOUT` 500 ms). Unix-socket round-trips are sub-millisecond; on a shared connection an unread reply would be read as the answer to the next command.
- On any error the connection is dropped; reconnects back off 100 ms → 200 ms → … → 5 s, and requests fail immediately between attempts so the UI never stalls.
- `quit_daemon()` writes `QUIT` without waiting (QUIT has no reply).

## RATATUI GOTCHAS

- `BarChart::data()` takes `impl Into<BarGroup>`; pass `BarGroup::default().bars(&bars)`.
- Import `Bar`/`BarGroup` from `ratatui::widgets` directly, NOT `ratatui::widgets::bar`.
- EQ bars use `.text_value(format_gain(..))` to show dB values ("+3", "-2", "0"); height is `eq_bar_height()` (`GAIN_MIN` → 0, `GAIN_MAX` → top).
- EQ bar colours: unselected → `EQ_BAR_FG`; selected → `EQ_SELECTED_FG`.
- EQ screen uses one chart area; selected band info shown in block title.

## ART MODULE

`art.rs` exposes two public items:
- `LOGO_LINE: &str` — single-line wordmark in fullwidth letters (`" ＷＯＯＳＨ "`). Used in the title bar span.
- `DitherBackground` — zero-size widget that fills a `Rect` with a deterministic diagonal dither pattern (`░▒` at `DITHER_FG` `Rgb(64,58,54)` on `DITHER_BG` `Rgb(41,37,34)`). Called by `render_dither_margins()` in `mod.rs` before the centered content box is drawn.

`render_dither_margins()` in `mod.rs` computes four strips (top, bottom, left, right) around the centered `outer` rect and renders `DitherBackground` into each non-empty strip.

## ANTI-PATTERNS

- NEVER block the tokio runtime in the event loop — crossterm events are read on a detached `std::thread`.
- NEVER use `spawn_blocking` for the crossterm reader: runtime shutdown waits for blocking tasks, so the TUI hangs on quit until the next key press.
- NEVER send a command on the persistent connection without reading its reply.
- NEVER assume the daemon is running — request errors must not end the TUI.
- NEVER write `KeyCode::Char('q') | KeyCode::Char('Q')`; clippy requires `KeyCode::Char('q' | 'Q')`.
