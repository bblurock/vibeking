//! Shared audio decoding + resampling helpers.
//!
//! Both the whisper.cpp engine (`local_stt`) and the Parakeet + ANE engine
//! (`local_parakeet`) need to consume the same in-memory WAV blob produced
//! by `audio::AudioCapture`, normalize it to mono Float32, and resample to
//! 16 kHz. Keeping the pipeline identical means engine comparisons stay
//! fair (no resampler differences leaking into WER deltas).

use anyhow::Result;

/// Decode a WAV byte slice into a mono Float32 buffer resampled to 16 kHz.
///
/// - Multi-channel input is downmixed by simple per-frame averaging.
/// - 16-bit / 24-bit PCM and float WAV are both supported.
/// - The resampler is a linear interpolator; quality is adequate for
///   speech-recognition models trained on telephony-grade audio.
pub fn decode_wav_to_mono_f32(wav: &[u8]) -> Result<Vec<f32>> {
    let cursor = std::io::Cursor::new(wav);
    let mut reader = hound::WavReader::new(cursor)?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let bits = spec.bits_per_sample as u32;
            let max = 1i64 << (bits - 1);
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max as f32))
                .collect::<Result<Vec<_>, _>>()?
        }
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<Vec<_>, _>>()?,
    };

    let mono = if spec.channels > 1 {
        samples
            .chunks(spec.channels as usize)
            .map(|c| c.iter().sum::<f32>() / c.len() as f32)
            .collect()
    } else {
        samples
    };

    Ok(resample_to_16k(&mono, spec.sample_rate))
}

/// Linear-interpolation resample to 16 kHz. Returns the input as-is when
/// already at 16 kHz to avoid floating-point round-trip noise.
pub fn resample_to_16k(samples: &[f32], src_rate: u32) -> Vec<f32> {
    if src_rate == 16_000 {
        return samples.to_vec();
    }
    let ratio = 16_000_f64 / src_rate as f64;
    let out_len = (samples.len() as f64 * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f64 / ratio;
        let idx = src_pos.floor() as usize;
        let frac = src_pos - idx as f64;
        let s0 = samples.get(idx).copied().unwrap_or(0.0);
        let s1 = samples.get(idx + 1).copied().unwrap_or(s0);
        out.push((s0 as f64 + frac * (s1 as f64 - s0 as f64)) as f32);
    }
    out
}
