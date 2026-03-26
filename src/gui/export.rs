use std::fs::File;
use std::io::Write;

use chrono::Local;

use crate::gui::RadarApp;
use crate::models::*;

pub fn export_json(app: &RadarApp) -> Result<String, Box<dyn std::error::Error>> {
    let timestamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
    let filename = format!("gojosix_eye_net_export_{}.json", timestamp);

    let mut networks = Vec::new();
    for net in &app.local_networks {
        networks.push(NetworkExport {
            ssid: net.ssid.clone(),
            bssid: net.bssid.clone(),
            signal_dbm: net.signal_strength,
            channel: net.channel,
            frequency: net.frequency,
            security: net.security.clone(),
            vendor: net.vendor.clone(),
            band: net.band_label().to_string(),
            is_connected: net.is_connected,
        });
    }

    let mut devices = Vec::new();
    for dev in &app.local_connected_devices {
        devices.push(DeviceExport {
            addresses: dev.addresses.clone(),
            mac_address: dev.mac_address.clone(),
            vendor: dev.vendor.clone(),
            role: dev.role.label().to_string(),
            fingerprint: dev.fingerprint.clone(),
            state: dev.state.clone(),
        });
    }

    let record = ExportRecord {
        timestamp: Local::now().to_rfc3339(),
        networks,
        devices,
        presence_score: app.monitoring_summary.presence_score,
        motion_score: app.monitoring_summary.motion_score,
        posture: app.monitoring_summary.pose.posture.label().to_string(),
        breathing_bpm: app.monitoring_summary.vitals.breathing_rate_bpm,
        alert_level: app.monitoring_summary.alert_level.label().to_string(),
    };

    let json = serde_json::to_string_pretty(&record)?;
    let mut file = File::create(&filename)?;
    file.write_all(json.as_bytes())?;

    log::info!("Exported current state to {}", filename);
    Ok(filename)
}
