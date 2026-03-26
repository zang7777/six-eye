use std::collections::VecDeque;

use chrono::{DateTime, Local};

use crate::models::{AlertLevel, ConnectedDevice, Network};
use crate::pose::{self, PoseEstimate, Posture};
use crate::signal_health::{self, SignalHealthReport};
use crate::vitals::{self, VitalSigns};

pub const ACTIVITY_HISTORY_LIMIT: usize = 48;

const LIVE_SCAN_INTERVAL_SECS: f32 = 3.0;

#[derive(Debug, Clone)]
pub struct SignalHotspot {
    pub bssid: String,
    pub label: String,
    pub angle: f32,
    pub distance_ratio: f32,
    pub activity_score: f32,
    pub intensity: f32,
    pub band_label: &'static str,
    pub signal_strength: i32,
}

#[derive(Debug, Clone)]
pub struct MonitoringSummary {
    pub nearby_networks: usize,
    pub visible_networks: usize,
    pub hidden_networks: usize,
    pub connected_devices: usize,
    pub strongest_signal: i32,
    pub average_signal: f32,
    pub variance_index: f32,
    pub presence_score: f32,
    pub motion_score: f32,
    pub presence_detected: bool,
    pub motion_detected: bool,
    pub presence_label: &'static str,
    pub motion_label: &'static str,
    pub dominant_band: &'static str,
    pub hotspots: Vec<SignalHotspot>,
    pub pose: PoseEstimate,
    pub vitals: VitalSigns,
    pub alert_level: AlertLevel,
    pub zone_activity: [f32; 8],
    pub signal_health: SignalHealthReport,
}

impl Default for MonitoringSummary {
    fn default() -> Self {
        Self {
            nearby_networks: 0,
            visible_networks: 0,
            hidden_networks: 0,
            connected_devices: 0,
            strongest_signal: -100,
            average_signal: -100.0,
            variance_index: 0.0,
            presence_score: 0.0,
            motion_score: 0.0,
            presence_detected: false,
            motion_detected: false,
            presence_label: "Waiting",
            motion_label: "Waiting",
            dominant_band: "None",
            hotspots: Vec::new(),
            pose: PoseEstimate::default(),
            vitals: VitalSigns::default(),
            alert_level: AlertLevel::Normal,
            zone_activity: [0.0; 8],
            signal_health: SignalHealthReport::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ActivitySample {
    pub timestamp: DateTime<Local>,
    pub presence_score: f32,
    pub motion_score: f32,
    pub variance_index: f32,
    pub network_count: usize,
    pub device_count: usize,
    pub posture: Posture,
    pub breathing_bpm: Option<f32>,
}

pub fn summarize(networks: &[Network], devices: &[ConnectedDevice]) -> MonitoringSummary {
    if networks.is_empty() {
        return MonitoringSummary {
            connected_devices: devices.len(),
            ..MonitoringSummary::default()
        };
    }

    let visible_networks = networks
        .iter()
        .filter(|network| !network.is_hidden())
        .count();
    let hidden_networks = networks.len().saturating_sub(visible_networks);
    let strongest_signal = networks
        .iter()
        .map(|network| network.signal_strength)
        .max()
        .unwrap_or(-100);
    let average_signal = networks
        .iter()
        .map(|network| network.signal_strength as f32)
        .sum::<f32>()
        / networks.len() as f32;
    let variance_index =
        networks.iter().map(Network::activity_score).sum::<f32>() / networks.len() as f32;
    let hotspots = hotspots(networks);
    let hotspot_bias = hotspots
        .iter()
        .take(2)
        .map(|hotspot| hotspot.intensity)
        .sum::<f32>()
        * 0.45;
    let presence_score = smoothstep(variance_index + hotspot_bias, 0.65, 2.8);
    let motion_score = smoothstep(variance_index + hotspot_bias * 1.2, 1.6, 4.8);
    let presence_detected =
        presence_score >= 0.38 || hotspots.iter().any(|hotspot| hotspot.intensity >= 0.45);
    let motion_detected =
        motion_score >= 0.52 || hotspots.iter().any(|hotspot| hotspot.intensity >= 0.78);

    // Pose estimation
    let pose = pose::estimate_pose(networks);

    // Vital signs analysis
    let vitals = vitals::analyze_vitals(networks, LIVE_SCAN_INTERVAL_SECS);

    // Alert level
    let alert_level = compute_alert_level(presence_score, motion_score, &pose);

    // Per-zone activity from pose zones
    let mut zone_activity = [0.0_f32; 8];
    for zone in &pose.zones {
        zone_activity[zone.sector as usize] = zone.activity_level;
    }

    // Signal health analysis
    let signal_health = signal_health::analyze_signal_health(networks);

    MonitoringSummary {
        nearby_networks: networks.len(),
        visible_networks,
        hidden_networks,
        connected_devices: devices.len(),
        strongest_signal,
        average_signal,
        variance_index,
        presence_score,
        motion_score,
        presence_detected,
        motion_detected,
        presence_label: presence_label(presence_score),
        motion_label: motion_label(motion_score),
        dominant_band: dominant_band(networks),
        hotspots,
        pose,
        vitals,
        alert_level,
        zone_activity,
        signal_health,
    }
}

pub fn push_activity_sample(history: &mut VecDeque<ActivitySample>, summary: &MonitoringSummary) {
    history.push_back(ActivitySample {
        timestamp: Local::now(),
        presence_score: summary.presence_score,
        motion_score: summary.motion_score,
        variance_index: summary.variance_index,
        network_count: summary.nearby_networks,
        device_count: summary.connected_devices,
        posture: summary.pose.posture,
        breathing_bpm: summary.vitals.breathing_rate_bpm,
    });

    while history.len() > ACTIVITY_HISTORY_LIMIT {
        history.pop_front();
    }
}

fn compute_alert_level(presence_score: f32, motion_score: f32, pose: &PoseEstimate) -> AlertLevel {
    if motion_score > 0.8 && presence_score > 0.7 {
        AlertLevel::Critical
    } else if motion_score > 0.5 || pose.posture == Posture::Moving {
        AlertLevel::Warning
    } else if presence_score > 0.4 {
        AlertLevel::Attention
    } else {
        AlertLevel::Normal
    }
}

fn hotspots(networks: &[Network]) -> Vec<SignalHotspot> {
    let mut hotspots: Vec<SignalHotspot> = networks
        .iter()
        .filter(|network| network.signal_history.len() >= 3)
        .map(|network| {
            let activity_score = network.activity_score();
            SignalHotspot {
                bssid: network.bssid.clone(),
                label: if network.is_hidden() {
                    "<hidden>".to_string()
                } else {
                    network.ssid.clone()
                },
                angle: network.angle,
                distance_ratio: signal_distance_ratio(network.signal_strength),
                activity_score,
                intensity: smoothstep(activity_score, 0.8, 4.8),
                band_label: network.band_label(),
                signal_strength: network.signal_strength,
            }
        })
        .filter(|hotspot| hotspot.intensity > 0.08)
        .collect();

    hotspots.sort_by(|left, right| right.activity_score.total_cmp(&left.activity_score));
    hotspots.truncate(6);
    hotspots
}

fn dominant_band(networks: &[Network]) -> &'static str {
    let mut band_24 = 0;
    let mut band_5 = 0;
    let mut band_6 = 0;

    for network in networks {
        match network.band_label() {
            "2.4 GHz" => band_24 += 1,
            "5 GHz" => band_5 += 1,
            "6 GHz" => band_6 += 1,
            _ => {}
        }
    }

    if band_6 > band_5 && band_6 > band_24 {
        "6 GHz"
    } else if band_5 >= band_24 {
        "5 GHz"
    } else {
        "2.4 GHz"
    }
}

fn presence_label(score: f32) -> &'static str {
    match score {
        score if score >= 0.75 => "Occupied",
        score if score >= 0.45 => "Likely present",
        score if score >= 0.2 => "Low drift",
        _ => "Calm",
    }
}

fn motion_label(score: f32) -> &'static str {
    match score {
        score if score >= 0.78 => "Active motion",
        score if score >= 0.48 => "Light motion",
        score if score >= 0.2 => "Micro motion",
        _ => "Still",
    }
}

fn smoothstep(value: f32, low: f32, high: f32) -> f32 {
    if high <= low {
        return 0.0;
    }

    let t = ((value - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn signal_distance_ratio(signal_strength: i32) -> f32 {
    let normalized = ((signal_strength + 90) as f32 / 60.0).clamp(0.0, 1.0);
    1.0 - normalized * 0.82
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Local;
    use std::collections::VecDeque;

    fn network(name: &str, history: &[i32], connected: bool) -> Network {
        let mut signal_history = VecDeque::new();
        for signal in history {
            signal_history.push_back((Local::now(), *signal));
        }

        Network {
            ssid: name.to_string(),
            bssid: format!("AA:BB:CC:00:00:{:02X}", history.len()),
            signal_strength: *history.last().unwrap_or(&-70),
            channel: 1,
            frequency: 2.412,
            security: "WPA2".to_string(),
            vendor: Some("Test Vendor".to_string()),
            angle: 90.0,
            signal_history,
            is_connected: connected,
        }
    }

    #[test]
    fn summary_flags_motion_when_variance_is_high() {
        let networks = vec![
            network("alpha", &[-54, -49, -58, -47, -60, -46], true),
            network("beta", &[-66, -69, -61, -71, -62, -70], false),
        ];

        let summary = summarize(&networks, &[]);

        assert!(summary.presence_detected);
        assert!(summary.motion_score > 0.3);
        assert!(!summary.hotspots.is_empty());
    }

    #[test]
    fn summary_includes_pose_and_vitals() {
        let networks = vec![network("alpha", &[-54, -49, -58, -47, -60, -46], true)];

        let summary = summarize(&networks, &[]);
        assert_ne!(summary.pose.posture, Posture::Away);
        // Vitals may or may not detect — depends on sample count
        assert!(summary.vitals.status_label.len() > 0);
    }
}
