# Woosh — Product Specification

> A terminal ambient noise generator with a daemon/TUI architecture featuring synthetic noise (white/pink/brown), YouTube place sounds, EQ controls, and a colorful ASCII art interface.

---

## Table of Contents

1. [Overview](#overview)
2. [Architecture](#architecture)
3. [Noise Generation](#noise-generation)
4. [Place Sounds](#place-sounds)
5. [Audio Effects](#audio-effects)
6. [Audio Daemon](#audio-daemon)
7. [IPC Protocol](#ipc-protocol)
8. [TUI](#tui)
9. [Spectrum Analyzer](#spectrum-analyzer)
10. [EQ System](#eq-system)
11. [Crate Inventory](#crate-inventory)
12. [File & Directory Layout](#file--directory-layout)
13. [Configuration](#configuration)
14. [Error Handling](#error-handling)
15. [Platform Support](#platform-support)

---

## Overview

Woosh is a command-line ambient noise app written in Rust. It plays continuous synthetic noise (white, pink, brown) and YouTube-sourced place sounds (e.g., "walking through Paris") through the system audio output. A persistent background daemon owns the audio device and synthesis pipeline; a lightweight TUI client connects to the daemon over a Unix domain socket to control playback and display a colorful, ASCII art-enhanced interface with EQ controls.

### Goals

- **Dual audio channels** — play synthetic noise and YouTube place sounds simultaneously, each with independent volume and EQ.
- **No sample files** — all synthetic noise is generated algorithmically at runtime.
- **YouTube integration** — stream ambient place sounds via mpv + yt-dlp without pre-downloading.
- **Low resource usage** — the daemon should idle at < 1 % CPU when playing synthetic noise (place sounds depend on mpv overhead).
- **Interactive TUI** — colorful, centered interface with ASCII art, real-time EQ control, volume sliders, and preset switching.
- **Daemon persistence** — close the TUI and audio keeps playing; reopen TUI and it reconnects instantly.

### Non-Goals

- GUI / native app
- Multi-output routing or per-app audio mixing
- Offline place sound caching (streams directly from YouTube)

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         woosh binary                            │
│                                                                 │
│  ┌──────────────┐        ┌───────────────────────────────────┐ │
│  │  TUI Client  │◄──────►│      Audio Daemon                 │ │
│  │  (ratatui)   │ Unix   │                                   │ │
│  │  crossterm   │ socket │  ┌─────────────┐  ┌────────────┐ │ │
│  │  + ASCII art │        │  │ Synth Sink  │  │ Place Sink │ │ │
│  └──────────────┘        │  │ (rodio)     │  │ (mpv PCM)  │ │ │
│                          │  │ NoiseSource │  │ YouTube    │ │ │
│                          │  │ + EQ        │  │ + EQ       │ │ │
│                          │  └─────────────┘  └────────────┘ │ │
│                          │         │               │         │ │
│                          │         └───────┬───────┘         │ │
│                          │            Audio Device           │ │
│                          └───────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

The single compiled binary (`woosh`) serves both roles:

- **`woosh`** (no subcommand) — launches the TUI in a stopped state. If no daemon is running, it spawns one in the background first. No noise plays until the user selects a preset and presses `Enter`.
- **`woosh pink|white|brown`** — quick command to play specific synthetic noise type without TUI, then exit.
- **`woosh {place}`** — quick command to play YouTube place sound (e.g., `woosh paris`), then exit.
- **`woosh daemon`** — starts the daemon in the foreground (used internally by the auto-spawn path).
- **`woosh stop`** — sends `QUIT` to the daemon and exits.
- **`woosh status`** — prints current daemon state to stdout and exits.

The daemon manages two independent audio channels that can play simultaneously: **synth** (algorithmic noise generation) and **place** (YouTube streams via mpv).

---

## Noise Generation

### Strategy

Noise is synthesised algorithmically. No WAV/OGG files are bundled or loaded.

### Noise Types

| Type  | Algorithm | Description | Status |
|-------|-----------|-------------|--------|
| White | PRNG — uniform distribution over `[-1.0, 1.0]` | Equal energy per Hz; sounds bright/hissy | ✓ |
| Pink  | Paul Kellet's 3-pole IIR approximation | Equal energy per octave; natural, balanced | **✓ Default** |
| Brown | Integration of white noise (random walk) | High bass content; deep rumble | ✓ |

**Default is pink noise** (changed from white in Phase 3.5). All three types are fully implemented.

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

## Place Sounds

### Overview

Woosh can stream ambient sounds from YouTube via the `mpv` media player with `yt-dlp` integration. Users type `woosh {place}` (e.g., `woosh tokyo`) and the daemon searches YouTube for "walking through {place}", picks a random video from results, skips the first 5 minutes (to avoid intro music), and streams audio-only to a second audio channel.

### Architecture

Place sounds run in a **separate audio channel** from synthetic noise. The daemon spawns an `mpv` subprocess with these flags:

```bash
mpv "ytsearch1:walking through {place}" \
    --no-video \
    --audio-display=no \
    --ytdl \
    --start=300 \
    --af=aformat=s16:44100 \
    --ao=pcm:file=/dev/stdout
```

- `ytsearch1:` — YouTube search, pick first result (randomization handled by query variation)
- `--start=300` — skip first 5 minutes (300 seconds)
- `--ao=pcm:file=/dev/stdout` — output raw PCM to stdout instead of system audio device
- `--af=aformat=s16:44100` — force 16-bit signed PCM at 44.1 kHz to match synth sample rate

The daemon captures stdout via `BufReader<ChildStdout>`, wraps it as a `rodio::Source`, applies EQ and volume, and appends it to the `place_sink`.

### Single Place Rule

Only **one place sound can play at a time**. If the user requests a new place while another is playing, the daemon kills the old `mpv` process and spawns a new one.

### Mixing with Synthetic Noise

The user can play **one synthetic noise (white/pink/brown) + one place sound simultaneously**. Each channel has independent volume control but shares a unified EQ by default (see § EQ System).

### Reverb Effect

Place sounds can optionally have reverb applied via `mpv`'s `--af=reverb` audio filter or post-processing in the daemon. Default: **off** (configurable in `config.toml`). Adds atmospheric depth to place sounds.

### Dependencies

- **mpv** — installed on user's system, must be in `$PATH`
- **yt-dlp** — required for YouTube search and streaming; mpv calls this automatically if installed

If `mpv` or `yt-dlp` is missing, place sound commands gracefully fail with an error message and synth-only mode remains available.

---

## Audio Effects

### Fade-In

All audio sources (synth noise and place sounds) **fade in over 1.5 seconds** when playback starts. This prevents sudden loud bursts that can be jarring.

**Implementation:** `NoiseSource` and `MpvSource` each hold a `fade_samples: u32` counter (0 → 66,150). Each sample is multiplied by `min(fade_samples / 66_150.0, 1.0)` and the counter increments by 1 (capped at 66,150). Because sources are recreated on every `PLAY`/`PLAY_PLACE` command, the counter resets automatically.

### Fade-Out

When playback stops (via `STOP` or `STOP_PLACE`), audio **fades out over 1.5 seconds** before the source is silenced. This avoids abrupt cuts.

**Implementation:** `NoiseSource` and `MpvSource` each hold a shared `fade_out: Arc<AtomicBool>` flag and a `fade_out_samples: u32` counter. When the audio thread receives `STOP`/`STOP_PLACE`, it sets the flag via the shared Arc. Subsequent samples are then multiplied by a linearly decreasing envelope (`1.0 − fade_out_samples / 66_150.0`). Once `fade_out_samples` reaches 66,150 the source returns `None`, signalling end-of-stream to rodio; the sink drains naturally without an explicit pause or drop.

### Volume Defaults

| Channel | Default Volume | Notes |
|---------|----------------|-------|
| Synth   | 0.5            | Reduced from 0.8 to avoid excessive loudness |
| Place   | 0.4            | Slightly quieter than synth for balanced mixing |

Users can adjust volumes independently via IPC commands or TUI controls.

### Reverb (Optional)

Place sounds can have reverb effect applied for atmospheric ambience. Configured via `~/.config/woosh/config.toml`:

```toml
[place]
reverb = false      # default: off
reverb_amount = 0.3 # 0.0 – 1.0, only used if reverb = true
```

Reverb adds CPU overhead (mpv subprocess filter chain), so it defaults to disabled.

---

## Audio Daemon

### Responsibilities

- Own the `rodio::OutputStream` and two `Sink` instances (synth + place).
- Run the synthesis loop for synthetic noise (via the `rodio::Source` implementation).
- Manage `mpv` subprocess for YouTube place sounds (spawn, monitor, kill).
- Listen on the Unix socket and handle IPC commands for both channels.
- Maintain fixed-size ring buffers of recent PCM samples for the visualizer (one per channel).
- Write and clean up the PID file.

### Lifecycle

1. On first `woosh` invocation, the TUI checks for `woosh.pid`.
2. If PID file absent or stale (process not alive), spawn `woosh daemon` as a detached child using `daemonize`.
3. Wait up to 500 ms for the socket file to appear, then connect.
4. Daemon starts in a **stopped state** — no audio plays until the first `PLAY` command is received.
5. The `rodio::OutputStream` (audio device claim) is opened **lazily** on the first `PLAY` or `PLAY_PLACE` command and released automatically once both channels have stopped and their fade-outs have drained. This allows Bluetooth headphones to hand off to other apps while woosh is idle.
6. When both channels have been stopped and no clients are connected for `idle_timeout_mins` minutes (configurable, default 15), the daemon auto-shuts down. Set `idle_timeout_mins = 0` to disable auto-shutdown.
7. On `QUIT` command or `SIGTERM`, flush audio, remove socket and PID file, exit.

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
| **Synth Channel** |
| `PLAY <preset>` | `PLAY pink` | Start or switch to a noise preset (white/pink/brown) |
| `STOP` | `STOP` | Pause synth audio output |
| `SET_VOLUME <f32>` | `SET_VOLUME 0.5` | Set synth volume, clamped to `[0.0, 1.0]` |
| `SET_EQ <band> <gain_db>` | `SET_EQ 5 3.0` | Adjust synth EQ band by index (0–9); gain clamped to ±12 dB |
| `GET_EQ` | `GET_EQ` | Query synth EQ (10 band gains) |
| **Place Channel** |
| `PLAY_PLACE <location>` | `PLAY_PLACE paris` | Start YouTube place sound (kills old place if playing) |
| `STOP_PLACE` | `STOP_PLACE` | Stop place sound (kill mpv subprocess) |
| `SET_PLACE_VOLUME <f32>` | `SET_PLACE_VOLUME 0.4` | Set place volume, clamped to `[0.0, 1.0]` |
| `SET_PLACE_EQ <band> <gain_db>` | `SET_PLACE_EQ 2 -3.0` | Adjust place EQ band by index (0–9); gain clamped to ±12 dB |
| `GET_PLACE_EQ` | `GET_PLACE_EQ` | Query place EQ (10 band gains) |
| `GET_PLACE_STATUS` | `GET_PLACE_STATUS` | Query place state (playing, location, volume) |
| **Global** |
| `STATUS` | `STATUS` | Request current state of both channels |
| `QUIT` | `QUIT` | Shut down daemon (stops both channels, kills mpv) |
| `SAVE_PRESET <name>` | `SAVE_PRESET focus` | Save current state (both channels) as a named preset |
| `DELETE_PRESET <name>` | `DELETE_PRESET focus` | Remove a named preset |

### Daemon → Client

| Response | Example | Description |
|----------|---------|-------------|
| `OK` | `OK` | Command accepted |
| `ERROR <msg>` | `ERROR unknown preset` | Command rejected with reason |
| `STATUS <fields>` | `STATUS synth=pink:playing:0.5 place=paris:playing:0.4` | State snapshot (both channels) |
| `PLACE_STATUS <fields>` | `PLACE_STATUS playing location=tokyo volume=0.4` | Place channel status |
| `SAMPLES <hex>` | `SAMPLES 3f800000...` | Batch of PCM samples (little-endian f32 hex) for visualizer |
| `EQ <values>` | `EQ 0.0 0.0 -3.0 0.0 0.0 0.0 0.0 0.0 0.0 0.0` | Synth EQ response (10 bands) |
| `PLACE_EQ <values>` | `PLACE_EQ 0.0 0.0 -3.0 0.0 0.0 0.0 0.0 0.0 0.0 0.0` | Place EQ response (10 bands) |

The TUI polls `STATUS` every tick and subscribes to unsolicited `SAMPLES` pushes after sending a `SUBSCRIBE_SAMPLES` command (added in Phase 2).

### Concurrency

The daemon spawns one Tokio task per accepted client connection. A shared `Arc<Mutex<DaemonState>>` protects mutable state (current preset, volume, EQ params, sample ring buffer).

---

## TUI

### Technology

- **`ratatui`** — terminal UI framework
- **`crossterm`** — cross-platform terminal backend (raw mode, events)
- **`tokio`** — async runtime; `tokio::select!` multiplexes crossterm events, socket reads, and a 33 ms tick timer (~30 fps)

### Interface Style

The TUI features a **centered, colorful layout with ASCII art**:

- **Centered frame:** Main content area (max 80 cols × 24 rows) with colored borders and padding
- **ASCII art logo:** "WOOSH" banner in figlet font displayed in title bar
- **Dithered backgrounds:** Box-drawing characters (`░▒▓█`) create ambient wave patterns in margins
- **Color palette:** Blues, purples, pinks via `Color::Rgb` for aesthetic, calming vibes
- **Border style:** Cyan borders around main content frame

### Screens

#### Screen 1 — Preset Selector

```
       ╔═══════════════════════════════════════════════╗
       ║   ██╗    ██╗ ██████╗  ██████╗ ███████╗██╗  ██╗║
       ║   ██║    ██║██╔═══██╗██╔═══██╗██╔════╝██║  ██║║
       ║   ██║ █╗ ██║██║   ██║██║   ██║███████╗███████║║
       ║   ██║███╗██║██║   ██║██║   ██║╚════██║██╔══██║║
       ║   ╚███╔███╔╝╚██████╔╝╚██████╔╝███████║██║  ██║║
       ║    ╚══╝╚══╝  ╚═════╝  ╚═════╝ ╚══════╝╚═╝  ╚═╝║
       ╠═══════════════════════════════════════════════╣
       ║                                               ║
       ║   Synth:  ▶ Pink    ████████░░  50%          ║
       ║   Place:  ● Paris   ██████░░░░  40%          ║
       ║                                               ║
       ║   ▶ White Noise                               ║
       ║   ● Pink Noise      (default)                 ║
       ║     Brown Noise                               ║
       ║                                               ║
       ║   [↑/↓] select  [Enter] play  [l] locations  ║
       ║   [q] quit      [→] equalizer                ║
       ╚═══════════════════════════════════════════════╝
░░▒▒▓▓  Ambient waves pattern in margins  ▓▓▒▒░░
```

- Arrow keys navigate the list.
- `Enter` sends `PLAY <preset>` to daemon.
- `l` opens Place Selector (Screen 4).
- `→` or `Tab` switches to Equalizer (Screen 2, changed from Visualizer).

#### Screen 2 — Equalizer (Default Screen)

```
       ╔═══════════════════════════════════════════════╗
       ║  WOOSH ● Pink + Paris  Synth: 50%  Place: 40%║
       ╠═══════════════════════════════════════════════╣
       ║                  Equalizer                    ║
       ║                                               ║
       ║   ▂▄█▆▃▂▄▅▂▃                                  ║
       ║   31 63 125 250 500 1k 2k 4k 8k 16k  Hz       ║
       ║   [0] [0] [-3] [0] [0] [0] [0] [0] [0] [0] dB ║
       ║                                               ║
       ║   [←/→] select band  [↑/↓] adjust ±1 dB      ║
       ║   [r] reset  [s] spectrum  [p] presets        ║
       ║   [q] quit                                    ║
       ╚═══════════════════════════════════════════════╝
```

- `←`/`→` navigate EQ bands, `↑`/`↓` adjust gain ±1 dB.
- `r` resets all bands to 0 dB (flat).
- `s` toggles Spectrum Analyzer (Screen 3, optional feature).
- `p` returns to Preset Selector.
- Selected band highlighted in yellow.

#### Screen 3 — Spectrum Analyzer (Toggle Feature)

```
       ╔═══════════════════════════════════════════════╗
       ║  WOOSH ● Pink + Paris  Synth: 50%  Place: 40%║
       ╠═══════════════════════════════════════════════╣
       ║                                               ║
       ║   ▄  ▄▄  █  ▄▄  █▄  ▄▄  ▄  ▄▄  ▄  ▄           ║
       ║   █  ██  █  ██  ██  ██  █  ██  █  █           ║
       ║   █  ██  █  ██  ██  ██  █  ██  █  █           ║
       ║   ───────────────────────────────────          ║
       ║   63 125 250 500 1k 2k 4k 8k 16k  Hz          ║
       ║                                               ║
       ║   [s] back to eq  [p] presets  [q] quit      ║
       ╚═══════════════════════════════════════════════╝
```

- Spectrum bars update at ~30 fps.
- `s` returns to Equalizer screen.

#### Screen 4 — Place Selector

```
       ╔═══════════════════════════════════════════════╗
       ║               Place Sounds                    ║
       ╠═══════════════════════════════════════════════╣
       ║                                               ║
       ║   Currently playing: Paris ▶ 40%             ║
       ║                                               ║
       ║   Type location and press Enter:             ║
       ║   > tokyo_____________                        ║
       ║                                               ║
       ║   Recent places:                              ║
       ║   • Paris                                     ║
       ║   • Tokyo                                     ║
       ║   • New York                                  ║
       ║                                               ║
       ║   [Esc] back  [Shift+←/→] volume  [q] quit  ║
       ╚═══════════════════════════════════════════════╝
```

- Type location name and press `Enter` to play.
- `Shift+←`/`→` adjust place volume.
- `Esc` returns to Preset Selector.

**Note:** Equalizer (Screen 2) is now the **default landing screen** after preset selection. Spectrum analyzer (Screen 3) is an optional toggle feature accessed via `s` key.

### Mouse Support

> **Phase 4 feature — not in MVP.**

- Click to select a preset from the list.
- Click-and-drag on the volume bar to set volume.
- Mouse capture can be disabled in `config.toml` (`[tui] mouse = false`).

### Key Bindings (global)

| Key | Action |
|-----|--------|
| `q` | Quit TUI (daemon keeps running) |
| `Q` (shift) | Quit TUI and stop daemon (kills both channels + mpv) |
| `p` | Go to Preset Selector |
| `e` | Go to Equalizer (default screen) |
| `s` | Toggle Spectrum Analyzer (from Equalizer screen) |
| `l` | Go to Place Selector |
| `←` / `→` | Synth volume down / up (Preset screen); select band (EQ screen) |
| `[` / `]` | Synth volume down / up (EQ screen, ±5 % per press) |
| `Shift+←` / `Shift+→` | Place volume down / up (Place Selector) |
| `↑` / `↓` | Navigate list (Preset screen); adjust EQ gain ±1 dB (EQ screen) |
| `Enter` | Confirm selection / start playback |
| `r` | Reset all EQ bands to 0 dB (flat) |
| `Esc` | Return to previous screen |
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

### Overview

Woosh features **two independent 10-band graphic EQs**: one for synthetic noise, one for place sounds. By default, they **stay in sync** — adjusting EQ in the TUI updates both channels simultaneously. Advanced users can enable `advanced_eq_mode = true` in `config.toml` to control each EQ independently.

### Bands

10-band graphic EQ with peaking filters at standard octave frequencies:

| Band | Frequency | Filter type | Q | Gain range |
|------|-----------|-------------|---|------------|
| 0 | 31 Hz | Peaking EQ | √2 | −12 to +12 dB |
| 1 | 63 Hz | Peaking EQ | √2 | −12 to +12 dB |
| 2 | 125 Hz | Peaking EQ | √2 | −12 to +12 dB |
| 3 | 250 Hz | Peaking EQ | √2 | −12 to +12 dB |
| 4 | 500 Hz | Peaking EQ | √2 | −12 to +12 dB |
| 5 | 1 kHz | Peaking EQ | √2 | −12 to +12 dB |
| 6 | 2 kHz | Peaking EQ | √2 | −12 to +12 dB |
| 7 | 4 kHz | Peaking EQ | √2 | −12 to +12 dB |
| 8 | 8 kHz | Peaking EQ | √2 | −12 to +12 dB |
| 9 | 16 kHz | Peaking EQ | √2 | −12 to +12 dB |

Gain is adjustable in 1 dB steps. Default is 0 dB (flat) for all bands.

### Implementation

Implemented in `src/daemon/eq.rs`. Each band is a 2nd-order biquad peaking filter using the Audio EQ Cookbook coefficients, computed in the Direct Form II Transposed structure for numerical stability.

`EqProcessor<S>` wraps a `rodio::Source` (currently `NoiseSource`) and applies all 10 filters in series per sample. Gains are shared between the audio thread and the IPC handler via `Arc<Mutex<[f32; 10]>>`; `EqProcessor` polls for updates every 512 samples using `try_lock` (same pattern as the sample visualizer buffer). Filter state is never reset on coefficient changes — new coefficients take effect smoothly within ~1 buffer (~12 ms), avoiding audible clicks.

At exactly 0 dB, the identity filter `{b0:1, b1:0, b2:0, a1:0, a2:0}` is used for bit-perfect pass-through with no state accumulation.

### IPC Extension

```
# Synth EQ
SET_EQ <band_index> <gain_db>   # band 0–9; gain clamped to [−12, +12] dB
GET_EQ                          # response: "EQ v0 v1 v2 v3 v4 v5 v6 v7 v8 v9"

# Place EQ
SET_PLACE_EQ <band_index> <gain_db>   # band 0–9; gain clamped to [−12, +12] dB
GET_PLACE_EQ                          # response: "PLACE_EQ v0 v1 v2 v3 v4 v5 v6 v7 v8 v9"

# Sync control (future feature)
SYNC_EQ <bool>                        # enable/disable EQ sync mode
```

### TUI

The EQ screen (Screen 2) is now the **default landing screen** (changed from spectrum analyzer). It shows a `BarChart` of 10 bands (bar height maps ±12 dB → 0–24 range) with the selected band highlighted in yellow. A readout line beneath shows all gains numerically.

Key bindings: `←`/`→` navigate bands, `↑`/`↓` adjust ±1 dB, `r` resets all to flat, `[`/`]` adjust synth volume ±5 %, `s` toggles spectrum analyzer, `Esc` or `p` returns to the Preset Selector.

When **advanced_eq_mode = false** (default): Adjusting EQ updates both synth and place channels simultaneously.

When **advanced_eq_mode = true**: TUI shows two separate EQ displays (synth and place) with a toggle to switch between them.

> **Note:** EQ gains are not currently persisted to `config.toml` — they reset to flat (0 dB) when the daemon restarts. Persistence is planned for Phase 4.

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
preset = "pink"     # changed from "white"
volume = 0.5        # 0.0 – 1.0, reduced from 0.8

[place]
enabled = true
volume = 0.4        # default place sound volume
reverb = false      # reverb effect for place sounds
reverb_amount = 0.3 # 0.0 – 1.0, only used if reverb = true

[eq]
enabled = false
advanced_eq_mode = false  # if true, control synth and place EQs independently
low_shelf_gain  = 0.0     # dB
peak1_gain      = 0.0
peak2_gain      = 0.0
high_shelf_gain = 0.0

[tui]
default_screen = "equalizer"  # "equalizer" or "presets"
show_ascii_art = true
color_scheme = "default"      # default = blues/purples/pinks

[daemon]
log_level = "info"            # error | warn | info | debug | trace
idle_timeout_mins = 15        # auto-shutdown after N minutes idle (0 = never)
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

### External Dependencies

- **mpv** (with yt-dlp) — Required for place sounds feature. Must be in `$PATH`.
  - macOS: `brew install mpv yt-dlp`
  - Linux: `apt install mpv yt-dlp` or equivalent package manager

If `mpv` or `yt-dlp` is not installed, woosh runs in **synth-only mode** (synthetic noise still works, place sounds gracefully disabled with error messages).
