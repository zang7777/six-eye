use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

use crate::models::Network;

/// Composite signal health report for the current RF environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalHealthReport {
    /// Overall health score 0–100
    pub health_score: u8,
    /// Signal stability index (low variance = stable)
    pub stability: f32,
    /// Coverage quality based on band diversity and signal spread
    pub coverage_quality: f32,
    /// Channel congestion / interference index
    pub interference_score: f32,
    /// Room fingerprint — hex digest of sorted BSSID+channel+band
    pub room_fingerprint: String,
    /// Human-readable health label
    pub health_label: &'static str,
    /// Emoji for health
    pub health_emoji: &'static str,
    /// Per-channel congestion (channels 1–14 for 2.4GHz)
    pub channel_congestion: [u8; 14],
}

impl Default for SignalHealthReport {
    fn default() -> Self {
        Self {
            health_score: 0,
            stability: 0.0,
            coverage_quality: 0.0,
            interference_score: 0.0,
            room_fingerprint: "—".to_string(),
            health_label: "No Data",
            health_emoji: "⬛",
            channel_congestion: [0; 14],
        }
    }
}

pub fn analyze_signal_health(networks: &[Network]) -> SignalHealthReport {
    if networks.is_empty() {
        return SignalHealthReport::default();
    }

    let stability = compute_stability(networks);
    let coverage_quality = compute_coverage(networks);
    let interference_score = compute_interference(networks);
    let channel_congestion = compute_channel_congestion(networks);
    let room_fingerprint = compute_fingerprint(networks);

    // Composite score: stability 40%, coverage 35%, low-interference 25%
    let raw = stability * 0.40 + coverage_quality * 0.35 + (1.0 - interference_score) * 0.25;
    let health_score = (raw * 100.0).clamp(0.0, 100.0) as u8;

    let (health_label, health_emoji) = match health_score {
        90..=100 => ("Excellent", "🟢"),
        70..=89 => ("Good", "🟡"),
        50..=69 => ("Fair", "🟠"),
        _ => ("Poor", "🔴"),
    };

    SignalHealthReport {
        health_score,
        stability,
        coverage_quality,
        interference_score,
        room_fingerprint,
        health_label,
        health_emoji,
        channel_congestion,
    }
}

fn compute_stability(networks: &[Network]) -> f32 {
    let variances: Vec<f32> = networks
        .iter()
        .filter(|n| n.signal_history.len() >= 3)
        .map(|n| n.activity_score())
        .collect();

    if variances.is_empty() {
        return 0.5;
    }

    let avg_variance = variances.iter().sum::<f32>() / variances.len() as f32;
    // Map: low variance (0–1) = high stability, high variance (4+) = low stability
    (1.0 - (avg_variance / 5.0).min(1.0)).max(0.0)
}

fn compute_coverage(networks: &[Network]) -> f32 {
    let has_24 = networks.iter().any(|n| n.frequency < 4.0);
    let has_5 = networks
        .iter()
        .any(|n| n.frequency >= 4.9 && n.frequency < 5.925);
    let has_6 = networks.iter().any(|n| n.frequency >= 5.925);

    let band_diversity = (has_24 as u8 + has_5 as u8 + has_6 as u8) as f32 / 3.0;

    let best_signal = networks
        .iter()
        .map(|n| n.signal_strength)
        .max()
        .unwrap_or(-100);
    let signal_quality = ((best_signal + 90) as f32 / 60.0).clamp(0.0, 1.0);

    let count_factor = (networks.len() as f32 / 15.0).min(1.0);

    band_diversity * 0.35 + signal_quality * 0.45 + count_factor * 0.20
}

fn compute_interference(networks: &[Network]) -> f32 {
    // Count overlapping 2.4 GHz APs on same channels
    let congestion = compute_channel_congestion(networks);
    let max_congestion = *congestion.iter().max().unwrap_or(&0);
    let avg_congestion = congestion
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| c as f32)
        .sum::<f32>()
        / congestion.iter().filter(|&&c| c > 0).count().max(1) as f32;

    // Normalize: 1 AP per channel = no interference, 5+ = heavy
    let peak_factor = ((max_congestion as f32 - 1.0) / 4.0).clamp(0.0, 1.0);
    let avg_factor = ((avg_congestion - 1.0) / 3.0).clamp(0.0, 1.0);

    peak_factor * 0.6 + avg_factor * 0.4
}

fn compute_channel_congestion(networks: &[Network]) -> [u8; 14] {
    let mut congestion = [0_u8; 14];

    for network in networks {
        if network.channel >= 1 && network.channel <= 14 {
            let idx = (network.channel - 1) as usize;
            congestion[idx] = congestion[idx].saturating_add(1);

            // 2.4 GHz channels overlap ±2
            if network.frequency < 4.0 {
                for offset in 1..=2_u8 {
                    if network.channel > offset {
                        let neighbor = (network.channel - offset - 1) as usize;
                        if neighbor < 14 {
                            congestion[neighbor] = congestion[neighbor].saturating_add(1);
                        }
                    }
                    let neighbor = (network.channel - 1 + offset) as usize;
                    if neighbor < 14 {
                        congestion[neighbor] = congestion[neighbor].saturating_add(1);
                    }
                }
            }
        }
    }

    congestion
}

fn compute_fingerprint(networks: &[Network]) -> String {
    let mut entries: Vec<String> = networks
        .iter()
        .map(|n| format!("{}:{}:{}", n.bssid, n.channel, n.band_label()))
        .collect();
    entries.sort();

    let mut hasher = DefaultHasher::new();
    for entry in &entries {
        entry.hash(&mut hasher);
    }
    let hash = hasher.finish();
    format!("{:016X}", hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Local;
    use std::collections::VecDeque;

    fn test_network(bssid: &str, signal: i32, channel: u8, freq: f32) -> Network {
        let mut history = VecDeque::new();
        for s in [signal, signal - 2, signal + 1] {
            history.push_back((Local::now(), s));
        }
        Network {
            ssid: "Test".to_string(),
            bssid: bssid.to_string(),
            signal_strength: signal,
            channel,
            frequency: freq,
            security: "WPA2".to_string(),
            vendor: None,
            angle: 0.0,
            signal_history: history,
            is_connected: false,
        }
    }

    #[test]
    fn test_signal_health_basic() {
        let networks = vec![
            test_network("AA:BB:CC:00:00:01", -45, 1, 2.412),
            test_network("AA:BB:CC:00:00:02", -60, 6, 2.437),
            test_network("AA:BB:CC:00:00:03", -55, 36, 5.180),
        ];
        let report = analyze_signal_health(&networks);
        assert!(report.health_score > 0);
        assert!(!report.room_fingerprint.is_empty());
    }

    #[test]
    fn test_fingerprint_deterministic() {
        let networks = vec![
            test_network("AA:BB:CC:00:00:01", -45, 1, 2.412),
            test_network("AA:BB:CC:00:00:02", -60, 6, 2.437),
        ];
        let fp1 = compute_fingerprint(&networks);
        let fp2 = compute_fingerprint(&networks);
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn test_channel_congestion() {
        let networks = vec![
            test_network("A", -50, 1, 2.412),
            test_network("B", -55, 2, 2.417),
            test_network("C", -60, 1, 2.412),
        ];
        let congestion = compute_channel_congestion(&networks);
        assert!(congestion[0] >= 3); // ch1 sees its own + overlap from ch2
    }
}
