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
- [x] Unit test: 10 000 sample sequence has mean ≈ 0 and std dev ≈ 0.577 (uniform [-1, 1]; ≈ 0.0996 since the 3.5.14 normalisation)

#### 1.2 — Audio output

- [x] `rodio::OutputStream` + `Sink` created in `daemon/audio.rs`
- [x] `NoiseSource` appended to `Sink` on `PLAY`; `Sink::pause()` / `Sink::play()` on `STOP` / `PLAY`
- [x] `Arc<Mutex<DaemonState>>` shared between audio thread and IPC handler

#### 1.3 — Daemonize & PID

- [x] `daemonize` crate: double-fork, stdout/stderr appended to `woosh.log`, write PID file
- [x] Paths via `dirs::data_dir()` → `~/Library/Application Support/woosh/` (macOS), `~/.local/share/woosh/` (Linux)
- [x] On startup, check for stale PID file (process dead); overwrite if stale
- [x] `tracing` logs to `woosh.log` in the data directory

#### 1.4 — IPC server

- [x] `tokio::net::UnixListener` on `woosh.sock` in the data directory
- [x] Spawn a task per connection; read lines, dispatch commands
- [x] Handle: `PLAY white`, `STOP`, `SET_VOLUME <f>`, `STATUS`, `QUIT`
- [x] `STATUS` response: `STATUS running preset=white volume=0.8` (or `stopped`)
- [x] `QUIT` removes socket + PID files and exits (since 3.5.15: returns from the server so the daemon fades out and kills mpv first)

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

- [x] `ratatui` + `crossterm` setup in `tui/mod.rs`: enable raw mode, alternate screen, cleanup on exit
- [x] `tokio::select!` event loop: crossterm events, 33 ms tick, socket read channel
- [x] `App` state machine in `tui/app.rs`: current screen, playing state, volume

#### 2.2 — Screen 1: Preset Selector

- [x] List widget showing `White Noise`, `Pink Noise`, `Brown Noise`
- [x] `↑`/`↓` navigation, `Enter` sends `PLAY <preset>`
- [x] Active preset highlighted; status indicator (playing / stopped)

#### 2.3 — IPC client

- [x] `tokio::net::UnixStream` in `tui/client.rs`
- [x] Async send (since 3.5.15: one persistent connection; every command awaits its reply)
- [x] Reconnect loop with exponential backoff if socket drops (since 3.5.15 for the main connection; previously only the unused sample subscription reconnected)

#### 2.4 — Screen 2: Visualizer layout

- [x] Header bar: app name, preset name, status dot, volume gauge
- [x] `BarChart` widget placeholder (static bars) for spectrum
- [x] Footer: key binding hints

#### 2.5 — Volume control

- [x] `←`/`→` on Screen 2 adjust volume ± 0.05, send `SET_VOLUME`
- [x] Volume gauge widget updates immediately (optimistic)

#### 2.6 — Sample ring buffer (daemon side) — removed in 3.5.8 / 3.5.15

- [x] `ringbuf` crate: `Producer` in `NoiseSource`, `Consumer` in IPC handler
- [x] Daemon sends `SAMPLES <hex>` to subscribed clients every 33 ms
- [x] Client subscribes with `SUBSCRIBE_SAMPLES` after connecting

#### 2.7 — Spectrum analyzer (TUI side) — removed in 3.5.8

- [x] Receive `SAMPLES` batches, parse hex to `Vec<f32>`
- [x] Apply Hann window, run FFT via `spectrum-analyzer` crate
- [x] Map 2048-bin output to 24 log-spaced frequency bars (20 Hz – 20 kHz)
- [x] Feed bar heights to `BarChart`; update each tick

#### 2.8 — Pink and brown noise (daemon side)

- [x] Pink noise: Paul Kellet 3-pole IIR in `NoiseSource`
- [x] Brown noise: running sum (random walk), normalised to ± 1
- [x] `PLAY pink` and `PLAY brown` recognised by IPC handler
- [x] Unit tests for approximate spectral slope (−3 dB/oct pink, −6 dB/oct brown)

#### 2.9 — Configuration file

- [x] `config.rs`: load `~/.config/woosh/config.toml` on daemon start
- [x] Write defaults if absent
- [x] Daemon starts at `defaults.volume`; the TUI's preset list starts on `defaults.preset` (since 3.5.15). `audio.sample_rate` was removed: output is fixed at 44.1 kHz mono

### Exit Criteria

`woosh` opens the TUI, plays the selected noise type, shows animated spectrum bars, and volume responds to keyboard. Closing the TUI leaves audio running. Reopening reconnects cleanly.

---

## Phase 3 — EQ Controls

**Goal:** 10-band graphic EQ with peaking filters, controllable from the TUI.

### Spec Coverage

- EQ system (§ EQ System)
- IPC `SET_EQ` / `GET_EQ` extensions (§ IPC Protocol)

### Tasks

#### 3.1 — Biquad filter library

- [x] Implement direct-form II transposed biquad in `daemon/eq.rs`
- [x] Coefficient calculator: peaking EQ (Audio EQ Cookbook); identity pass-through at 0 dB
- [x] Unit tests: identity at 0 dB, pass-through correctness, +6 dB boost amplifies b0

#### 3.2 — EQ processor wrapping NoiseSource

- [x] `EqProcessor<S>` struct in `src/daemon/eq.rs`: 10 peaking filters in series
- [x] Gains shared via `Arc<Mutex<[f32; N_BANDS]>>`; polled every 512 samples via `try_lock`
- [x] No state reset on coefficient change — avoids audible clicks
- [x] Appended to `rodio::Sink` as `EqProcessor<NoiseSource>`; persists across `PLAY` commands via shared Arc

#### 3.3 — IPC extensions

- [x] `SET_EQ <band_index> <gain_db>` command (band 0–9, gain clamped to ±12 dB, NaN-guarded)
- [x] `GET_EQ` response: `EQ v0 v1 ... v9` (space-separated floats)
- [ ] `config.toml` `[eq]` section persistence — deferred to Phase 4

#### 3.4 — TUI EQ screen

- [x] `e` key on Visualizer opens dedicated EQ screen (`Screen::Equalizer`)
- [x] `BarChart` showing 10 bands (±12 dB → 0–24 range); selected band highlighted in yellow
- [x] `←`/`→` navigate bands; `↑`/`↓` adjust gain ±1 dB; `r` resets all to 0 dB
- [x] `Esc`/`Backspace` returns to Visualizer; `q` quits
- [x] Readout line shows all gains with selected band bracketed: `[−3]  0  0  +6 …`
- [x] EQ state synced from daemon on TUI startup via `GET_EQ`

### Exit Criteria

✅ **Phase 3 Complete** — EQ screen opens from the Visualizer; adjusting bands changes the audible tone in real time. Gains reset to flat (0 dB) when the daemon restarts (config persistence is a Phase 4 follow-up).

---

## Phase 3.5 — Place Sounds & Interface Redesign

**Goal:** Add YouTube-based ambient place sounds with dual-channel architecture, and modernize the TUI with centered layout, colors, and ASCII art.

### Spec Coverage

- Place Sounds (§ Place Sounds)
- Audio Effects (§ Audio Effects)
- Dual-channel architecture (§ Architecture)
- TUI redesign (§ TUI, Interface Style)

### Tasks

#### 3.5.1 — Dual-channel audio architecture

- [x] Refactor `daemon/audio.rs` to create two separate `rodio::Sink` instances: `synth_sink` and `place_sink`
- [x] Update `DaemonState` to track two playback states: `synth_state` and `place_state`
- [x] Ensure both sinks share the same `OutputStream` to avoid device conflicts
- [x] Add mutex-protected shared state for volume and EQ parameters for each channel

#### 3.5.2 — mpv integration for YouTube place sounds

- [x] Create `daemon/mpv.rs` module to spawn mpv subprocess with PCM stdout capture
- [x] Implement YouTube search: `ytsearch1:walking through {place}` passed to mpv
- [x] Capture stdout PCM stream and wrap as `rodio::Source` via reader thread + `VecDeque<f32>`
- [x] Handle mpv process lifecycle (spawn on `PLAY_PLACE`, kill on `STOP_PLACE` via `Drop`)
- [x] Enforce single-place-at-a-time rule: kill old mpv before starting new place sound

#### 3.5.3 — Fade-in audio effect

- [x] Add `fade_samples` field to `NoiseSource` (u32 counter, 0 → 66,150 at 44,100 Hz ≈ 1.5 s)
- [x] Multiply each sample by `min(fade_samples / 66_150.0, 1.0)` and increment counter per sample
- [x] Apply same fade logic to `MpvSource` (place sounds)
- [x] Fade resets automatically on each `PLAY`/`PLAY_PLACE` because sources are recreated fresh

#### 3.5.3b — Fade-out audio effect

- [x] Add `fade_out: Arc<AtomicBool>` flag and `fade_out_samples: u32` counter to `NoiseSource`
- [x] On `STOP` command: set flag rather than calling `sink.pause()` immediately; source ramps volume 1.0 → 0.0 over 66,150 samples, then returns `None` to signal end of stream
- [x] Apply same fade-out logic to `MpvSource` (on `STOP_PLACE`)
- [x] After source yields `None`, rodio drains the sink naturally (no explicit drop required)
- [x] Unit test: collect samples after `fade_out` is triggered, confirm RMS decreasing envelope (start > midpoint)

#### 3.5.3c — No auto-play on daemon startup

**Spec misalignment:** The daemon currently appends a `NoiseSource` to the sink immediately on startup, so noise plays the moment the TUI opens (or the daemon is spawned) rather than waiting for the user to make a selection.

**Desired behaviour:** Daemon starts in a fully stopped state. The Preset Selector screen opens with no noise playing. The user presses `Enter` to start a preset for the first time.

- [x] Remove the initial `sink.append(NoiseSource::new(...))` call from `spawn_audio_thread`; start with an empty, paused sink (or no sink until the first `PLAY` command)
- [x] Update `DaemonState` initial `play_state` from `Running` → `Stopped`; clear `preset` default (use `Option<NoisePreset>`)
- [x] Update SPEC §Overview and §Audio Daemon to replace "launches the TUI with pink noise as default" → "launches the TUI in stopped state; user selects a preset to begin playback"
- [x] TUI Preset Selector: on first open show all three presets unselected; status indicator shows `stopped` until user presses `Enter`

#### 3.5.4 — Extended IPC protocol for place sounds

- [x] Add commands: `PLAY_PLACE <location>`, `STOP_PLACE`, `SET_PLACE_VOLUME <f32>`, `GET_PLACE_STATUS`
- [x] Add `SET_PLACE_EQ <band> <gain_db>` and `GET_PLACE_EQ` (10-band EQ for place channel)
- [x] Update `STATUS` response to include both channels: `STATUS synth=pink:playing:0.5 place=paris:playing:0.4`
- [x] Handle `PLAY_PLACE` conflicts: stop old place before starting new

#### 3.5.5 — CLI shortcuts for quick commands

- [x] Implement `woosh pink|white|brown` shortcuts (connect to daemon, send `PLAY`, exit)
- [x] Implement `woosh place <name>` subcommand (multi-word names work without quotes, sends `PLAY_PLACE`, exit)
- [x] Update `main.rs` CLI parser to route these before TUI launch
- [x] Change default to `pink` when no args — moot; `woosh` opens TUI in stopped state with the list on `defaults.preset` (default `pink`; White stays first in the list)

#### 3.5.6 — TUI centered layout with colors

- [x] Modify `tui/mod.rs` render loop to create centered `Rect` (max 80 cols × 24 rows) with padding
- [x] Add colored border using `Block::default().borders(Borders::ALL).border_style(Style::fg(Color::Cyan))`
- [x] Create layout: Title bar (3 rows) + Main content (16 rows) + Footer (2 rows) + Status bar (1 row)
- [x] Apply RGB color palette (blues, purples, pinks) throughout TUI

#### 3.5.6b — Eliminate fade counter precision casts

- [x] Change `fade_samples` and `fade_out_samples` fields in `NoiseSource` and `MpvSource` from `u32` to `f32` — removes the `cast_precision_loss` `#[allow]` workarounds added in 3.5.3b/3.5.3c and makes the fade arithmetic self-documenting (the counter is already a fraction, not a sample index)
- [x] Update arithmetic sites: `saturating_add` → direct increment (`+= 1.0`), `.min(66_150)` → `.min(66_150.0)`
- [x] Remove `#[allow(clippy::cast_precision_loss)]` attributes from both files

#### 3.5.7 — ASCII art and dithered backgrounds

- [x] Create `tui/art.rs` module with const string for ASCII logo
- [x] Add dithered wave/sound patterns in empty spaces (box-drawing chars: `░▒▓█`)
- [x] Display logo in title bar area
- [x] Render dithered background in margins outside centered content

#### 3.5.11 — Release audio device when both channels are stopped

- [x] Change audio thread to hold `Option<(OutputStream, OutputStreamHandle)>` instead of an always-open stream
- [x] Open `OutputStream` lazily on first `Play`/`PlayPlace` command
- [x] While the device is open, poll with `recv_timeout(50 ms)` (since 3.5.15; previously only while fading, which kept the device after a failed mpv start or a stream that ended by itself)
- [x] Each pass: drop voices whose sink is empty; drop the stream once no voice is playing or pending
- [x] Correct drop ordering enforced by Rust's reverse-declaration semantics (sinks declared after stream → dropped first)

#### 3.5.12 — Auto-shutdown daemon after configurable idle period

- [x] Add `DaemonConfig { idle_timeout_mins: u64 }` (default 15) to `src/config.rs` and `[daemon]` section in `config.toml`
- [x] Add `Arc<AtomicUsize>` client counter to `run_ipc_server`; RAII `ClientCountGuard` decrements on every connection exit path
- [x] Watchdog tokio task: checks every 60 s; if `client_count == 0` and both channels stopped for ≥ `idle_timeout_mins`, shuts the daemon down (since 3.5.15 the whole daemon exits; it used to stop only the audio thread and keep answering `OK`)
- [x] Thread the timeout through `mod.rs` → `run_ipc_server` as `Option<Duration>`; integration tests use `None`, or 100 ms to test the watchdog
- [x] All tests pass; zero clippy warnings

#### 3.5.8 — EQ as default screen (Visualizer removed)

- [x] Remove `Screen::Visualizer` variant; two screens remain: `Presets` and `Equalizer`
- [x] Enter/Space on Presets plays preset and navigates directly to `Equalizer`
- [x] Spectrum pipeline removed (FFT, bar_heights, sample_window fields; the leftover `SUBSCRIBE_SAMPLES` command, sample buffer and client code were removed in 3.5.15)
- [x] EQ footer updated: `s` stops playback; Esc/p/Tab returns to Presets
- [x] Presets footer hint updated to `Enter / Space  play + eq`

#### 3.5.13 — Volume control from EQ screen

- [x] `[` decreases synth volume by 5 % (clamped to 0.0); `]` increases by 5 % (clamped to 1.0)
- [x] Optimistic update: `app.volume` updated immediately; title bar reflects it on next tick
- [x] Sends `SET_VOLUME {:.2}` to the daemon (awaits the reply on the persistent connection since 3.5.15)
- [x] Footer hint updated: `[ ]  vol` appended to EQ screen hint line

#### 3.5.14 — Noise level normalization

White noise is ~3× louder than Pink/Brown by RMS (measured baseline: White 0.580, Pink 0.194, Brown 0.199).

- [x] Add `#[ignore]` test `measure_noise_levels` in `audio.rs` — prints RMS, peak, crest factor per preset (run with `cargo test measure_noise_levels -- --nocapture --ignored`)
- [x] Adjust per-preset scaling constants so all three types reach the same target RMS — done at ≈ 0.10 RMS rather than the ≈ 0.20 first proposed (see `src/daemon/AGENTS.md` baseline)

#### 3.5.15 — Whole-app review fixes (2026-11-02)

- [x] CI: `cargo fmt` clean; clippy `-D warnings` clean; every `#[allow]` explains itself
- [x] Integration tests use temp-dir socket/ready paths (`run_ipc_server` takes them) instead of the real data directory
- [x] One shutdown path for `QUIT`, idle timeout and SIGTERM: remove socket/ready/PID files, fade out both channels, kill mpv, exit
- [x] `woosh status` reads one reply line instead of waiting for EOF (it used to hang)
- [x] CLI play commands print daemon `ERROR` replies; `woosh <place>` shortcut (e.g. `woosh tokyo`)
- [x] mpv: `ytdl://` prefix (without it mpv never called yt-dlp, so no place sound ever played), valid output flags (`--audio-format=s16 --audio-samplerate=44100 --audio-channels=mono`; the old `--af=aformat=…` produced no audio on mpv 0.41), `--start=300` with a 10-minute minimum video length, lock-free bounded channel with backpressure instead of a capped buffer, fade-in waits for real audio
- [x] Place stream that ends by itself is marked stopped, so the device is released and the idle watchdog can fire
- [x] `PLAY_PLACE` fails with an error when `mpv` or `yt-dlp` is not on `PATH`
- [x] Audio device open failure: retry every 5 s, `STATUS … device=unavailable`, TUI banner
- [x] `SET_VOLUME` / `SET_PLACE_VOLUME` reject NaN
- [x] Config sections and keys default when missing (old config files no longer fall back to all defaults); parse errors are logged
- [x] TUI: one persistent connection with reconnect backoff; `STATUS` + `GET_EQ` synced every tick; request errors no longer exit the TUI; EQ changes applied to both channels; `Q` stops the daemon; `e`/Tab opens the EQ
- [x] TUI no longer hangs on quit (crossterm reader moved off `spawn_blocking`)
- [x] Daemon logging honours `RUST_LOG`
- [x] Removed dead code: `SUBSCRIBE_SAMPLES` pipeline, `ringbuf` and `spectrum-analyzer` crates, `NoiseSource` volume, `audio.sample_rate` config
- [x] Refactors: `Channel`/`ChannelId` replace parallel synth/place fields; shared `Fade` envelope; one handler per IPC command shape; `NoisePreset::ALL`/`id()`/`label()`
- [x] Tests for `SET_EQ`/`GET_EQ`, NaN, `QUIT`, idle shutdown, config defaults, `STATUS` parsing

#### 3.5.9 — Place Selector screen and dual volume controls

- [ ] Add new `PlaceSelector` screen accessible via `l` key
- [ ] Show currently playing place with volume slider
- [ ] Preset screen displays both synth and place status: `Synth: Pink ▶ 50% | Place: Paris ▶ 40%`
- [ ] Volume controls: `←`/`→` adjust synth, `Shift+←`/`Shift+→` adjust place

#### 3.5.10 — Volume defaults and config updates

- [ ] Reduce synth default volume from `0.8` → `0.5`
- [ ] Set place default volume to `0.4`
- [ ] Add `[place]` section to `config.toml` (reverb settings, enabled flag)
- [ ] Add `[tui]` section (default_screen, show_ascii_art, color_scheme)

### Exit Criteria

User can play synthetic noise + YouTube place sound simultaneously with independent volumes. TUI opens in a stopped state; noise only begins when the user explicitly selects a preset. All sounds fade in and fade out smoothly. TUI displays centered, colorful layout with ASCII art. CLI shortcuts `woosh pink` and `woosh tokyo` work. EQ is the primary screen (spectrum visualizer removed).

---

## Phase 4 — Presets & Polish

**Goal:** User-defined presets, keyboard shortcut help overlay, and UX refinements.

**Dependencies:** Requires Phase 3.5 completion.

### Tasks

#### 4.1 — Named presets

- [ ] `[presets]` section in `config.toml`: name → `{ noise_type, volume, eq, place_sound, place_volume }` map
- [ ] Preset Selector screen lists built-in + user presets
- [ ] `SAVE_PRESET <name>` IPC command saves current state (both channels) as a named preset
- [ ] `DELETE_PRESET <name>` IPC command

#### 4.2 — Help overlay

- [ ] `?` key toggles a full-screen key binding reference
- [ ] Rendered as a centered `Paragraph` widget over a dimmed background

#### 4.3 — Startup animation

- [ ] Brief "WOOSH" ASCII splash animation on first render before daemon connect completes (fade-in effect)

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

| Phase | Description | Key Deliverable | Status |
|-------|-------------|-----------------|--------|
| 0 | Project scaffold | Compilable repo, CI green | ✅ Complete |
| 1 | Audio daemon MVP | White noise plays, IPC works | ✅ Complete |
| 2 | TUI + Visualizer | Interactive UI, pink/brown noise (spectrum visualizer later removed) | ✅ Complete |
| 3 | EQ controls | Parametric EQ from TUI | ✅ Complete |
| 3.5 | Place sounds & UI redesign | YouTube integration, dual audio, centered layout, ASCII art | 🚧 In progress (3.5.9, 3.5.10 open) |
| 4 | Presets & polish | Named presets, help overlay, mouse support | 📋 Future |
| 5 | Packaging | Homebrew, AUR, crates.io | 📋 Future |

---

## Dependency Map

```
Phase 0 → Phase 1 → Phase 2 → Phase 3
                              ↓
                           Phase 4 → Phase 5
```

Each phase depends strictly on the previous. Phases 3, 4, and 5 can be developed concurrently once Phase 2 is complete.


# NOTE

If you are updating this file because a task has been completed, ask yourself: should I also update @SPEC.md or any of the AGENTS.md files in the source code folders or root? Could this change have made those files stale? If so, update them.
