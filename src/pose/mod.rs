use std::collections::VecDeque;

use chrono::{DateTime, Local};

use crate::models::Network;

/// Maximum pose history samples retained.
pub const POSE_HISTORY_LIMIT: usize = 32;

// ── Pose classification ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Posture {
    Standing,
    Sitting,
    Lying,
    Moving,
    Away,
}

impl Posture {
    pub fn label(&self) -> &'static str {
        match self {
            Posture::Standing => "Standing",
            Posture::Sitting => "Sitting",
            Posture::Lying => "Lying Down",
            Posture::Moving => "Moving",
            Posture::Away => "Away",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Posture::Standing => "🧍",
            Posture::Sitting => "🪑",
            Posture::Lying => "🛏",
            Posture::Moving => "🚶",
            Posture::Away => "🚫",
        }
    }
}

// ── Zone classification ──────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ZoneActivity {
    pub sector: u8, // 0–7
    pub label: &'static str,
    pub signal_mean: f32,
    pub signal_variance: f32,
    pub network_count: usize,
    pub activity_level: f32, // 0.0–1.0
    pub dominant: bool,
}

const ZONE_LABELS: [&str; 8] = [
    "Zone N", "Zone NE", "Zone E", "Zone SE", "Zone S", "Zone SW", "Zone W", "Zone NW",
];

// ── Pose estimate ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PoseEstimate {
    pub posture: Posture,
    pub confidence: f32,
    pub dominant_zone: Option<u8>,
    pub zones: Vec<ZoneActivity>,
    pub body_centroid_angle: f32,
    pub body_centroid_distance: f32,
    pub timestamp: DateTime<Local>,
}

impl Default for PoseEstimate {
    fn default() -> Self {
        Self {
            posture: Posture::Away,
            confidence: 0.0,
            dominant_zone: None,
            zones: Vec::new(),
            body_centroid_angle: 0.0,
            body_centroid_distance: 1.0,
            timestamp: Local::now(),
        }
    }
}

// ── Estimation logic ─────────────────────────────────────────────────

pub fn estimate_pose(networks: &[Network]) -> PoseEstimate {
    if networks.is_empty() {
        return PoseEstimate::default();
    }

    let zones = build_zone_map(networks);
    let (dominant_zone, dominant_activity) = zones
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.activity_level.total_cmp(&b.activity_level))
        .map(|(i, z)| (i as u8, z.activity_level))
        .unwrap_or((0, 0.0));

    // Aggregate signal statistics across all networks
    let total_variance: f32 =
        networks.iter().map(|n| n.signal_variance()).sum::<f32>() / networks.len().max(1) as f32;
    let total_stddev: f32 =
        networks.iter().map(|n| n.signal_stddev()).sum::<f32>() / networks.len().max(1) as f32;
    let avg_signal = networks
        .iter()
        .map(|n| n.signal_strength as f32)
        .sum::<f32>()
        / networks.len() as f32;
    let strongest = networks
        .iter()
        .map(|n| n.signal_strength)
        .max()
        .unwrap_or(-100);

    // Count how many zones have significant activity
    let active_zone_count = zones.iter().filter(|z| z.activity_level > 0.2).count();

    // Classify posture from RF behavioral indicators
    let posture = classify_posture(
        total_variance,
        total_stddev,
        avg_signal,
        strongest,
        active_zone_count,
        dominant_activity,
    );

    // Confidence is based on sample depth and signal diversity
    let sample_depth = networks
        .iter()
        .map(|n| n.signal_history.len())
        .max()
        .unwrap_or(0);
    let depth_factor = (sample_depth as f32 / 12.0).clamp(0.0, 1.0);
    let coverage_factor = (networks.len() as f32 / 4.0).clamp(0.3, 1.0);
    let confidence = (depth_factor * coverage_factor * 0.85).clamp(0.0, 0.95);

    // Compute body centroid from weighted AP positions
    let (centroid_angle, centroid_dist) = compute_centroid(networks);

    PoseEstimate {
        posture,
        confidence,
        dominant_zone: if dominant_activity > 0.15 {
            Some(dominant_zone)
        } else {
            None
        },
        zones,
        body_centroid_angle: centroid_angle,
        body_centroid_distance: centroid_dist,
        timestamp: Local::now(),
    }
}

fn classify_posture(
    variance: f32,
    stddev: f32,
    avg_signal: f32,
    strongest: i32,
    active_zones: usize,
    dominant_activity: f32,
) -> Posture {
    // Away: no significant signal presence
    if strongest < -80 && dominant_activity < 0.1 {
        return Posture::Away;
    }

    // Moving: high variance across multiple zones
    if stddev > 3.2 && active_zones >= 3 {
        return Posture::Moving;
    }
    if variance > 12.0 && dominant_activity > 0.6 {
        return Posture::Moving;
    }

    // Standing: moderate variance, single strong zone, good signal
    if stddev > 1.5 && stddev <= 3.2 && strongest > -60 && active_zones <= 2 {
        return Posture::Standing;
    }

    // Lying: very low variance, weak-to-moderate signal (body absorbing signal)
    if stddev < 0.8 && avg_signal < -55.0 && dominant_activity < 0.3 {
        return Posture::Lying;
    }

    // Sitting: low variance, focused zone, decent signal
    if stddev <= 1.5 && strongest > -65 {
        return Posture::Sitting;
    }

    // Default: if signal is present but pattern unclear
    if strongest > -75 {
        Posture::Sitting
    } else {
        Posture::Away
    }
}

fn compute_centroid(networks: &[Network]) -> (f32, f32) {
    let mut wx = 0.0_f32;
    let mut wy = 0.0_f32;
    let mut total_weight = 0.0_f32;

    for network in networks {
        let weight = ((network.signal_strength + 100) as f32).max(1.0);
        let angle = network.angle.to_radians();
        let dist = 1.0 - ((network.signal_strength + 90) as f32 / 60.0).clamp(0.0, 1.0) * 0.82;
        wx += angle.cos() * dist * weight;
        wy += angle.sin() * dist * weight;
        total_weight += weight;
    }

    if total_weight < 0.01 {
        return (0.0, 1.0);
    }

    let cx = wx / total_weight;
    let cy = wy / total_weight;
    let angle = cy.atan2(cx).to_degrees().rem_euclid(360.0);
    let distance = (cx * cx + cy * cy).sqrt().clamp(0.05, 1.0);

    (angle, distance)
}

fn build_zone_map(networks: &[Network]) -> Vec<ZoneActivity> {
    let mut zones: Vec<ZoneActivity> = (0..8)
        .map(|i| ZoneActivity {
            sector: i,
            label: ZONE_LABELS[i as usize],
            signal_mean: -100.0,
            signal_variance: 0.0,
            network_count: 0,
            activity_level: 0.0,
            dominant: false,
        })
        .collect();

    for network in networks {
        let sector = ((network.angle / 45.0).round() as u8) % 8;
        let z = &mut zones[sector as usize];
        z.network_count += 1;

        let sig = network.signal_strength as f32;
        if z.signal_mean <= -100.0 {
            z.signal_mean = sig;
        } else {
            z.signal_mean =
                (z.signal_mean * (z.network_count - 1) as f32 + sig) / z.network_count as f32;
        }

        z.signal_variance += network.signal_variance();
        z.activity_level += network.activity_score();
    }

    // Normalize activity levels
    let max_activity = zones
        .iter()
        .map(|z| z.activity_level)
        .fold(0.0_f32, f32::max);
    if max_activity > 0.0 {
        for zone in &mut zones {
            zone.activity_level = (zone.activity_level / max_activity).clamp(0.0, 1.0);
        }
    }

    // Mark dominant
    if let Some(dominant) = zones
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.activity_level.total_cmp(&b.activity_level))
        .map(|(i, _)| i)
    {
        if zones[dominant].activity_level > 0.2 {
            zones[dominant].dominant = true;
        }
    }

    zones
}

pub fn push_pose_history(history: &mut VecDeque<PoseEstimate>, estimate: PoseEstimate) {
    history.push_back(estimate);
    while history.len() > POSE_HISTORY_LIMIT {
        history.pop_front();
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn make_network(ssid: &str, signal: i32, angle: f32, history: &[i32]) -> Network {
        let mut signal_history = VecDeque::new();
        for s in history {
            signal_history.push_back((Local::now(), *s));
        }

        Network {
            ssid: ssid.to_string(),
            bssid: format!("AA:BB:CC:00:00:{:02X}", (angle as u8) % 255),
            signal_strength: signal,
            channel: 1,
            frequency: 2.412,
            security: "WPA2".to_string(),
            vendor: None,
            angle,
            signal_history,
            is_connected: false,
        }
    }

    #[test]
    fn test_pose_estimation_from_stable_signals() {
        let networks = vec![
            make_network("AP1", -45, 90.0, &[-45, -46, -45, -44, -45, -46]),
            make_network("AP2", -60, 180.0, &[-60, -61, -60, -59, -60, -61]),
        ];
        let estimate = estimate_pose(&networks);
        assert!(estimate.confidence > 0.0);
        assert_ne!(estimate.posture, Posture::Away);
    }

    #[test]
    fn test_zone_classification() {
        let networks = vec![
            make_network("AP1", -50, 10.0, &[-50, -51, -49]),
            make_network("AP2", -55, 20.0, &[-55, -56, -54]),
            make_network("AP3", -80, 200.0, &[-80, -79, -81]),
        ];
        let estimate = estimate_pose(&networks);
        assert_eq!(estimate.zones.len(), 8);

        // Zone N (sector 0) should have the most activity from AP1+AP2
        let zone_n = &estimate.zones[0];
        assert!(zone_n.network_count >= 2);
    }

    #[test]
    fn test_empty_networks_returns_away() {
        let estimate = estimate_pose(&[]);
        assert_eq!(estimate.posture, Posture::Away);
        assert_eq!(estimate.confidence, 0.0);
    }
}
