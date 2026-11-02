# AGENTS.md — src/daemon/

Daemon internals for woosh. Root AGENTS.md covers project-wide conventions; this file covers only what lives under src/daemon/.

## OVERVIEW

Seven modules implement the daemon: orchestration, lifecycle, state, IPC server, audio thread, EQ DSP, and the mpv place-sound source. See WHERE TO LOOK for routing.

## WHERE TO LOOK

| File          | Lines | Responsibility                                                              |
|---------------|-------|-----------------------------------------------------------------------------|
| mod.rs        | ~90   | `start()`: config, shared state, audio thread, tokio runtime, SIGTERM; one cleanup path for every exit |
| lifecycle.rs  | ~130  | PID/socket/ready/log paths, `daemonize()`, `daemon_is_alive()`, `init_logging()` (RUST_LOG) |
| state.rs      | ~140  | `NoisePreset` (`ALL`, `id()`, `label()`), `PlayState`, `ChannelId`, `Channel`, `DaemonState`, `lock()` |
| ipc.rs        | ~310  | `run_ipc_server()`, `dispatch()` + one handler per command shape, idle watchdog, place-tool check |
| audio.rs      | ~470  | `spawn_audio_thread()`, `AudioCommand`, `Fade`, `NoiseSource`, lazy device + retry |
| eq.rs         | ~210  | `BiquadCoeffs`, `BiquadState`, `peaking_coeffs()`, `apply_biquad()`, `EqProcessor`, `SharedGains` |
| mpv.rs        | ~140  | `MpvSource`: mpv subprocess → PCM chunks → bounded channel (backpressure) → rodio `Source` |

## SHARED STATE

- `Arc<Mutex<DaemonState>>` — created in mod.rs, shared by IPC and the audio thread. Holds `synth` and `place` (`Channel { play_state, volume, eq_gains }`), `preset`, `place_location`, `device_available`. Lock with `state::lock()`, which recovers from poisoning.
- **Who writes what:** the IPC handler records each request as it accepts it (so `STATUS` is current immediately); the audio thread then records the outcome of each command it processes: channel started, failed, stopped, or place stream ended. Commands are processed in order, so the last write always reflects what is playing. Volume and EQ gains are written only by IPC.
- `mpsc::Sender<AudioCommand>` — IPC → audio thread; never block on send.
- `SharedGains` (`Arc<Mutex<[f32; 10]>>`), one per channel — created inside the audio thread, written on `SetEq`, read by `EqProcessor` every 512 samples via `try_lock`.

## AUDIO THREAD

- Spawned with `std::thread`, NOT tokio (rodio `OutputStream` is `!Send`).
- `AudioCommand`: `Play(NoisePreset)`, `PlayPlace(String)`, `Stop(ChannelId)`, `SetVolume(ChannelId, f32)`, `SetEq(ChannelId, gains)`, `Shutdown`.
- Each playing channel is a `Voice { sink, fade_out }`. `start_voice()` builds the source, wraps it in `EqProcessor`, and appends it to a new sink.
- `NoiseSource` generators: white = uniform RNG, pink = 7-state Paul Kellet IIR, brown = first-order integration. All are scaled to ~0.1 RMS.
- **Device:** none at startup. `Play`/`PlayPlace` queue a pending request; the loop opens the device, then starts pending voices. If opening fails, it retries every 5 s and sets `device_available = false` (`STATUS … device=unavailable`). While the device is open, the loop polls every 50 ms (`recv_timeout`), drops finished voices, and releases the device once no voice is playing or pending. While idle, it blocks on `recv()`.
- **Place stream ending on its own** (mpv exited): the voice is dropped and the place channel is set to stopped, which lets the device be released and the idle watchdog fire.
- **Fades:** `Fade` (shared by `NoiseSource` and `MpvSource`) ramps 0 → 1 over `FADE_SAMPLES` (66,150 = 1.5 s at `SAMPLE_RATE`), and 1 → 0 once its `fade_out` flag is set, then the source returns `None`. `Play`/`PlayPlace` replace the old voice immediately (no fade).
- **Shutdown:** fades out both voices, waits up to 2 s for them to finish, then returns. Locals drop in reverse order, so sinks drop before the device.

## PLACE SOUNDS (mpv.rs)

- Args: `ytdl://ytsearch10:walking through {location}` (without `ytdl://`, mpv opens the query as a file and exits at once), `--ytdl-raw-options=match-filter=duration>=600` (the top result is often a short music video; `--start=300` past its end plays nothing), `--start=300`, `--audio-format=s16 --audio-samplerate=44100 --audio-channels=mono`, `--ao=pcm --ao-pcm-file=/dev/stdout --ao-pcm-waveheader=no`. Older `--af=aformat=format=…` syntax is rejected by mpv 0.41 and yields no audio.
- `--ao=pcm` decodes faster than real time. The reader thread sends 2,048-sample chunks over a `sync_channel` holding ≈ 5 s; when it is full the reader blocks, so mpv blocks on the full pipe. Never drop samples to bound the buffer.
- `next()` never takes a lock: a lock collision would read as an empty buffer and end the stream mid-fade. While mpv is buffering it returns silence and the fade-in does not advance.
- `PLAY_PLACE` returns `ERROR … not found in PATH` if `mpv` or `yt-dlp` is missing (`missing_place_tool()` in ipc.rs).

## EQ SUBSYSTEM (eq.rs)

- `N_BANDS = 10`; `BAND_FREQS = [31, 63, 125, 250, 500, 1k, 2k, 4k, 8k, 16k]` Hz; `EQ_Q = √2`; gain ±12 dB.
- `peaking_coeffs()` follows Audio EQ Cookbook (peaking EQ filter).
- `apply_biquad()` uses Direct Form II Transposed.
- `BiquadCoeffs` is `Copy`; `BiquadState` is `Default + Copy`.
- Coefficients recompute every 512 samples. State is NOT reset on recompute — avoids audible clicks.

## IPC SERVER (ipc.rs)

- `run_ipc_server(socket_path, ready_path, state, audio_tx, idle_timeout)` binds the socket, writes the ready file, and accepts connections until shutdown is notified (`QUIT` or idle watchdog). It returns `Ok(())`; mod.rs then removes the socket/ready/PID files, sends `Shutdown`, and joins the audio thread. Never call `process::exit` from IPC code: it skips that cleanup and leaves mpv running.
- `dispatch()` returns `None` for `QUIT` (no reply; connection closes).
- Idle watchdog (`watch_idle`): checks every `min(idle_timeout, 60 s)`; shuts down once there have been no clients and both channels have been stopped for `idle_timeout`. `None` disables it (config `idle_timeout_mins = 0`). The TUI holds one connection open, so the daemon never idles out under an open TUI.
- Paths are parameters so integration tests can use a temp dir; tests never touch the real data directory.
- Full command table: root AGENTS.md.

## NOISE LEVEL BASELINE (measured 2026-04-05)

`audio.rs` contains a `#[ignore]` test `measure_noise_levels` that skips the 1.5 s fade-in and measures 1 second of steady-state output at volume 1.0:

| Preset | RMS    | Peak   | Crest Factor |
|--------|--------|--------|--------------|
| White  | 0.0997 | 0.1725 | 1.73         |
| Pink   | 0.1041 | 0.3742 | 3.59         |
| Brown  | 0.1047 | 0.4182 | 3.99         |

All three types normalized to ~0.1 RMS at volume=1.0.
Run with: `cargo test measure_noise_levels -- --nocapture --ignored`

## ANTI-PATTERNS

- Never use tokio for the audio thread — `rodio::OutputStream` is `!Send`.
- Never `unwrap()` on `try_lock` in the audio hot path — skip on contention.
- Never log from a source's `Iterator::next` — it runs on rodio's real-time thread.
- Never reset `BiquadState` when EQ coefficients change — causes audible clicks.
- Never share one `SharedGains` between channels — synth and place EQs are separate.
- Never call `sink.pause()` on `Stop` — set the voice's `fade_out` flag and let the source ramp down.
- Never write volume or EQ state from the audio thread — IPC owns those fields.
