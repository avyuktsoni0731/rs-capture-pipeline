//! Dialog / presence emphasis on interleaved PCM (light HF tilt via sample differentiation).

/// Gentle emphasis so mids and consonants come forward vs overly bass-heavy mixes.
/// `prev[c]` is the previous sample for channel `c` — retain across chunks for continuous playback.
///
/// `strength` ≈ 0.06–0.14 is subtle; 0 disables when callers gate on `> 0`.
pub fn emphasis_interleaved_f32_inplace(
    samples: &mut [f32],
    channels: usize,
    prev: &mut [f32],
    strength: f32,
) {
    if samples.is_empty() || channels == 0 || strength <= 0.0 {
        return;
    }
    if prev.len() < channels {
        return;
    }
    let nf = samples.len() / channels;
    for frame in 0..nf {
        for c in 0..channels {
            let i = frame * channels + c;
            let x = samples[i];
            let d = x - prev[c];
            let y = x + strength * d;
            samples[i] = y.clamp(-1.0, 1.0);
            prev[c] = x;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emphasis_advances_prev_per_channel() {
        let mut v = vec![0.25f32, -0.25f32];
        let mut prev = vec![0.0f32; 2];
        emphasis_interleaved_f32_inplace(&mut v, 2, &mut prev, 0.2);
        assert!((prev[0] - 0.25).abs() < 1e-6);
        assert!((prev[1] + 0.25).abs() < 1e-6);
    }
}
