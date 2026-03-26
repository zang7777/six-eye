use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Network {
    pub ssid: String,
    pub bssid: String,
    pub signal_strength: i32,
    pub channel: u8,
    pub frequency: f32,
    pub security: String,
    pub vendor: Option<String>,
    pub angle: f32,
    pub signal_history: VecDeque<(DateTime<Local>, i32)>,
    pub is_connected: bool,
}

impl Network {
    pub fn band_label(&self) -> &'static str {
        match self.frequency {
            freq if freq >= 5.925 => "6 GHz",
            freq if freq >= 5.0 => "5 GHz",
            _ => "2.4 GHz",
        }
    }

    pub fn is_hidden(&self) -> bool {
        self.ssid.trim().is_empty() || self.ssid == "<hidden>"
    }

    pub fn peak_signal(&self) -> i32 {
        self.signal_history
            .iter()
            .map(|(_, signal)| *signal)
            .max()
            .unwrap_or(self.signal_strength)
    }

    pub fn last_seen(&self) -> Option<&DateTime<Local>> {
        self.signal_history.back().map(|(ts, _)| ts)
    }

    pub fn estimated_distance_m(&self) -> Option<f32> {
        let freq_mhz = self.frequency * 1000.0;
        if freq_mhz <= 0.0 {
            return None;
        }

        let path_loss = 27.55 - (20.0 * freq_mhz.log10()) + self.signal_strength.abs() as f32;
        Some(10_f32.powf(path_loss / 20.0).clamp(0.3, 999.0))
    }

    pub fn average_signal(&self) -> f32 {
        let mut sum = 0.0;
        let mut count = 0.0;

        for (_, signal) in &self.signal_history {
            sum += *signal as f32;
            count += 1.0;
        }

        if count == 0.0 {
            self.signal_strength as f32
        } else {
            sum / count
        }
    }

    pub fn signal_span(&self) -> i32 {
        let mut min_signal = self.signal_strength;
        let mut max_signal = self.signal_strength;

        for (_, signal) in &self.signal_history {
            min_signal = min_signal.min(*signal);
            max_signal = max_signal.max(*signal);
        }

        max_signal - min_signal
    }

    pub fn signal_stddev(&self) -> f32 {
        let samples = self.recent_samples(8);
        if samples.len() < 2 {
            return 0.0;
        }

        let mean = samples.iter().sum::<f32>() / samples.len() as f32;
        let variance = samples
            .iter()
            .map(|sample| {
                let delta = *sample - mean;
                delta * delta
            })
            .sum::<f32>()
            / samples.len() as f32;

        variance.sqrt()
    }

    pub fn signal_variance(&self) -> f32 {
        let stddev = self.signal_stddev();
        stddev * stddev
    }

    pub fn activity_score(&self) -> f32 {
        let strength_weight = ((self.signal_strength + 90) as f32 / 60.0).clamp(0.25, 1.0);
        let mean = self.average_signal();
        let latest_delta = self
            .signal_history
            .back()
            .map(|(_, signal)| (*signal as f32 - mean).abs())
            .unwrap_or(0.0);

        self.signal_stddev() * strength_weight
            + latest_delta * 0.18
            + if self.is_connected { 0.25 } else { 0.0 }
    }

    fn recent_samples(&self, window: usize) -> Vec<f32> {
        let mut samples: Vec<f32> = self
            .signal_history
            .iter()
            .rev()
            .take(window)
            .map(|(_, signal)| *signal as f32)
            .collect();
        samples.reverse();
        samples
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeviceRole {
    Gateway,
    Router,
    Peer,
    Unknown,
}

impl DeviceRole {
    pub fn label(&self) -> &'static str {
        match self {
            DeviceRole::Gateway => "Gateway",
            DeviceRole::Router => "Router",
            DeviceRole::Peer => "Peer",
            DeviceRole::Unknown => "Unknown",
        }
    }

    pub fn sort_key(&self) -> u8 {
        match self {
            DeviceRole::Gateway => 0,
            DeviceRole::Router => 1,
            DeviceRole::Peer => 2,
            DeviceRole::Unknown => 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectedDevice {
    pub addresses: Vec<String>,
    pub interface: String,
    pub mac_address: Option<String>,
    pub vendor: Option<String>,
    pub state: String,
    pub role: DeviceRole,
    pub fingerprint: String,
}

impl ConnectedDevice {
    pub fn primary_address(&self) -> &str {
        self.addresses.first().map(String::as_str).unwrap_or("—")
    }

    pub fn address_label(&self) -> String {
        self.addresses.join(", ")
    }

    pub fn vendor_label(&self) -> &str {
        self.vendor.as_deref().unwrap_or("Unknown vendor")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConnectionInfo {
    pub interface: String,
    pub connection_name: String,
    pub ssid: Option<String>,
    pub bssid: Option<String>,
    pub local_hwaddr: Option<String>,
    pub local_ipv4: Option<String>,
    pub gateway: Option<String>,
}

impl ConnectionInfo {
    pub fn display_name(&self) -> &str {
        if let Some(ssid) = self.ssid.as_deref().filter(|ssid| !ssid.is_empty()) {
            ssid
        } else if !self.connection_name.is_empty() {
            self.connection_name.as_str()
        } else {
            "Offline"
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlertLevel {
    Normal,
    Attention,
    Warning,
    Critical,
}

impl AlertLevel {
    pub fn label(&self) -> &'static str {
        match self {
            AlertLevel::Normal => "Normal",
            AlertLevel::Attention => "Attention",
            AlertLevel::Warning => "Warning",
            AlertLevel::Critical => "Critical",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            AlertLevel::Normal => "●",
            AlertLevel::Attention => "◉",
            AlertLevel::Warning => "▲",
            AlertLevel::Critical => "⬤",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportRecord {
    pub timestamp: String,
    pub networks: Vec<NetworkExport>,
    pub devices: Vec<DeviceExport>,
    pub presence_score: f32,
    pub motion_score: f32,
    pub posture: String,
    pub breathing_bpm: Option<f32>,
    pub alert_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkExport {
    pub ssid: String,
    pub bssid: String,
    pub signal_dbm: i32,
    pub channel: u8,
    pub frequency: f32,
    pub security: String,
    pub vendor: Option<String>,
    pub band: String,
    pub is_connected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceExport {
    pub addresses: Vec<String>,
    pub mac_address: Option<String>,
    pub vendor: Option<String>,
    pub role: String,
    pub fingerprint: String,
    pub state: String,
}
