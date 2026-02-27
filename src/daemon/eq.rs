use rodio::Source;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const N_BANDS: usize = 10;
pub const BAND_FREQS: [f32; N_BANDS] = [
    31.0, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];
pub const EQ_Q: f32 = std::f32::consts::SQRT_2;
pub const GAIN_MIN: f32 = -12.0;
pub const GAIN_MAX: f32 = 12.0;

/// Biquad filter coefficients for a peaking EQ band.
#[derive(Clone, Copy)]
pub struct BiquadCoeffs {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl BiquadCoeffs {
    /// Identity filter: passes samples through unchanged (y[n] = x[n]).
    #[must_use]
    pub const fn identity() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }
}

/// Two state variables for Direct Form II Transposed biquad.
#[derive(Clone, Copy, Default)]
pub struct BiquadState {
    pub s1: f32,
    pub s2: f32,
}

/// Compute peaking EQ coefficients (Audio EQ Cookbook).
///
/// Returns an identity filter when `|gain_db| < 1e-6` to avoid
/// state accumulation at 0 dB.
#[must_use]
pub fn peaking_coeffs(freq: f32, q: f32, gain_db: f32, sample_rate: u32) -> BiquadCoeffs {
    if gain_db.abs() < 1e-6 {
        return BiquadCoeffs::identity();
    }

    #[allow(clippy::cast_precision_loss)]
    let sr = sample_rate as f32;
    let a = 10.0_f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * freq / sr;
    let alpha = w0.sin() / (2.0 * q);

    let denom = 1.0 + alpha / a;
    let b0 = (1.0 + alpha * a) / denom;
    let b1 = -2.0 * w0.cos() / denom;
    let b2 = (1.0 - alpha * a) / denom;
    let a1 = b1; // same formula as b1
    let a2 = (1.0 - alpha / a) / denom;

    BiquadCoeffs { b0, b1, b2, a1, a2 }
}

/// Apply one sample through a biquad filter using Direct Form II Transposed.
///
/// Filter state is updated in place; the filtered sample is returned.
/// Do NOT reset state on coefficient changes — new coefficients take effect
/// smoothly within ~1 buffer, avoiding audible clicks.
#[inline]
pub fn apply_biquad(coeffs: &BiquadCoeffs, state: &mut BiquadState, x: f32) -> f32 {
    let y = coeffs.b0 * x + state.s1;
    state.s1 = coeffs.b1 * x - coeffs.a1 * y + state.s2;
    state.s2 = coeffs.b2 * x - coeffs.a2 * y;
    y
}

/// Wraps a `rodio::Source` and applies a 10-band peaking EQ in series.
///
/// Gains are shared via `Arc<Mutex<[f32; N_BANDS]>>` so the audio thread can
/// update them without re-creating the sink. Coefficients are refreshed every
/// 512 samples using `try_lock` (same pattern as `sample_buf`).
pub struct EqProcessor<S> {
    inner: S,
    eq_gains: Arc<Mutex<[f32; N_BANDS]>>,
    cached_gains: [f32; N_BANDS],
    coeffs: [BiquadCoeffs; N_BANDS],
    states: [BiquadState; N_BANDS],
    check_counter: u32,
    sample_rate: u32,
}

impl<S: Source<Item = f32>> EqProcessor<S> {
    pub fn new(inner: S, eq_gains: Arc<Mutex<[f32; N_BANDS]>>) -> Self {
        let sample_rate = inner.sample_rate();
        Self {
            inner,
            eq_gains,
            cached_gains: [0.0f32; N_BANDS],
            coeffs: [BiquadCoeffs::identity(); N_BANDS],
            states: [BiquadState::default(); N_BANDS],
            check_counter: 0,
            sample_rate,
        }
    }

    /// Poll `eq_gains` every 512 samples; recompute coefficients on change.
    fn refresh_coeffs_if_needed(&mut self) {
        self.check_counter += 1;
        if self.check_counter < 512 {
            return;
        }
        self.check_counter = 0;

        if let Ok(gains) = self.eq_gains.try_lock() {
            #[allow(clippy::float_cmp)]
            if *gains != self.cached_gains {
                self.cached_gains = *gains;
                for (i, &freq) in BAND_FREQS.iter().enumerate() {
                    self.coeffs[i] =
                        peaking_coeffs(freq, EQ_Q, self.cached_gains[i], self.sample_rate);
                }
            }
        }
    }
}

impl<S: Source<Item = f32>> Iterator for EqProcessor<S> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        self.refresh_coeffs_if_needed();
        let sample = self.inner.next()?;
        let mut x = sample;
        for i in 0..N_BANDS {
            x = apply_biquad(&self.coeffs[i], &mut self.states[i], x);
        }
        Some(x)
    }
}

impl<S: Source<Item = f32>> Source for EqProcessor<S> {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_at_zero_gain() {
        let c = peaking_coeffs(1000.0, EQ_Q, 0.0, 44_100);
        assert!((c.b0 - 1.0).abs() < f32::EPSILON, "b0={}", c.b0);
        assert!(c.b1.abs() < f32::EPSILON, "b1={}", c.b1);
        assert!(c.b2.abs() < f32::EPSILON, "b2={}", c.b2);
        assert!(c.a1.abs() < f32::EPSILON, "a1={}", c.a1);
        assert!(c.a2.abs() < f32::EPSILON, "a2={}", c.a2);
    }

    #[test]
    fn biquad_identity_passthrough() {
        let coeffs = BiquadCoeffs::identity();
        let mut state = BiquadState::default();
        for x in [-1.0_f32, 0.0, 0.5, 1.0] {
            let y = apply_biquad(&coeffs, &mut state, x);
            assert!((y - x).abs() < 1e-6, "y={y}, x={x}");
        }
    }

    #[test]
    fn peaking_coeffs_nonzero_gain() {
        let c = peaking_coeffs(1000.0, EQ_Q, 6.0, 44_100);
        // With +6 dB gain the passband should amplify — b0 > 1
        assert!(c.b0 > 1.0, "b0={}", c.b0);
    }

    #[test]
    fn gain_clamping_constants() {
        const _: () = {
            assert!(GAIN_MIN < 0.0);
            assert!(GAIN_MAX > 0.0);
        };
        assert_eq!(N_BANDS, BAND_FREQS.len());
    }
}
