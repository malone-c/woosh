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
- **`woosh {place}`** — quick command to play YouTube place sound (e.g., `woosh paris`), then exit. Any word that is not a subcommand is treated as a place.
- **`woosh place {name}`** — the same, for names that match a subcommand (e.g. `woosh place status`).
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
| Pink  | Paul Kellet's refined 7-state IIR filter | Equal energy per octave; natural, balanced | **✓ Default** |
| Brown | Integration of white noise (random walk) | High bass content; deep rumble | ✓ |

**Default is pink noise** (changed from white in Phase 3.5): the TUI's preset list starts on `[defaults] preset`, which defaults to `pink`. All three types are fully implemented, and each is scaled to about 0.1 RMS so switching presets does not change loudness.

### Implementation

A custom struct implements `rodio::Source`:

```
struct NoiseSource {
    rng: SmallRng,              // rand crate, seeded from entropy
    dist: Uniform<f32>,         // [-1.0, 1.0]
    algorithm: NoiseAlgorithm,  // White | Pink { b: [f32; 7] } | Brown { last: f32 }
    fade: Fade,                 // fade-in / fade-out envelope (§ Audio Effects)
}

impl Iterator for NoiseSource { type Item = f32; … }
impl Source for NoiseSource { … }   // mono, 44 100 Hz, endless
```

Each `PLAY` creates a new `NoiseSource`, wraps it in an `EqProcessor`, and appends it to a new `rodio::Sink`. Volume is applied by the sink (`Sink::set_volume`). The `rodio::OutputStream` is opened lazily (§ Audio Daemon). An `Arc<Mutex<DaemonState>>` bridges the IPC handler tasks and the audio thread.

### Sample Rate

Fixed at 44 100 Hz, mono, for both channels. Not configurable.

---

## Place Sounds

### Overview

Woosh can stream ambient sounds from YouTube via the `mpv` media player with `yt-dlp` integration. Users type `woosh {place}` (e.g., `woosh tokyo`) and the daemon searches YouTube for "walking through {place}", takes the first result at least 10 minutes long, skips the first 5 minutes (to avoid intro music), and streams audio-only to a second audio channel.

### Architecture

Place sounds run in a **separate audio channel** from synthetic noise. The daemon spawns an `mpv` subprocess with these flags:

```bash
mpv "ytdl://ytsearch10:walking through {place}" \
    --ytdl-raw-options=match-filter=duration>=600 \
    --start=300 \
    --no-video --no-terminal --audio-display=no \
    --ytdl-format=bestaudio/best \
    --audio-format=s16 --audio-samplerate=44100 --audio-channels=mono \
    --ao=pcm --ao-pcm-waveheader=no --ao-pcm-file=/dev/stdout
```

- `ytdl://ytsearch10:` + `match-filter=duration>=600` — YouTube search via yt-dlp, keeping results of at least 10 minutes; mpv plays the first and continues with the next when it ends (mpv needs the `ytdl://` prefix to hand a search to yt-dlp)
- `--start=300` — skip first 5 minutes (300 seconds)
- `--audio-format=s16 --audio-samplerate=44100 --audio-channels=mono` — 16-bit signed mono PCM at 44.1 kHz, matching the synth channel
- `--ao=pcm … --ao-pcm-file=/dev/stdout` — raw PCM to stdout instead of the system audio device

A reader thread converts mpv's stdout to `f32` samples in a buffer that `MpvSource` (a `rodio::Source`) plays from, through the place EQ and sink volume. `--ao=pcm` decodes faster than real time, so the reader stops reading while 5 s of audio is buffered; mpv then blocks on the full pipe, which paces it to playback speed. While mpv is still buffering, the source plays silence. When the stream ends, the place channel returns to `stopped`.

### Single Place Rule

Only **one place sound can play at a time**. If the user requests a new place while another is playing, the daemon kills the old `mpv` process and spawns a new one.

### Mixing with Synthetic Noise

The user can play **one synthetic noise (white/pink/brown) + one place sound simultaneously**. Each channel has independent volume control but shares a unified EQ by default (see § EQ System).

### Reverb Effect (planned, ROADMAP 3.5.10)

Place sounds can optionally have reverb applied via `mpv`'s `--af=reverb` audio filter or post-processing in the daemon. Default: **off** (configurable in `config.toml`). Adds atmospheric depth to place sounds.

### Dependencies

- **mpv** — installed on user's system, must be in `$PATH`
- **yt-dlp** — required for YouTube search and streaming; mpv calls this automatically if installed

If `mpv` or `yt-dlp` is not on the daemon's `PATH`, `PLAY_PLACE` replies `ERROR mpv not found in PATH; …` (or `yt-dlp`), and synth-only mode remains available.

---

## Audio Effects

### Fade-In

All audio sources (synth noise and place sounds) **fade in over 1.5 seconds** when playback starts. This prevents sudden loud bursts that can be jarring.

**Implementation:** `NoiseSource` and `MpvSource` each hold a `Fade` envelope with an `f32` fade-in counter (0 → `FADE_SAMPLES` = 66,150). Each sample is multiplied by `counter / 66_150.0` and the counter increments by 1 (capped at 66,150). Because sources are recreated on every `PLAY`/`PLAY_PLACE` command, the counter resets automatically. `MpvSource` only advances it on real audio, so the fade starts when mpv's audio does.

### Fade-Out

When playback stops (via `STOP` or `STOP_PLACE`), audio **fades out over 1.5 seconds** before the source is silenced. This avoids abrupt cuts.

**Implementation:** each `Fade` holds a shared `Arc<AtomicBool>` fade-out flag and an `f32` fade-out counter. When the audio thread receives `STOP`/`STOP_PLACE` (or shuts down), it sets the flag via the shared Arc. Subsequent samples are then multiplied by a linearly decreasing envelope (`1.0 − fade_out_samples / 66_150.0`). Once `fade_out_samples` reaches 66,150 the source returns `None`, signalling end-of-stream to rodio; the sink drains naturally without an explicit pause or drop.

### Volume Defaults

| Channel | Default Volume | Notes |
|---------|----------------|-------|
| Synth   | 0.8            | `[defaults] volume`; ROADMAP 3.5.10 proposes 0.5 |
| Place   | 0.4            | Slightly quieter than synth for balanced mixing |

Users can adjust volumes independently via IPC commands or TUI controls.

### Reverb (Optional, planned)

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

- Own the `rodio::OutputStream` and one `Sink` per playing channel (synth + place).
- Run the synthesis loop for synthetic noise (via the `rodio::Source` implementation).
- Manage `mpv` subprocess for YouTube place sounds (spawn, detect end of stream, kill).
- Listen on the Unix socket and handle IPC commands for both channels.
- Write and clean up the PID, socket and ready files.

### Lifecycle

1. On first `woosh` invocation, the CLI checks for `woosh.pid`.
2. If PID file absent or stale (process not alive), it spawns `woosh daemon`, which detaches using `daemonize`.
3. Wait up to 500 ms for both the socket and `woosh.ready` (written once the socket accepts connections), then connect.
4. Daemon starts in a **stopped state** — no audio plays until the first `PLAY` command is received.
5. The `rodio::OutputStream` (audio device claim) is opened **lazily** on the first `PLAY` or `PLAY_PLACE` command and released automatically once both channels have stopped and their fade-outs have drained (or the place stream has ended). This allows Bluetooth headphones to hand off to other apps while woosh is idle. If the device cannot be opened, the daemon keeps the request, retries every 5 s, and reports `device=unavailable` in `STATUS`.
6. When both channels have been stopped and no clients are connected for `idle_timeout_mins` minutes (configurable, default 15), the daemon shuts down as for `QUIT`. An open TUI holds a connection, so it prevents this. Set `idle_timeout_mins = 0` to disable auto-shutdown.
7. On `QUIT`, `SIGTERM` or idle timeout: remove the socket, ready and PID files, fade out both channels (up to 2 s), kill mpv, exit.

### PID & Socket Paths

All runtime files live in the data directory from `dirs::data_dir()`:

| Platform | Directory |
|----------|-----------|
| macOS | `~/Library/Application Support/woosh/` |
| Linux | `${XDG_DATA_HOME:-~/.local/share}/woosh/` |

Files: `woosh.pid`, `woosh.sock`, `woosh.ready`, `woosh.log`.

### Daemonize

Uses the `daemonize` crate:

- Double-fork to detach from the controlling terminal.
- Append stdout/stderr to `woosh.log`.
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
| `SAVE_PRESET <name>` | `SAVE_PRESET focus` | Save current state (both channels) as a named preset (Phase 4, not built) |
| `DELETE_PRESET <name>` | `DELETE_PRESET focus` | Remove a named preset (Phase 4, not built) |

Commands are case-insensitive. `QUIT` gets no reply; the daemon closes the connection.

### Daemon → Client

| Response | Example | Description |
|----------|---------|-------------|
| `OK` | `OK` | Command accepted |
| `ERROR <msg>` | `ERROR unknown preset` | Command rejected with reason |
| `STATUS <fields>` | `STATUS synth=pink:running:0.50 place=paris:running:0.40 device=ok` | State snapshot: `<preset or none>:<running\|stopped>:<volume>` per channel, and `device=ok\|unavailable` |
| `PLACE_STATUS <fields>` | `PLACE_STATUS place=tokyo:running:0.40` | Place channel status |
| `EQ <values>` | `EQ 0.0 0.0 -3.0 0.0 0.0 0.0 0.0 0.0 0.0 0.0` | Synth EQ response (10 bands) |
| `PLACE_EQ <values>` | `PLACE_EQ 0.0 0.0 -3.0 0.0 0.0 0.0 0.0 0.0 0.0 0.0` | Place EQ response (10 bands) |

The TUI keeps one connection open and sends `STATUS` and `GET_EQ` every tick (33 ms). Every command, including volume and EQ changes, waits for its one-line reply.

### Concurrency

The daemon spawns one Tokio task per accepted client connection. A shared `Arc<Mutex<DaemonState>>` protects mutable state (per-channel play state, volume and EQ gains; current preset and place; device availability). The IPC handler records each request; the audio thread records each outcome.

---

## TUI

### Technology

- **`ratatui`** — terminal UI framework
- **`crossterm`** — cross-platform terminal backend (raw mode, events)
- **`tokio`** — async runtime; `tokio::select!` multiplexes crossterm events, socket reads, and a 33 ms tick timer (~30 fps)

### Interface Style

The TUI features a **centered, colorful layout with ASCII art**:

- **Centered frame:** Main content area (max 80 cols × 24 rows) with colored borders and padding
- **Logo:** "ＷＯＯＳＨ" wordmark in fullwidth letters in the title bar
- **Dithered backgrounds:** shade characters (`░▒`) in a diagonal pattern fill the margins
- **Color palette:** warm greys and creams via `Color::Rgb`, with a cyan border
- **Border style:** Cyan borders around main content frame

### Screens

The mockups show the full design; parts marked planned are not built yet.

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

- `↑`/`↓` (or `j`/`k`) navigate the list, which starts on `[defaults] preset`.
- `Enter` or `Space` sends `PLAY <preset>` to daemon and opens the Equalizer.
- `e` or `Tab` switches to Equalizer (Screen 2).
- `l` opens Place Selector (Screen 4) — planned, ROADMAP 3.5.9.

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

- `←`/`→` navigate EQ bands, `↑`/`↓` adjust gain ±1 dB (vim keys `h`/`l`/`k`/`j` also work).
- `r` resets all bands to 0 dB (flat).
- `s` toggles play/stop, resuming the last preset.
- `[`/`]` adjust synth volume ±5 %.
- `p`, `Esc`, `Tab` or `Backspace` returns to Preset Selector.
- Selected band highlighted in yellow.

#### Screen 3 — Spectrum Analyzer (removed in ROADMAP 3.5.8)

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

#### Screen 4 — Place Selector (planned, ROADMAP 3.5.9)

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

**Note:** Equalizer (Screen 2) is the landing screen after preset selection. The spectrum analyzer (Screen 3) was removed.

### Mouse Support

> **Phase 4 feature — not in MVP.**

- Click to select a preset from the list.
- Click-and-drag on the volume bar to set volume.
- Mouse capture can be disabled in `config.toml` (`[tui] mouse = false`).

### Key Bindings (global)

| Key | Action | Status |
|-----|--------|--------|
| `q` | Quit TUI (daemon keeps running) | ✓ |
| `Q` (shift) | Quit TUI and stop daemon (fades out both channels, kills mpv) | ✓ |
| `p` / `Esc` / `Tab` | Go to Preset Selector (from Equalizer) | ✓ |
| `e` / `Tab` | Go to Equalizer (from Presets) | ✓ |
| `s` | Toggle play/stop (Equalizer screen) | ✓ |
| `l` | Go to Place Selector | planned (3.5.9) |
| `←` / `→` | Select band (EQ screen) | ✓ |
| `←` / `→` | Synth volume down / up (Preset screen) | planned (3.5.9) |
| `[` / `]` | Synth volume down / up (EQ screen, ±5 % per press) | ✓ |
| `Shift+←` / `Shift+→` | Place volume down / up (Place Selector) | planned (3.5.9) |
| `↑` / `↓` | Navigate list (Preset screen); adjust EQ gain ±1 dB (EQ screen) | ✓ |
| `Enter` / `Space` | Start playback of the highlighted preset | ✓ |
| `r` | Reset all EQ bands to 0 dB (flat) | ✓ |
| `?` | Toggle help overlay | planned (Phase 4) |

The status bar shows whether the TUI is connected to the daemon (it reconnects automatically) and whether the audio device is unavailable.

---

## Spectrum Analyzer

Removed in ROADMAP 3.5.8: the Equalizer replaced it as the main screen, and the sample-streaming pipeline (`SUBSCRIBE_SAMPLES`, `ringbuf`, `spectrum-analyzer`) was deleted.

---

## EQ System

### Overview

Woosh features **two independent 10-band graphic EQs**: one for synthetic noise, one for place sounds. The TUI keeps them **in sync** — adjusting EQ sends both `SET_EQ` and `SET_PLACE_EQ`. Over IPC each can be set independently. (`advanced_eq_mode` for independent control in the TUI is planned.)

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

`EqProcessor<S>` wraps a `rodio::Source` (`NoiseSource` or `MpvSource`) and applies all 10 filters in series per sample. Each channel's gains live in an `Arc<Mutex<[f32; 10]>>` that the audio thread updates on `SET_EQ`/`SET_PLACE_EQ`; `EqProcessor` polls for updates every 512 samples using `try_lock`, so the audio callback never blocks. Filter state is never reset on coefficient changes — new coefficients take effect smoothly within ~1 buffer (~12 ms), avoiding audible clicks.

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

Key bindings: `←`/`→` navigate bands, `↑`/`↓` adjust ±1 dB, `r` resets all to flat, `[`/`]` adjust synth volume ±5 %, `s` toggles play/stop, `Esc` or `p` returns to the Preset Selector.

Adjusting EQ updates both synth and place channels simultaneously. A planned `advanced_eq_mode = true` would show two separate EQ displays (synth and place) with a toggle to switch between them.

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
| `serde` | 1 | Config serialization |
| `toml` | 0.8 | Config file parsing |
| `dirs` | 5 | XDG-aware path resolution |
| `anyhow` | 1 | Error handling |
| `tracing` | 0.1 | Structured logging (daemon) |
| `tracing-subscriber` | 0.3 | Log output (daemon: file; CLI/TUI: stderr), `RUST_LOG` filter |
| `libc` | 0.2 | `kill(pid, 0)` daemon liveness check |
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
├── AGENTS.md              # Agent runbook (plus src/daemon/ and src/tui/)
├── docs/logging.md
├── src/
│   ├── main.rs            # CLI entry point, subcommand dispatch, daemon auto-spawn
│   ├── lib.rs
│   ├── config.rs          # Config struct, load / write defaults
│   ├── daemon/
│   │   ├── mod.rs         # Daemon entry point, shutdown
│   │   ├── lifecycle.rs   # Paths, daemonize, PID, logging
│   │   ├── state.rs       # DaemonState, Channel, NoisePreset
│   │   ├── ipc.rs         # Unix socket listener, command handler, idle watchdog
│   │   ├── audio.rs       # Audio thread, NoiseSource, Fade
│   │   ├── eq.rs          # Biquad filters, EqProcessor
│   │   └── mpv.rs         # MpvSource (place sounds)
│   └── tui/
│       ├── mod.rs         # TUI entry point, event loop, key handling, rendering
│       ├── app.rs         # App state
│       ├── client.rs      # IPC client (persistent connection, reconnect)
│       └── art.rs         # Logo, dithered background
└── tests/
    ├── ipc_integration.rs
    └── daemon_startup_race_test.rs
```

### Runtime

```
config:  ~/Library/Application Support/woosh/config.toml   (macOS)
         ${XDG_CONFIG_HOME:-~/.config}/woosh/config.toml   (Linux)

data:    ~/Library/Application Support/woosh/              (macOS)
         ${XDG_DATA_HOME:-~/.local/share}/woosh/           (Linux)
         ├── woosh.pid
         ├── woosh.sock
         ├── woosh.ready
         └── woosh.log
```

---

## Configuration

`config.toml` (path in § File & Directory Layout). Implemented keys:

```toml
[defaults]
preset = "pink"           # preset highlighted when the TUI opens
volume = 0.8              # synth volume the daemon starts with (0.0 – 1.0)

[daemon]
idle_timeout_mins = 15    # auto-shutdown after N minutes idle (0 = never)
```

On first run, if the file is absent, woosh writes these defaults. Missing sections or keys take their defaults, and unknown keys are ignored. If the file cannot be parsed, woosh logs a warning and uses all defaults.

Planned keys (not read yet):

```toml
[place]                   # ROADMAP 3.5.10
enabled = true
volume = 0.4
reverb = false
reverb_amount = 0.3

[eq]
advanced_eq_mode = false  # control synth and place EQs independently

[tui]                     # ROADMAP 3.5.10
default_screen = "equalizer"
show_ascii_art = true
color_scheme = "default"

[daemon]
log_level = "info"        # today, set RUST_LOG instead
```

---

## Error Handling

- All recoverable errors use `anyhow::Result`.
- If the audio device cannot be opened, the error is logged and the daemon retries every 5 seconds; `STATUS` reports `device=unavailable` and the TUI shows a status banner.
- Socket connection failures in the TUI trigger an automatic reconnect with exponential backoff (100 ms, 200 ms, 400 ms, max 5 s); the status bar shows "disconnected" meanwhile and the TUI keeps running.
- Unknown IPC commands return `ERROR unknown command: <VERB>` and are otherwise ignored. Invalid arguments (unknown preset, NaN volume, bad EQ band) return `ERROR <reason>`.
- CLI play commands print the daemon's `ERROR` reply and exit non-zero.

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
