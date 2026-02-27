# AGENTS.md — src/tui/

OVERVIEW: Tokio current_thread TUI client; renders ratatui UI, streams spectrum data from daemon, handles keyboard input.

## WHERE TO LOOK

| File      | Responsibility |
|-----------|----------------|
| mod.rs    | Runtime, event loop (33ms tick), terminal setup/teardown, spectrum FFT pipeline |
| app.rs    | `App` struct + `Screen` enum; all display state |
| client.rs | `DaemonClient`; IPC send, subscribe_samples, encode/decode |

## SCREENS & KEYS

**Presets** (`Screen::Presets`): ↑/↓/j/k navigate; Enter/Space plays + switches to Visualizer; q quit.

**Visualizer** (`Screen::Visualizer`): ←/→ volume ±5% (optimistic); p/Tab → Presets; e → Equalizer; s stop; q quit.

**Equalizer** (`Screen::Equalizer`): ←/→ select band (0–9); ↑/↓ gain ±1 dB (clamped ±12 dB); r reset all bands; Esc/Backspace → Visualizer; q quit.

## SPECTRUM PIPELINE

1. `subscribe_samples` pushes decoded f32 chunks into `samples_rx`.
2. Main loop appends to `app.sample_window` (rolling cap 4096).
3. Take last 2048, zero-pad if short, apply Hann window.
4. spectrum-analyzer FFT (20 Hz–20 kHz).
5. Log-map to 24 bars, scale 0–100, apply per-bar peak-hold → `app.bar_heights`.

`data()` on spectrum-analyzer returns `&[(Frequency, FrequencyValue)]`; unwrap values with `.val() -> f32` (both are `OrderableF32`).

## IPC CLIENT NOTES

- `send_command()`: send + read first response line. Use for STATUS, GET_EQ.
- `send_fire_and_forget()`: send only, no read. Use for PLAY, SET_VOLUME, SET_EQ (high-frequency).
- `subscribe_samples()`: dedicated async task; exponential backoff 100 ms → 5 s on disconnect; reconnects automatically so TUI survives daemon restart.
- Wire encoding: f32 ↔ little-endian hex, 8 chars/sample. `decode_samples` parses `"SAMPLES <hex>"` lines.
- `eq_gains` in `App` mirrors daemon state for display only; daemon is authoritative.

## RATATUI GOTCHAS

- `BarChart::data()` takes `impl Into<BarGroup>`; pass `bar_data.as_slice()` where `bar_data: Vec<(&str, u64)>`.
- Import `Bar`/`BarGroup` from `ratatui::widgets` directly, NOT `ratatui::widgets::bar`.

## ANTI-PATTERNS

- NEVER block the tokio runtime in the event loop — use `spawn_blocking` for crossterm event polling.
- NEVER await IPC responses for volume or EQ changes — use `fire_and_forget`.
- NEVER assume the daemon is running — `subscribe_samples` must handle reconnect silently.
- NEVER write `KeyCode::Char('q') | KeyCode::Char('Q')`; clippy requires `KeyCode::Char('q' | 'Q')`.
