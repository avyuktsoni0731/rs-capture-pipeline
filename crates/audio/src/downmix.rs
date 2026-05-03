//! Downmix multichannel float PCM to stereo (AAC-LC / Opus accept at most 2 channels).

use std::sync::Once;

static NON_STANDARD_CH_LAYOUT: Once = Once::new();

const SQRT2_INV: f32 = 0.707_106_77;

/// Convert interleaved `f32` PCM to stereo. Mono is duplicated; stereo is pass-through.
/// **5.1** assumes **FL, FR, FC, LFE, BL, BR**. **7.1** assumes **FL, FR, FC, LFE, BL, BR, SL, SR**
/// (common Windows mix layouts). Other channel counts use **first two** interleaved samples per frame.
pub fn downmix_interleaved_f32_to_stereo(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels == 0 || samples.is_empty() {
        return Vec::new();
    }
    if channels == 1 {
        let frames = samples.len();
        let mut out = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let m = samples[i];
            out.push(m);
            out.push(m);
        }
        return out;
    }
    if channels == 2 {
        return samples.to_vec();
    }

    let frames = samples.len() / channels;
    let mut out = Vec::with_capacity(frames * 2);

    match channels {
        6 => {
            // ~ITU-style Lo/Ro: center and rear at 0.707; LFE is easy to over-mix (muddy bass) so we
            // keep it well below full “+3 dB to each” consumer fold-in.
            for i in 0..frames {
                let b = i * 6;
                let fl = samples[b];
                let fr = samples[b + 1];
                let fc = samples[b + 2];
                let lfe = samples[b + 3];
                let bl = samples[b + 4];
                let br = samples[b + 5];
                let l = fl + SQRT2_INV * fc + SQRT2_INV * bl + 0.07 * lfe;
                let r = fr + SQRT2_INV * fc + SQRT2_INV * br + 0.07 * lfe;
                let g = 0.52;
                out.push((l * g).clamp(-1.0, 1.0));
                out.push((r * g).clamp(-1.0, 1.0));
            }
        }
        8 => {
            for i in 0..frames {
                let b = i * 8;
                let fl = samples[b];
                let fr = samples[b + 1];
                let fc = samples[b + 2];
                let lfe = samples[b + 3];
                let bl = samples[b + 4];
                let br = samples[b + 5];
                let sl = samples[b + 6];
                let sr = samples[b + 7];
                let l = fl + SQRT2_INV * fc + SQRT2_INV * bl + SQRT2_INV * sl + 0.05 * lfe;
                let r = fr + SQRT2_INV * fc + SQRT2_INV * br + SQRT2_INV * sr + 0.05 * lfe;
                let g = 0.46;
                out.push((l * g).clamp(-1.0, 1.0));
                out.push((r * g).clamp(-1.0, 1.0));
            }
        }
        n => {
            NON_STANDARD_CH_LAYOUT.call_once(|| {
                tracing::info!(
                    "WASAPI reports {n} channels; downmixing to stereo using first two channels per frame"
                );
            });
            for i in 0..frames {
                let b = i * n;
                let l = *samples.get(b).unwrap_or(&0.0);
                let r = *samples.get(b + 1).unwrap_or(&l);
                out.push(l.clamp(-1.0, 1.0));
                out.push(r.clamp(-1.0, 1.0));
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eight_channel_frame_becomes_stereo_pair() {
        let mut v = vec![0.0f32; 8];
        v[0] = 1.0;
        v[1] = 1.0;
        let s = downmix_interleaved_f32_to_stereo(&v, 8);
        assert_eq!(s.len(), 2);
    }
}
