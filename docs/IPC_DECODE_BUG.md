# IPC Sample Decode Bug

## Problem
The TUI panicked with "Infinite-values are not supported!" from the spectrum-analyzer crate when displaying the spectrum visualizer.

## Root Cause
A bug in the hex decoding logic caused audio samples to be corrupted during IPC transmission from the daemon to the TUI.

### The Bug

**Encoding (daemon, `src/daemon/ipc.rs`)**:
```rust
fn encode_samples(samples: &[f32]) -> String {
    let mut out = String::with_capacity(samples.len() * 8);
    for &s in samples {
        for b in s.to_le_bytes() {  // Convert f32 to little-endian bytes
            let _ = write!(out, "{b:02x}");
        }
    }
    out
}
```

**Decoding (TUI, `src/tui/client.rs`) - BUGGY**:
```rust
fn decode_samples(hex: &str) -> Vec<f32> {
    hex.as_bytes()
        .chunks(8)
        .filter_map(|chunk| {
            let s = std::str::from_utf8(chunk).ok()?;
            let n = u32::from_str_radix(s, 16).ok()?;
            Some(f32::from_le_bytes(n.to_le_bytes()))  // BUG HERE
        })
        .collect()
}
```

### What Went Wrong

1. The hex string `"604a283d"` represents the bytes `[0x60, 0x4a, 0x28, 0x3d]`
2. Parsing `"604a283d"` as hex gives `u32 = 0x604a283d`
3. Calling `.to_le_bytes()` on `0x604a283d` gives `[0x3d, 0x28, 0x4a, 0x60]`
4. This is backwards! The original bytes were `[0x60, 0x4a, 0x28, 0x3d]`

The code was double-swapping the byte order, producing garbage float values like `58267840000000000000` instead of normal audio samples.

## The Fix

Decode each pair of hex characters directly into bytes:

```rust
fn decode_samples(hex: &str) -> Vec<f32> {
    hex.as_bytes()
        .chunks(8)
        .filter_map(|chunk| {
            if chunk.len() != 8 {
                return None;
            }
            let mut bytes = [0u8; 4];
            for (i, pair) in chunk.chunks(2).enumerate() {
                let s = std::str::from_utf8(pair).ok()?;
                let n = u8::from_str_radix(s, 16).ok()?;
                bytes[i] = n;
            }
            Some(f32::from_le_bytes(bytes))
        })
        .collect()
}
```

## Timeline

- Issue reported: Panic with "Infinite-values are not supported!"
- Debug logging added: Traced samples from audio generation → IPC encoding → IPC decoding
- Root cause identified: Double endianness swap in hex decode (via u32 then .to_le_bytes())
- Fix applied: Direct hex-to-bytes conversion instead of via u32
