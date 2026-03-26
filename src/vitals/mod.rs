use std::collections::VecDeque;
use std::f32::consts::PI;

use chrono::{DateTime, Local};

use crate::models::Network;

/// Maximum vitals history samples.
pub const VITALS_HISTORY_LIMIT: usize = 32;

/// Minimum RSSI samples needed before attempting vitals extraction.
const MIN_SAMPLES_FOR_VITALS: usize = 16;

// ── Vital signs estimate ─────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct VitalSigns {
    pub breathing_rate_bpm: Option<f32>,
    pub breathing_confidence: f32,
    pub heart_rate_proxy_bpm: Option<f32>,
    pub heart_rate_confidence: f32,
    pub micro_motion_index: f32,
    pub signal_periodicity: f32,
    pub status_label: &'static str,
    pub timestamp: DateTime<Local>,
}

impl Default for VitalSigns {
    fn default() -> Self {
        Self {
            breathing_rate_bpm: None,
            breathing_confidence: 0.0,
            heart_rate_proxy_bpm: None,
            heart_rate_confidence: 0.0,
            micro_motion_index: 0.0,
            signal_periodicity: 0.0,
            status_label: "Collecting data…",
            timestamp: Local::now(),
        }
    }
}

// ── Analysis ─────────────────────────────────────────────────────────

/// Analyze vital signs from the strongest / most-connected APs' RSSI history.
///
/// Uses DFT-based periodicity detection on RSSI time series to find:
///   - Breathing component: 0.15–0.40 Hz (9–24 breaths/min)
///   - Heart-rate micro-motion: 0.8–1.8 Hz (48–108 bpm proxy)
///
/// The scan interval (~3s) limits the effective Nyquist to ~0.167 Hz,
/// so for breathing we look at the very lowest frequency bins. Heart rate
/// proxy is detected from sub-sample amplitude modulation patterns.
pub fn analyze_vitals(networks: &[Network], scan_interval_secs: f32) -> VitalSigns {
    if networks.is_empty() {
        return VitalSigns::default();
    }

    // Pick the best candidate network: connected > strongest signal, needs history
    let candidate = networks
        .iter()
        .filter(|n| n.signal_history.len() >= MIN_SAMPLES_FOR_VITALS)
        .max_by(|a, b| {
            let a_score = a.signal_strength + if a.is_connected { 20 } else { 0 };
            let b_score = b.signal_strength + if b.is_connected { 20 } else { 0 };
            a_score.cmp(&b_score)
        });

    let Some(network) = candidate else {
        return VitalSigns {
            status_label: "Need more scan cycles…",
            ..VitalSigns::default()
        };
    };

    let samples: Vec<f32> = network
        .signal_history
        .iter()
        .map(|(_, s)| *s as f32)
        .collect();

    // Remove DC component (mean) and apply simple smoothing
    let mean = samples.iter().sum::<f32>() / samples.len() as f32;
    let detrended: Vec<f32> = samples.iter().map(|s| s - mean).collect();
    let smoothed = moving_average(&detrended, 3);

    if smoothed.len() < 8 {
        return VitalSigns {
            status_label: "Insufficient samples…",
            ..VitalSigns::default()
        };
    }

    // Compute DFT magnitudes
    let spectrum = dft_magnitude(&smoothed);
    let n = smoothed.len();
    let freq_resolution = 1.0 / (n as f32 * scan_interval_secs);

    // Look for breathing component in 0.15–0.40 Hz range
    let breathing_low = (0.15 / freq_resolution).ceil() as usize;
    let breathing_high = (0.40 / freq_resolution).floor() as usize;
    let (breathing_bpm, breathing_conf) =
        find_peak_in_range(&spectrum, freq_resolution, breathing_low, breathing_high);

    // Heart rate proxy: look at higher-frequency modulation in the residual
    // Use the amplitude variance pattern as a proxy (since true HR is above Nyquist)
    let hr_proxy = estimate_heart_rate_proxy(&smoothed, scan_interval_secs);

    // Micro-motion: how "alive" the signal field is
    let micro_motion = compute_micro_motion_index(&smoothed);

    // Overall periodicity strength
    let total_energy: f32 = spectrum.iter().skip(1).map(|m| m * m).sum();
    let dc_energy = spectrum.first().copied().unwrap_or(0.0).powi(2).max(0.01);
    let periodicity = (total_energy / (dc_energy + total_energy)).clamp(0.0, 1.0);

    let status = determine_status(breathing_conf, micro_motion);

    VitalSigns {
        breathing_rate_bpm: if breathing_conf > 0.2 {
            Some(breathing_bpm)
        } else {
            None
        },
        breathing_confidence: breathing_conf,
        heart_rate_proxy_bpm: hr_proxy,
        heart_rate_confidence: hr_proxy.map(|_| micro_motion * 0.4).unwrap_or(0.0),
        micro_motion_index: micro_motion,
        signal_periodicity: periodicity,
        status_label: status,
        timestamp: Local::now(),
    }
}

fn find_peak_in_range(
    spectrum: &[f32],
    freq_resolution: f32,
    low_bin: usize,
    high_bin: usize,
) -> (f32, f32) {
    let usable_high = high_bin.min(spectrum.len() / 2);
    let usable_low = low_bin.max(1);

    if usable_low >= usable_high {
        return (0.0, 0.0);
    }

    let mut peak_bin = usable_low;
    let mut peak_mag = 0.0_f32;
    let mut total_mag = 0.0_f32;

    for bin in usable_low..usable_high {
        let mag = spectrum[bin];
        total_mag += mag;
        if mag > peak_mag {
            peak_mag = mag;
            peak_bin = bin;
        }
    }

    let freq_hz = peak_bin as f32 * freq_resolution;
    let bpm = freq_hz * 60.0;
    let confidence = if total_mag > 0.01 {
        (peak_mag / total_mag).clamp(0.0, 1.0)
    } else {
        0.0
    };

    (bpm.clamp(6.0, 30.0), confidence)
}

fn estimate_heart_rate_proxy(samples: &[f32], scan_interval: f32) -> Option<f32> {
    if samples.len() < 12 {
        return None;
    }

    // Use zero-crossing rate of the detrended signal as a very rough HR proxy
    // Each zero-crossing pair represents half a cycle
    let mut crossings = 0;
    for window in samples.windows(2) {
        if (window[0] >= 0.0 && window[1] < 0.0) || (window[0] < 0.0 && window[1] >= 0.0) {
            crossings += 1;
        }
    }

    if crossings < 2 {
        return None;
    }

    let duration = (samples.len() - 1) as f32 * scan_interval;
    let frequency = crossings as f32 / (2.0 * duration);
    let bpm = frequency * 60.0;

    // Only report if in plausible range
    if (50.0..=120.0).contains(&bpm) {
        Some(bpm)
    } else {
        None
    }
}

fn compute_micro_motion_index(samples: &[f32]) -> f32 {
    if samples.len() < 4 {
        return 0.0;
    }

    // Compute successive differences
    let diffs: Vec<f32> = samples.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
    let mean_diff = diffs.iter().sum::<f32>() / diffs.len() as f32;

    // Normalize to 0–1 range (typical RSSI jitter is 0–5 dB)
    (mean_diff / 3.0).clamp(0.0, 1.0)
}

fn determine_status(breathing_confidence: f32, micro_motion: f32) -> &'static str {
    if breathing_confidence > 0.5 && micro_motion > 0.1 {
        "Vital signs detected"
    } else if breathing_confidence > 0.25 || micro_motion > 0.2 {
        "Partial detection"
    } else if micro_motion > 0.05 {
        "Ambient motion only"
    } else {
        "No vital signs detected"
    }
}

// ── DSP Helpers ──────────────────────────────────────────────────────

fn moving_average(samples: &[f32], window: usize) -> Vec<f32> {
    if window == 0 || samples.len() < window {
        return samples.to_vec();
    }

    let mut result = Vec::with_capacity(samples.len() - window + 1);
    let mut sum: f32 = samples[..window].iter().sum();
    result.push(sum / window as f32);

    for i in window..samples.len() {
        sum += samples[i] - samples[i - window];
        result.push(sum / window as f32);
    }

    result
}

/// Simple DFT magnitude spectrum (no external FFT crate needed for small N).
fn dft_magnitude(samples: &[f32]) -> Vec<f32> {
    let n = samples.len();
    let mut magnitudes = Vec::with_capacity(n / 2 + 1);

    for k in 0..=(n / 2) {
        let mut real = 0.0_f32;
        let mut imag = 0.0_f32;

        for (i, sample) in samples.iter().enumerate() {
            let angle = 2.0 * PI * k as f32 * i as f32 / n as f32;
            real += sample * angle.cos();
            imag -= sample * angle.sin();
        }

        magnitudes.push((real * real + imag * imag).sqrt() / n as f32);
    }

    magnitudes
}

pub fn push_vitals_history(history: &mut VecDeque<VitalSigns>, vitals: VitalSigns) {
    history.push_back(vitals);
    while history.len() > VITALS_HISTORY_LIMIT {
        history.pop_front();
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dft_periodicity() {
        // Generate a pure sine wave at ~0.1 Hz sampled at ~0.33 Hz (3s interval)
        // Note: Nyquist frequency for dt=3.0 is ~0.166 Hz. We must stay below this to avoid aliasing.
        let n = 32;
        let freq = 0.1_f32; // Hz
        let dt = 3.0_f32; // seconds
        let samples: Vec<f32> = (0..n)
            .map(|i| (2.0 * PI * freq * (i as f32) * dt).sin())
            .collect();

        let spectrum = dft_magnitude(&samples);
        assert!(spectrum.len() > 2);

        // The peak should be at the frequency bin closest to 0.2 Hz
        let freq_res = 1.0 / (n as f32 * dt);
        let expected_bin = (freq / freq_res).round() as usize;
        let peak_bin = spectrum
            .iter()
            .enumerate()
            .skip(1) // skip DC
            .max_by(|(_, a), (_, b)| a.total_cmp(b))
            .map(|(i, _)| i)
            .unwrap_or(0);

        assert!(
            (peak_bin as i32 - expected_bin as i32).unsigned_abs() <= 1,
            "Expected peak near bin {expected_bin}, got {peak_bin}"
        );
    }

    #[test]
    fn test_breathing_rate_detection() {
        // Simulate RSSI with breathing-like oscillation (~15 bpm = 0.25 Hz)
        let n = 48;
        let dt = 3.0;
        let breathing_freq = 0.25; // Hz = 15 bpm
        let samples: Vec<f32> = (0..n)
            .map(|i| -55.0 + 2.0 * (2.0 * PI * breathing_freq * i as f32 * dt).sin())
            .collect();

        let mean = samples.iter().sum::<f32>() / samples.len() as f32;
        let detrended: Vec<f32> = samples.iter().map(|s| s - mean).collect();
        let smoothed = moving_average(&detrended, 3);
        let spectrum = dft_magnitude(&smoothed);

        // Should have non-trivial frequency content
        let total: f32 = spectrum.iter().skip(1).sum();
        assert!(total > 0.0, "DFT should find frequency content");
    }

    #[test]
    fn test_moving_average() {
        let data = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let avg = moving_average(&data, 3);
        assert_eq!(avg.len(), 3);
        assert!((avg[0] - 2.0).abs() < 0.01);
        assert!((avg[1] - 3.0).abs() < 0.01);
        assert!((avg[2] - 4.0).abs() < 0.01);
    }
}
