# Logging

## Overview

Woosh uses Rust's standard logging ecosystem with runtime-controlled verbosity. Logging is always compiled in but can be controlled via environment variables.

## Logging Levels

- **`error`**: Critical failures, crashes, unrecoverable errors
- **`warn`**: Important issues that don't stop execution
- **`info`**: Key operations, startup/shutdown, major state changes (default)
- **`debug`**: Detailed diagnostics for troubleshooting
- **`trace`**: Very verbose, fine-grained execution details

## Usage

Set the `RUST_LOG` environment variable to control verbosity:

```bash
# Info level (default)
RUST_LOG=info woosh

# Debug level for troubleshooting
RUST_LOG=debug woosh

# Trace level for deep diagnostics
RUST_LOG=trace woosh

# Per-module control
RUST_LOG=woosh=debug,woosh::audio=trace woosh
```

## Implementation

Use the `log` crate for logging:

```rust
use log::{error, warn, info, debug, trace};

error!("Failed to connect: {}", e);
warn!("Retrying operation");
info!("Server started on port {}", port);
debug!("Processing request: {:?}", req);
trace!("Raw buffer: {:?}", buf);
```

## Best Practices

- Use `error!` for anything that requires user attention
- Use `info!` for operations users should know about
- Use `debug!` for developer troubleshooting
- Avoid logging in hot paths unless at debug/trace level
- Include context: module names, IDs, relevant data
