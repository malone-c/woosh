# Woosh — Development Roadmap

Phased plan from zero to full-featured. Each phase produces a working, shippable artifact; later phases layer on top without breaking earlier ones.

---

## Phase 0 — Project Scaffold

**Goal:** Compilable project with CI, linting, and the correct dependency tree in place.

### Tasks

- [x] `cargo new woosh` with workspace layout (single crate for now)
- [x] Add all Phase 1 dependencies to `Cargo.toml` (`rodio`, `ratatui`, `crossterm`, `tokio`, `rand`, `daemonize`, `dirs`, `anyhow`, `tracing`, `tracing-subscriber`, `serde`, `toml`)
- [x] Set up `rustfmt.toml` and `clippy` deny list (`clippy::all`, `clippy::pedantic`)
- [x] Add `.github/workflows/ci.yml`: `cargo check`, `cargo clippy`, `cargo test`, `cargo fmt --check`
- [x] Stub `main.rs` with a CLI argument parser (`clap`) that routes to `daemon`, `stop`, `status`, and default (TUI) subcommands — each printing "not yet implemented"
- [x] Verify `cargo build --release` succeeds with zero warnings

### Exit Criteria

`cargo build --release` completes cleanly; CI pipeline is green on a trivial commit.

---

## Phase 1 — Audio Daemon (MVP)

**Goal:** A background daemon that plays continuous white noise and responds to basic IPC commands.

### Spec Coverage

- Noise generation: white noise only (§ Noise Generation)
- Audio daemon lifecycle (§ Audio Daemon)
- IPC protocol: `PLAY`, `STOP`, `SET_VOLUME`, `STATUS`, `QUIT` (§ IPC Protocol)

### Tasks

#### 1.1 — Noise source

- [x] Implement `NoiseSource` struct with `Iterator<Item = f32>` and `rodio::Source` trait
- [x] White noise: uniform random samples from `SmallRng` (seeded via `rand::thread_rng()`)
- [x] Per-sample volume scaling via `self.volume`
- [x] Unit test: 10 000 sample sequence has mean ≈ 0 and std dev ≈ 0.577 (uniform [-1, 1])

#### 1.2 — Audio output

- [x] `rodio::OutputStream` + `Sink` created in `daemon/audio.rs`
- [x] `NoiseSource` appended to `Sink` on `PLAY`; `Sink::pause()` / `Sink::play()` on `STOP` / `PLAY`
- [x] `Arc<Mutex<DaemonState>>` shared between audio thread and IPC handler

#### 1.3 — Daemonize & PID

- [x] `daemonize` crate: double-fork, redirect stdio to `/dev/null`, write PID file
- [x] XDG-aware paths via `dirs::data_dir()` → `~/.local/share/woosh/`
- [x] On startup, check for stale PID file (process dead); overwrite if stale
- [x] `tracing` logs to `~/.local/share/woosh/woosh.log`

#### 1.4 — IPC server

- [x] `tokio::net::UnixListener` on `~/.local/share/woosh/woosh.sock`
- [x] Spawn a task per connection; read lines, dispatch commands
- [x] Handle: `PLAY white`, `STOP`, `SET_VOLUME <f>`, `STATUS`, `QUIT`
- [x] `STATUS` response: `STATUS running preset=white volume=0.8` (or `stopped`)
- [x] `QUIT` removes socket + PID files and calls `std::process::exit(0)`

#### 1.5 — Auto-spawn from TUI entry point

- [x] `main.rs` default path: check PID file → if absent/stale, `spawn woosh daemon` detached → wait for socket file (poll 10 ms, timeout 500 ms) → connect

#### 1.6 — `woosh stop` and `woosh status` subcommands

- [x] Connect to socket, send `QUIT` or `STATUS`, print response, exit

#### 1.7 — Integration test

- [x] Spawn daemon in-process (no daemonize) via a test helper, send IPC commands, assert responses

### Exit Criteria

`woosh` plays audible white noise in the background. `woosh status` prints `STATUS running preset=white volume=0.8`. `woosh stop` silences it. Closing the terminal leaves audio playing.

---

## Phase 2 — TUI Client & Visualizer

**Goal:** A rich terminal UI that connects to the daemon, shows a live spectrum analyzer, and exposes volume + preset controls.

### Spec Coverage

- TUI screens 1 and 2 (§ TUI)
- Spectrum analyzer data flow (§ Spectrum Analyzer)
- Pink and brown noise types (§ Noise Generation)

### Tasks

#### 2.1 — TUI skeleton

- [ ] `ratatui` + `crossterm` setup in `tui/mod.rs`: enable raw mode, alternate screen, cleanup on exit
- [ ] `tokio::select!` event loop: crossterm events, 33 ms tick, socket read channel
- [ ] `App` state machine in `tui/app.rs`: current screen, playing state, volume

#### 2.2 — Screen 1: Preset Selector

- [ ] List widget showing `White Noise`, `Pink Noise`, `Brown Noise`
- [ ] `↑`/`↓` navigation, `Enter` sends `PLAY <preset>`
- [ ] Active preset highlighted; status indicator (playing / stopped)

#### 2.3 — IPC client

- [ ] `tokio::net::UnixStream` in `tui/client.rs`
- [ ] Async send (fire-and-forget for `SET_VOLUME`; await `OK`/`ERROR` for `PLAY`/`STOP`)
- [ ] Reconnect loop with exponential backoff if socket drops

#### 2.4 — Screen 2: Visualizer layout

- [ ] Header bar: app name, preset name, status dot, volume gauge
- [ ] `BarChart` widget placeholder (static bars) for spectrum
- [ ] Footer: key binding hints

#### 2.5 — Volume control

- [ ] `←`/`→` on Screen 2 adjust volume ± 0.05, send `SET_VOLUME`
- [ ] Volume gauge widget updates immediately (optimistic)

#### 2.6 — Sample ring buffer (daemon side)

- [ ] `ringbuf` crate: `Producer` in `NoiseSource`, `Consumer` in IPC handler
- [ ] Daemon sends `SAMPLES <hex>` to subscribed clients every 33 ms
- [ ] Client subscribes with `SUBSCRIBE_SAMPLES` after connecting

#### 2.7 — Spectrum analyzer (TUI side)

- [ ] Receive `SAMPLES` batches, parse hex to `Vec<f32>`
- [ ] Apply Hann window, run FFT via `spectrum-analyzer` crate
- [ ] Map 2048-bin output to 24 log-spaced frequency bars (20 Hz – 20 kHz)
- [ ] Feed bar heights to `BarChart`; update each tick

#### 2.8 — Pink and brown noise (daemon side)

- [ ] Pink noise: Paul Kellet 3-pole IIR in `NoiseSource`
- [ ] Brown noise: running sum (random walk), normalised to ± 1
- [ ] `PLAY pink` and `PLAY brown` recognised by IPC handler
- [ ] Unit tests for approximate spectral slope (−3 dB/oct pink, −6 dB/oct brown)

#### 2.9 — Configuration file

- [ ] `config.rs`: load `~/.config/woosh/config.toml` on daemon start
- [ ] Write defaults if absent
- [ ] Daemon respects `defaults.preset`, `defaults.volume`, `audio.sample_rate`

### Exit Criteria

`woosh` opens the TUI, plays the selected noise type, shows animated spectrum bars, and volume responds to keyboard. Closing the TUI leaves audio running. Reopening reconnects cleanly.

---

## Phase 3 — EQ Controls

**Goal:** Parametric EQ with four bands, controllable from the TUI.

### Spec Coverage

- EQ system (§ EQ System)
- IPC `SET_EQ` / `GET_EQ` extensions (§ IPC Protocol)

### Tasks

#### 3.1 — Biquad filter library

- [ ] Implement direct-form II transposed biquad in `daemon/audio.rs`
- [ ] Coefficient calculators: low shelf, high shelf, peaking EQ (Audio EQ Cookbook)
- [ ] Unit test: apply known filter, verify frequency response at target frequency (± 0.5 dB)

#### 3.2 — EQ chain in NoiseSource

- [ ] `EqChain` struct: four `BiquadFilter` instances in series
- [ ] Parameters: `gain_db`, `freq_hz`, `q` per band
- [ ] Thread-safe update via `Arc<Mutex<DaemonState>>`

#### 3.3 — IPC extensions

- [ ] `SET_EQ <band> <gain_db> [freq] [q]` command
- [ ] `GET_EQ` response: all band parameters as key=value pairs
- [ ] `config.toml` `[eq]` section persisted on `SET_EQ`

#### 3.4 — TUI EQ panel

- [ ] `e` key on Screen 2 opens EQ overlay panel
- [ ] Four vertical sliders (−12 dB to +12 dB) for each band
- [ ] `←`/`→` navigate bands; `↑`/`↓` adjust gain; `Esc` closes panel
- [ ] Band labels: `Low Shelf`, `Peak 1`, `Peak 2`, `High Shelf`
- [ ] Sends `SET_EQ` on each change (live preview)

### Exit Criteria

EQ overlay opens, adjusting sliders changes the audible tone, and settings survive daemon restarts (persisted to config).

---

## Phase 4 — Presets & Polish

**Goal:** User-defined presets, keyboard shortcut help overlay, and UX refinements.

### Tasks

#### 4.1 — Named presets

- [ ] `[presets]` section in `config.toml`: name → `{ noise_type, volume, eq }` map
- [ ] Preset Selector screen lists built-in + user presets
- [ ] `SAVE_PRESET <name>` IPC command saves current state as a named preset
- [ ] `DELETE_PRESET <name>` IPC command

#### 4.2 — Help overlay

- [ ] `?` key toggles a full-screen key binding reference
- [ ] Rendered as a centered `Paragraph` widget over a dimmed background

#### 4.3 — Startup animation

- [ ] Brief "woosh" ASCII splash on first render before daemon connect completes

#### 4.4 — Status bar refinements

- [ ] Animated spinner while reconnecting to daemon
- [ ] Colour-coded status dot: green = playing, yellow = stopped, red = error

#### 4.5 — Mouse support

- [ ] Click to select preset
- [ ] Click-and-drag volume bar
- [ ] `crossterm` mouse capture enable/disable via config

#### 4.6 — README

- [ ] Installation instructions (cargo install, AUR, Homebrew tap)
- [ ] Usage examples with screenshots (recorded with `vhs`)
- [ ] Config file reference

### Exit Criteria

A polished, documented release candidate that a new user can install and use without reading the source code.

---

## Phase 5 — Packaging & Distribution

**Goal:** Installable via common package managers.

### Tasks

- [ ] Homebrew formula (`woosh.rb`) — macOS
- [ ] AUR PKGBUILD — Arch Linux
- [ ] Debian/Ubuntu `.deb` via `cargo-deb`
- [ ] GitHub Releases with pre-built binaries (macOS aarch64, macOS x86_64, Linux x86_64) built in CI
- [ ] `cargo publish` to crates.io

### Exit Criteria

`brew install woosh` installs and runs correctly on macOS. Binary release artifacts are attached to the GitHub release.

---

## Milestone Summary

| Phase | Description | Key Deliverable |
|-------|-------------|-----------------|
| 0 | Project scaffold | Compilable repo, CI green |
| 1 | Audio daemon MVP | White noise plays, IPC works |
| 2 | TUI + Visualizer | Interactive UI, spectrum bars, pink/brown noise |
| 3 | EQ controls | Parametric EQ from TUI |
| 4 | Presets & polish | Named presets, help overlay, mouse support |
| 5 | Packaging | Homebrew, AUR, crates.io |

---

## Dependency Map

```
Phase 0 → Phase 1 → Phase 2 → Phase 3
                              ↓
                           Phase 4 → Phase 5
```

Each phase depends strictly on the previous. Phases 3, 4, and 5 can be developed concurrently once Phase 2 is complete.
