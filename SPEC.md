# Woosh — Product Specification

> A terminal white noise generator with a daemon/TUI architecture, spectrum visualization, and EQ controls.

---

## Table of Contents

1. [Overview](#overview)
2. [Architecture](#architecture)
3. [Noise Generation](#noise-generation)
4. [Audio Daemon](#audio-daemon)
5. [IPC Protocol](#ipc-protocol)
6. [TUI](#tui)
7. [Spectrum Analyzer](#spectrum-analyzer)
8. [EQ System](#eq-system)
9. [Crate Inventory](#crate-inventory)
10. [File & Directory Layout](#file--directory-layout)
11. [Configuration](#configuration)
12. [Error Handling](#error-handling)
13. [Platform Support](#platform-support)

---

## Overview

Woosh is a command-line white noise app written in Rust. It plays continuous synthetic ambient noise (white, pink, brown) through the system audio output. A persistent background daemon owns the audio device and synthesis pipeline; a lightweight TUI client connects to the daemon over a Unix domain socket to control playback and display a live spectrum visualizer.

### Goals

- **No sample files** — all noise is generated algorithmically at runtime.
- **Low resource usage** — the daemon should idle at < 1 % CPU when playing.
- **Interactive TUI** — real-time visualizer, volume control, EQ, and preset switching without leaving the terminal.
- **Daemon persistence** — close the TUI and audio keeps playing; reopen TUI and it reconnects instantly.

### Non-Goals

- GUI / native app
- Streaming from internet sources
- Multi-output routing or per-app audio mixing

---

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                    woosh binary                     │
│                                                     │
│  ┌──────────────┐        ┌──────────────────────┐  │
│  │  TUI Client  │◄──────►│   Audio Daemon       │  │
│  │  (ratatui)   │ Unix   │   (rodio + synth)    │  │
│  │  crossterm   │ socket │   daemonize          │  │
│  └──────────────┘        └──────────────────────┘  │
└─────────────────────────────────────────────────────┘
```

The single compiled binary (`woosh`) serves both roles:

- **`woosh`** (no subcommand) — launches the TUI. If no daemon is running, it spawns one in the background first.
- **`woosh daemon`** — starts the daemon in the foreground (used internally by the auto-spawn path).
- **`woosh stop`** — sends `QUIT` to the daemon and exits.
- **`woosh status`** — prints current daemon state to stdout and exits.

---

## Noise Generation

### Strategy

Noise is synthesised algorithmically. No WAV/OGG files are bundled or loaded.

### Noise Types

| Type  | Algorithm | Description |
|-------|-----------|-------------|
| White | PRNG — uniform distribution over `[-1.0, 1.0]` | Equal energy per Hz; sounds bright/hissy |
| Pink  | Paul Kellet's 3-pole IIR approximation | Equal energy per octave; natural, balanced |
| Brown | Integration of white noise (random walk) | High bass content; deep rumble |

**MVP ships white noise only.** Pink and brown are reserved for Phase 2.

### Implementation

A custom struct implements `rodio::Source`:

```
struct NoiseSource {
    sample_rate: u32,
    channels: u16,
    noise_type: NoiseType,
    volume: f32,          // 0.0 – 1.0, applied per sample
    eq_chain: EqChain,    // passthrough at MVP
    rng: SmallRng,        // rand crate, seeded from entropy
    // pink noise state
    pink_b: [f64; 3],
}

impl Iterator for NoiseSource { type Item = f32; … }
impl Source for NoiseSource { … }
```

`rodio::OutputStream` is created once at daemon startup and `NoiseSource` is appended to the sink. Volume changes mutate `NoiseSource.volume` in the next audio callback; a `Mutex<SharedState>` bridges IPC handler threads and the synthesis thread.

### Sample Rate

Default 44 100 Hz, stereo. Configurable in `~/.config/woosh/config.toml`.

---

## Audio Daemon

### Responsibilities

- Own the `rodio::OutputStream` and `Sink`.
- Run the synthesis loop (via the `rodio::Source` implementation).
- Listen on the Unix socket and handle IPC commands.
- Maintain a fixed-size ring buffer of recent PCM samples for the visualizer.
- Write and clean up the PID file.

### Lifecycle

1. On first `woosh` invocation, the TUI checks for `woosh.pid`.
2. If PID file absent or stale (process not alive), spawn `woosh daemon` as a detached child using `daemonize`.
3. Wait up to 500 ms for the socket file to appear, then connect.
4. On `QUIT` command or `SIGTERM`, flush audio, remove socket and PID file, exit.

### PID & Socket Paths

| File | Path |
|------|------|
| PID file | `~/.local/share/woosh/woosh.pid` |
| Unix socket | `~/.local/share/woosh/woosh.sock` |

Both paths respect `$XDG_DATA_HOME` if set.

### Daemonize

Uses the `daemonize` crate:

- Double-fork to detach from the controlling terminal.
- Redirect stdin/stdout/stderr to `/dev/null`.
- Writes PID to `woosh.pid` after fork.

---

## IPC Protocol

A simple line-oriented text protocol over a Unix domain socket. Each message is a single UTF-8 line terminated by `\n`.

### Client → Daemon

| Command | Example | Description |
|---------|---------|-------------|
| `PLAY <preset>` | `PLAY white` | Start or switch to a noise preset |
| `STOP` | `STOP` | Pause audio output |
| `SET_VOLUME <f32>` | `SET_VOLUME 0.75` | Set volume, clamped to `[0.0, 1.0]` |
| `SET_EQ <band> <gain_db>` | `SET_EQ high_shelf 3.0` | Adjust an EQ band (Phase 3+) |
| `STATUS` | `STATUS` | Request current state |
| `QUIT` | `QUIT` | Shut down daemon |
| `SAVE_PRESET <name>` | `SAVE_PRESET focus` | Save current state as a named preset |
| `DELETE_PRESET <name>` | `DELETE_PRESET focus` | Remove a named preset |

### Daemon → Client

| Response | Example | Description |
|----------|---------|-------------|
| `OK` | `OK` | Command accepted |
| `ERROR <msg>` | `ERROR unknown preset` | Command rejected with reason |
| `STATUS <fields>` | `STATUS running preset=white volume=0.8` | State snapshot |
| `SAMPLES <hex>` | `SAMPLES 3f800000...` | Batch of PCM samples (little-endian f32 hex) for visualizer |

The TUI polls `STATUS` every tick and subscribes to unsolicited `SAMPLES` pushes after sending a `SUBSCRIBE_SAMPLES` command (added in Phase 2).

### Concurrency

The daemon spawns one Tokio task per accepted client connection. A shared `Arc<Mutex<DaemonState>>` protects mutable state (current preset, volume, EQ params, sample ring buffer).

---

## TUI

### Technology

- **`ratatui`** — terminal UI framework
- **`crossterm`** — cross-platform terminal backend (raw mode, events)
- **`tokio`** — async runtime; `tokio::select!` multiplexes crossterm events, socket reads, and a 33 ms tick timer (~30 fps)

### Screens

#### Screen 1 — Preset Selector

```
┌─────────────────────────────────┐
│  woosh                          │
├─────────────────────────────────┤
│                                 │
│  ▶ White Noise                  │
│    Pink Noise                   │
│    Brown Noise                  │
│                                 │
│  [↑/↓] select  [Enter] play     │
│  [q] quit      [→] visualizer   │
└─────────────────────────────────┘
```

- Arrow keys navigate the list.
- `Enter` sends `PLAY <preset>` to daemon.
- `→` or `Tab` switches to Screen 2.

#### Screen 2 — Visualizer + Controls

```
┌─────────────────────────────────────────────────────┐
│  woosh  ●  white noise  vol: ████████░░  80%        │
├─────────────────────────────────────────────────────┤
│                                                     │
│  ▄  ▄▄  █  ▄▄  █▄  ▄▄  ▄  ▄▄  ▄  ▄                  │
│  █  ██  █  ██  ██  ██  █  ██  █  █                  │
│  █  ██  █  ██  ██  ██  █  ██  █  █                  │
│  ──────────────────────────────────                 │
│  63  125 250 500 1k  2k  4k  8k 16k  Hz             │
│                                                     │
│  [←/→] volume  [e] eq  [p] presets  [q] quit        │
└─────────────────────────────────────────────────────┘
```

- Left/right arrow adjusts volume (sends `SET_VOLUME`).
- `e` opens EQ panel (Phase 3).
- `p` or `←` returns to Screen 1.
- Spectrum bars update at ~30 fps.

### Mouse Support

> **Phase 4 feature — not in MVP.**

- Click to select a preset from the list.
- Click-and-drag on the volume bar to set volume.
- Mouse capture can be disabled in `config.toml` (`[tui] mouse = false`).

### Key Bindings (global)

| Key | Action |
|-----|--------|
| `q` | Quit TUI (daemon keeps running) |
| `Q` (shift) | Quit TUI and stop daemon |
| `p` | Go to Preset Selector |
| `v` | Go to Visualizer |
| `←` / `→` | Volume down / up (on Visualizer screen) |
| `↑` / `↓` | Navigate list (on Preset screen) |
| `Enter` | Confirm selection |
| `?` | Toggle help overlay |

---

## Spectrum Analyzer

### Data Flow

```
NoiseSource → ring buffer (daemon) → SAMPLES push (IPC) → TUI FFT → BarChart
```

1. `NoiseSource` writes each generated sample into a lock-free ring buffer (`ringbuf` crate, capacity 4096 f32 samples).
2. The daemon's sample-push task reads the ring buffer every ~33 ms and sends a `SAMPLES` message to all subscribed TUI clients.
3. The TUI receives the sample batch, runs an FFT via the `spectrum-analyzer` crate (which wraps `rustfft`), and maps frequency bins to bar heights.
4. A Hann window is applied before the FFT to reduce spectral leakage.
5. Bars are rendered with `ratatui::widgets::BarChart`, logarithmically scaled on the frequency axis.

### Parameters

| Parameter | Value |
|-----------|-------|
| FFT window size | 2048 samples |
| Overlap | 50 % (1024-sample hop) |
| Window function | Hann |
| Frequency axis | Log scale, 20 Hz – 20 kHz |
| Number of display bars | 24 |
| Update rate | ~30 fps |

---

## EQ System

> **Phase 3 feature — not in MVP.**

### Filter Types

| Band | Filter type | Default |
|------|-------------|---------|
| Low shelf | 2nd-order biquad | 0 dB @ 200 Hz |
| Peak 1 | Peaking EQ | 0 dB @ 1 kHz, Q = 1.0 |
| Peak 2 | Peaking EQ | 0 dB @ 4 kHz, Q = 1.0 |
| High shelf | 2nd-order biquad | 0 dB @ 8 kHz |

### Implementation

Biquad filters are implemented in the `NoiseSource` chain using the Audio EQ Cookbook coefficients. Each filter is a direct-form II transposed structure for numerical stability.

### IPC Extension

```
SET_EQ <band_id> <gain_db> [freq_hz] [q]
GET_EQ
```

The TUI renders sliders for each band in a pop-up panel (triggered by `e`).

---

## Crate Inventory

| Crate | Version | Purpose |
|-------|---------|---------|
| `ratatui` | 0.29 | Terminal UI framework |
| `crossterm` | 0.28 | Terminal backend / event input |
| `rodio` | 0.20 | Audio output / source trait |
| `tokio` | 1 (full) | Async runtime |
| `rand` | 0.8 | PRNG for noise generation |
| `daemonize` | 0.5 | UNIX process daemonization |
| `ringbuf` | 0.4 | Lock-free ring buffer (sample sharing) |
| `spectrum-analyzer` | 1.5 | FFT + frequency analysis (wraps rustfft) |
| `serde` | 1 | Config serialization |
| `toml` | 0.8 | Config file parsing |
| `dirs` | 5 | XDG-aware path resolution |
| `anyhow` | 1 | Error handling |
| `tracing` | 0.1 | Structured logging (daemon) |
| `tracing-subscriber` | 0.3 | Log output to file |
| `clap` | 4 | CLI argument parsing (subcommands) |

---

## File & Directory Layout

### Repository

```
woosh/
├── Cargo.toml
├── Cargo.lock
├── SPEC.md
├── ROADMAP.md
├── src/
│   ├── main.rs            # CLI entry point, subcommand dispatch
│   ├── daemon/
│   │   ├── mod.rs         # Daemon entry point, lifecycle
│   │   ├── audio.rs       # NoiseSource, rodio setup
│   │   ├── ipc.rs         # Unix socket listener, command handler
│   │   └── state.rs       # DaemonState, ring buffer
│   ├── tui/
│   │   ├── mod.rs         # TUI entry point, event loop
│   │   ├── app.rs         # App state machine
│   │   ├── screens/
│   │   │   ├── presets.rs # Screen 1
│   │   │   └── visualizer.rs # Screen 2
│   │   ├── widgets/
│   │   │   ├── spectrum.rs # FFT + BarChart wrapper
│   │   │   └── volume.rs   # Volume bar widget
│   │   └── client.rs      # IPC client (socket reads/writes)
│   └── config.rs          # Config struct, load/save
└── tests/
    ├── ipc_protocol.rs
    └── noise_source.rs
```

### Runtime

```
~/.config/woosh/
└── config.toml

~/.local/share/woosh/
├── woosh.pid
├── woosh.sock
└── woosh.log
```

---

## Configuration

`~/.config/woosh/config.toml`:

```toml
[audio]
sample_rate = 44100
channels = 2        # 1 = mono, 2 = stereo

[defaults]
preset = "white"
volume = 0.8        # 0.0 – 1.0

[eq]
enabled = false
low_shelf_gain  = 0.0   # dB
peak1_gain      = 0.0
peak2_gain      = 0.0
high_shelf_gain = 0.0

[daemon]
log_level = "info"   # error | warn | info | debug | trace
```

On first run, if the file is absent, woosh writes the defaults above.

---

## Error Handling

- All recoverable errors use `anyhow::Result`.
- Audio device errors (e.g., device disconnected) are logged and trigger a 5-second retry loop in the daemon; the TUI shows a status banner.
- Socket connection failures in the TUI trigger an automatic reconnect with exponential backoff (100 ms, 200 ms, 400 ms, max 5 s).
- Unknown IPC commands return `ERROR unknown command` and are otherwise ignored.

---

## Platform Support

| Platform | Status |
|----------|--------|
| macOS (Apple Silicon, Intel) | Primary target |
| Linux (x86_64, aarch64) | Supported |
| Windows | Not supported (Unix sockets, `daemonize`) |

`rodio` abstracts over CoreAudio (macOS) and ALSA/PulseAudio/PipeWire (Linux) transparently.
