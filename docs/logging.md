# Logging

Woosh logs with the `tracing` crate. Verbosity comes from the `RUST_LOG` environment variable (default `info`).

## Where logs go

- **Daemon:** appended to `woosh.log` in the data directory (`~/Library/Application Support/woosh/` on macOS, `${XDG_DATA_HOME:-~/.local/share}/woosh/` on Linux). The daemon's stdout and stderr go to the same file.
- **CLI and TUI:** stderr.

## Levels

- `error`: failures the user should know about (audio device, mpv)
- `warn`: problems that don't stop woosh (unreadable config, broken IPC connection)
- `info`: startup, shutdown, and major state changes (default)
- `debug` / `trace`: developer diagnostics

## Usage

`RUST_LOG` is read when the daemon starts, so set it on the command that spawns the daemon, or run the daemon in the foreground:

```bash
RUST_LOG=debug woosh daemon --no-daemonize

# Per-module control
RUST_LOG=woosh=info,woosh::daemon::audio=debug woosh daemon --no-daemonize
```

## In code

```rust
tracing::error!("audio: cannot open output device: {e}");
tracing::info!("auto-shutdown: idle for {idle_timeout:?} with no clients");
```

Prefix audio-thread messages with `audio:`. Never log from the audio callback (`Iterator::next` of a source); it runs on rodio's real-time thread.
