//! Downmix multichannel float PCM to stereo (AAC-LC / Opus accept at most 2 channels).

use std::sync::Once;

static NON_STANDARD_CH_LAYOUT: Once = Once::new();

/// Stronger than sqrt(1/2): lifts dialog / vocals in FC-heavy mixes (games, films).
const FC_TO_LR: f32 = 0.92;
/// Surrounds lowered vs strict ITU — reduces ambient mud that masks mids when folded to stereo.
const SURR_TO_LR: f32 = 0.50;
/// Direct LFE into stereo is a major source of “all bass” on Windows; fold very little or none.
const LFE_51: f32 = 0.0;
const LFE_71: f32 = 0.0;
const GAIN_51: f32 = 0.58;
const GAIN_71: f32 = 0.54;

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
            for i in 0..frames {
                let b = i * 6;
                let fl = samples[b];
                let fr = samples[b + 1];
                let fc = samples[b + 2];
                let lfe = samples[b + 3];
                let bl = samples[b + 4];
                let br = samples[b + 5];
                let l = fl + FC_TO_LR * fc + SURR_TO_LR * bl + LFE_51 * lfe;
                let r = fr + FC_TO_LR * fc + SURR_TO_LR * br + LFE_51 * lfe;
                out.push((l * GAIN_51).clamp(-1.0, 1.0));
                out.push((r * GAIN_51).clamp(-1.0, 1.0));
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
                let l = fl + FC_TO_LR * fc + SURR_TO_LR * bl + SURR_TO_LR * sl + LFE_71 * lfe;
                let r = fr + FC_TO_LR * fc + SURR_TO_LR * br + SURR_TO_LR * sr + LFE_71 * lfe;
                out.push((l * GAIN_71).clamp(-1.0, 1.0));
                out.push((r * GAIN_71).clamp(-1.0, 1.0));
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
