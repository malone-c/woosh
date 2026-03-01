# AGENTS.md — src/daemon/

Daemon internals for woosh. Root AGENTS.md covers project-wide conventions; this file covers only what lives under src/daemon/.

## OVERVIEW

Seven modules implement the daemon: orchestration, lifecycle, state, IPC server, audio thread, EQ DSP, and mpv place-audio stub. See WHERE TO LOOK for routing.

## WHERE TO LOOK

| File          | Lines | Responsibility                                                              |
|---------------|-------|-----------------------------------------------------------------------------|
| mod.rs        | 79    | Orchestrator: creates shared Arcs, spawns audio thread, runs tokio runtime  |
| lifecycle.rs  | 140   | PID/socket/ready-file paths, daemonize(), daemon_is_alive(), init_logging() |
| state.rs      | 83    | NoisePreset (White/Pink/Brown), PlayState, DaemonState struct               |
| ipc.rs        | 328   | UnixListener, dispatch() command router, broadcast_task() every 33ms        |
| audio.rs      | 415   | spawn_audio_thread(), NoiseSource generators, AudioCommand enum             |
| eq.rs         | 201   | BiquadCoeffs, BiquadState, peaking_coeffs(), apply_biquad(), EqProcessor   |
| mpv.rs        | 174   | MpvSource: mpv subprocess → PCM → VecDeque → rodio Source                  |

## SHARED STATE

Four shared objects wired at startup in mod.rs and passed into IPC + audio:

- `Arc<Mutex<DaemonState>>` — all IPC state reads/writes (preset, volume, play state)
- `Arc<Mutex<Vec<f32>>>` — sample_buf: NoiseSource batches 512 samples → broadcast_task drains every 33ms → `"SAMPLES <hex>\n"` to subscribers
- `Arc<Mutex<[f32; 10]>>` eq_arc — synth EQ gains; read by EqProcessor every 512 samples
- `Arc<Mutex<[f32; 10]>>` place_eq_arc — place-audio EQ gains; separate from eq_arc
- `mpsc::Sender<AudioCommand>` — IPC thread → audio thread; never block on send

## AUDIO THREAD

- Spawned with `std::thread`, NOT tokio (rodio `OutputStream` is `!Send`).
- `NoiseSource` implements `rodio::Source`; generators: white = uniform RNG, pink = 7-state Paul Kellet IIR, brown = first-order integration.
- Every 512 samples, NoiseSource calls `try_lock` on sample_buf; skips on contention — never blocks.
- `AudioCommand` variants: `Play`, `Stop`, `SetVolume`, `SetEq`, `PlayPlace`, `StopPlace`, `SetPlaceVolume`, `Shutdown`.
- EqProcessor wraps NoiseSource in the rodio sink; also polls eq_gains via `try_lock` every 512 samples.
- **Fade-in:** Both `NoiseSource` and `MpvSource` ramp from 0 → 1.0 over 66,150 samples (1.5 s at 44,100 Hz) via `fade_samples` counter. Resets automatically when sources are recreated on `Play`/`PlayPlace`.
- **Fade-out:** Each source holds a shared `Arc<AtomicBool>` (`fade_out`) and a `fade_out_samples: u32` counter. `Stop` sets the noise flag; `StopPlace` sets the place flag. The source then ramps 1.0 → 0.0 over 66,150 samples and returns `None` — rodio drains the sink naturally. `Play`/`PlayPlace` create a fresh `Arc<AtomicBool>`, so the old fade-out is abandoned when the sink is dropped.

## EQ SUBSYSTEM (eq.rs)

- `N_BANDS = 10`; `BAND_FREQS = [31, 63, 125, 250, 500, 1k, 2k, 4k, 8k, 16k]` Hz; `EQ_Q = √2`; gain ±12 dB.
- `peaking_coeffs()` follows Audio EQ Cookbook (peaking EQ filter).
- `apply_biquad()` uses Direct Form II Transposed.
- `BiquadCoeffs` is `Copy`; `BiquadState` is `Default + Copy`.
- Coefficients recompute every 512 samples. State is NOT reset on recompute — avoids audible clicks.

## IPC PROTOCOL

Wire format: newline-delimited UTF-8 over Unix socket at `~/.local/share/woosh/woosh.sock`.

Key commands handled in `dispatch()`:

| Command                  | Response                        |
|--------------------------|---------------------------------|
| `PLAY`                   | `"OK\n"`                        |
| `STOP`                   | `"OK\n"`                        |
| `SET_VOLUME <f32>`       | `"OK\n"`                        |
| `STATUS`                 | `"STATUS synth=p:s:v place=p:s:v\n"`    |
| `SET_EQ <band> <gain>`   | `"OK\n"`                        |
| `GET_EQ`                 | `"EQ v0 v1 ... v9\n"`          |
| `SUBSCRIBE_SAMPLES`      | switches conn to push-only mode |
| `QUIT`                   | calls `process::exit(0)`        |

Ready file (`~/.local/share/woosh/woosh.ready`) is written by ipc.rs after socket bind; TUI polls both socket + ready file before connecting.

`dispatch()` returns `Option<String>`; QUIT never returns — `#[allow(clippy::unnecessary_wraps)]` suppresses the lint.

## ANTI-PATTERNS

- Never use tokio for the audio thread — `rodio::OutputStream` is `!Send`.
- Never `unwrap()` on `try_lock` in the audio hot path — skip on contention.
- Never reset `BiquadState` when EQ coefficients change — causes audible clicks.
- Never conflate eq_arc (synth) with place_eq_arc (place audio) — they are separate Arcs.
- Never call `sink.pause()` on `Stop`/`StopPlace` — set the `fade_out` flag instead and let the source ramp down.
