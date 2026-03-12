# AGENTS.md — src/tui/

OVERVIEW: Tokio current_thread TUI client; renders ratatui UI, handles keyboard input.

## WHERE TO LOOK

| File      | Responsibility |
|-----------|----------------|
| mod.rs    | Runtime, event loop (33ms tick), terminal setup/teardown, key handling, rendering |
| app.rs    | `App` struct + `Screen` enum; all display state |
| client.rs | `DaemonClient`; IPC send, subscribe_samples, encode/decode |
| art.rs    | `LOGO_LINE` const (box-drawing "WOOSH" wordmark) + `DitherBackground` widget |

## SCREENS & KEYS

**Presets** (`Screen::Presets`): ↑/↓/j/k navigate; Enter/Space plays preset + switches to Equalizer; q quit.

**Equalizer** (`Screen::Equalizer`): ←/→ select band (0–9); ↑/↓ gain ±1 dB (clamped ±12 dB); r reset all bands; s stop; Esc/p/Tab → Presets; q quit.

## IPC CLIENT NOTES

- `send_command()`: send + read first response line. Use for STATUS, GET_EQ, STOP.
- `send_fire_and_forget()`: send only, no read. Use for PLAY, SET_VOLUME, SET_EQ (high-frequency).
- `subscribe_samples()`: retained in client.rs for future use; not wired into the event loop.
- Wire encoding: f32 ↔ little-endian hex, 8 chars/sample. `decode_samples` parses `"SAMPLES <hex>"` lines.
- `eq_gains` in `App` mirrors daemon state for display only; daemon is authoritative.

## RATATUI GOTCHAS

- `BarChart::data()` takes `impl Into<BarGroup>`; pass `bar_data.as_slice()` where `bar_data: Vec<(&str, u64)>`.
- Import `Bar`/`BarGroup` from `ratatui::widgets` directly, NOT `ratatui::widgets::bar`.

## ART MODULE

`art.rs` exposes two public items:
- `LOGO_LINE: &str` — compact single-line "WOOSH" wordmark using box-drawing chars (`╦ ╦╔═╗╔═╗╔═╗╦ ╦`). Used in the title bar span.
- `DitherBackground` — zero-size widget that fills a `Rect` with a deterministic diagonal dither pattern (`░▒` characters at `Rgb(45,35,80)` fg on `Rgb(8,6,18)` bg). Called by `render_dither_margins()` in `mod.rs` before the centered content box is drawn.

`render_dither_margins()` in `mod.rs` computes four strips (top, bottom, left, right) around the centered `outer` rect and renders `DitherBackground` into each non-empty strip.

## ANTI-PATTERNS

- NEVER block the tokio runtime in the event loop — use `spawn_blocking` for crossterm event polling.
- NEVER await IPC responses for volume or EQ changes — use `fire_and_forget`.
- NEVER assume the daemon is running — IPC calls should be handled gracefully.
- NEVER write `KeyCode::Char('q') | KeyCode::Char('Q')`; clippy requires `KeyCode::Char('q' | 'Q')`.
