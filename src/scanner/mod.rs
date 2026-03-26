pub mod ffi;

use std::collections::{HashMap, VecDeque};
use std::ffi::CString;
use std::process::Command;

use chrono::Local;
use thiserror::Error;

use crate::models::{ConnectedDevice, ConnectionInfo, DeviceRole, Network};
use crate::oui::OuiDatabase;

#[derive(Debug, Clone, Default)]
pub struct ScanBundle {
    pub networks: Vec<Network>,
    pub connection: Option<ConnectionInfo>,
    pub connected_devices: Vec<ConnectedDevice>,
}

#[derive(Debug, Error)]
pub enum ScanError {
    #[error("No WiFi networks found")]
    NoNetworksFound,
    #[error("WiFi Interface Not Found")]
    InterfaceNotFound,
    #[error("Permission Denied (Are you root? Or fallback to cached mode failed)")]
    PermissionDenied,
    #[error("NetworkManager (nmcli) not found")]
    NmcliNotFound,
    #[error("NetworkManager scan failed: {0}")]
    NmcliFailed(String),
    #[error("Unknown Zig Scan Error: {0}")]
    UnknownError(i32),
}

/// Helper to find the active WiFi interface on Linux (wlan0, wlp2s0, wlo1, etc.)
fn get_wifi_interface() -> String {
    if let Ok(entries) = std::fs::read_dir("/sys/class/net") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            // Prefix "wl" represents wireless interfaces in consistent network device naming
            if name.starts_with("wl") {
                return name;
            }
        }
    }
    "wlan0".to_string()
}

pub fn scan(oui_db: &OuiDatabase) -> Result<ScanBundle, ScanError> {
    let iface = get_wifi_interface();
    let c_iface = CString::new(iface.clone()).unwrap();
    let mut networks = scan_networks(&c_iface, oui_db)?;
    let connection = scan_connection(&iface);

    mark_connected_networks(&mut networks, connection.as_ref());
    let connected_devices = scan_connected_devices(&iface, connection.as_ref(), oui_db);

    Ok(ScanBundle {
        networks,
        connection,
        connected_devices,
    })
}

fn scan_networks(c_iface: &CString, oui_db: &OuiDatabase) -> Result<Vec<Network>, ScanError> {
    match (scan_via_zig(c_iface, oui_db), scan_via_nmcli(oui_db)) {
        (Ok(zig_networks), Ok(nmcli_networks)) => {
            // On Ubuntu, NetworkManager often has a richer unprivileged cache than
            // direct nl80211 reads, so prefer the broader result set when available.
            if nmcli_networks.len() > zig_networks.len() {
                Ok(nmcli_networks)
            } else if !zig_networks.is_empty() {
                Ok(zig_networks)
            } else if !nmcli_networks.is_empty() {
                Ok(nmcli_networks)
            } else {
                Err(ScanError::NoNetworksFound)
            }
        }
        (Ok(zig_networks), Err(_)) if !zig_networks.is_empty() => Ok(zig_networks),
        (Err(_), Ok(nmcli_networks)) if !nmcli_networks.is_empty() => Ok(nmcli_networks),
        (Err(zig_err), Err(nmcli_err)) => Err(prefer_scan_error(zig_err, nmcli_err)),
        (Ok(_), Err(nmcli_err)) => Err(nmcli_err),
        (Err(zig_err), Ok(_)) => Err(zig_err),
    }
}

fn scan_via_zig(c_iface: &CString, oui_db: &OuiDatabase) -> Result<Vec<Network>, ScanError> {
    // Try a fresh scan first; if the kernel rejects it, retry against cached
    // results on a fresh FFI call.
    match parse_scan_result(unsafe { ffi::wifi_scan(c_iface.as_ptr()) }, oui_db) {
        Err(ScanError::PermissionDenied) => {
            parse_scan_result(unsafe { ffi::wifi_get_cached(c_iface.as_ptr()) }, oui_db)
        }
        other => other,
    }
}

fn scan_via_nmcli(oui_db: &OuiDatabase) -> Result<Vec<Network>, ScanError> {
    let _ = Command::new("nmcli")
        .args(["dev", "wifi", "rescan"])
        .output();

    let output = Command::new("nmcli")
        .args([
            "-t",
            "-f",
            "ACTIVE,SSID,BSSID,SIGNAL,FREQ,CHAN,SECURITY",
            "dev",
            "wifi",
            "list",
        ])
        .output()
        .map_err(|_| ScanError::NmcliNotFound)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let message = if stderr.is_empty() {
            "nmcli returned a non-zero exit status".to_string()
        } else {
            stderr
        };
        return Err(ScanError::NmcliFailed(message));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let networks: Vec<Network> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| parse_nmcli_line(line, oui_db))
        .collect();

    if networks.is_empty() {
        return Err(ScanError::NoNetworksFound);
    }

    Ok(networks)
}

fn parse_scan_result(
    result_ptr: *const ffi::CScanResult,
    oui_db: &OuiDatabase,
) -> Result<Vec<Network>, ScanError> {
    if result_ptr.is_null() {
        return Err(ScanError::UnknownError(-999));
    }

    let c_result = unsafe { &*result_ptr };

    // Handle library errors
    if c_result.error_code < 0 {
        return match c_result.error_code {
            -1 => Err(ScanError::PermissionDenied),
            -2 => Err(ScanError::InterfaceNotFound),
            code => Err(ScanError::UnknownError(code)),
        };
    }

    let mut networks = Vec::with_capacity(c_result.count as usize);

    if c_result.count > 0 && !c_result.networks.is_null() {
        let c_networks =
            unsafe { std::slice::from_raw_parts(c_result.networks, c_result.count as usize) };

        for c_net in c_networks {
            // Parse SSID (up to 32 chars)
            let ssid = parse_c_string(&c_net.ssid);
            let ssid = if ssid.is_empty() {
                "<hidden>".to_string()
            } else {
                ssid
            };

            // Format MAC address
            let bssid = format!(
                "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                c_net.bssid[0],
                c_net.bssid[1],
                c_net.bssid[2],
                c_net.bssid[3],
                c_net.bssid[4],
                c_net.bssid[5]
            );

            let security = ffi::SecurityType::from_raw(c_net.security)
                .map(ffi::SecurityType::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("Unknown ({})", c_net.security));

            let vendor = oui_db.lookup(&bssid);
            let angle = stable_angle_from_identity(&bssid, c_net.channel);

            let mut signal_history = VecDeque::new();
            signal_history.push_back((Local::now(), c_net.signal_dbm));

            networks.push(Network {
                ssid,
                bssid,
                signal_strength: c_net.signal_dbm,
                channel: c_net.channel,
                // Zig gives us MHz, we want GHz
                frequency: (c_net.frequency as f32) / 1000.0,
                security,
                vendor,
                angle,
                signal_history,
                is_connected: false,
            });
        }
    }

    Ok(networks)
}

fn parse_nmcli_line(line: &str, oui_db: &OuiDatabase) -> Option<Network> {
    let fields = split_nmcli_fields(line);
    if fields.len() < 7 {
        return None;
    }

    let is_connected = matches!(fields[0].as_str(), "yes" | "true" | "activated");
    let ssid = unescape_nmcli_value(&fields[1]);
    let ssid = if ssid.trim().is_empty() {
        "<hidden>".to_string()
    } else {
        ssid
    };

    let bssid = normalize_mac(&unescape_nmcli_value(&fields[2]));
    let signal_pct: f32 = fields[3].parse().unwrap_or(0.0);
    let signal_dbm = percent_to_dbm(signal_pct);
    let freq_mhz: f32 = fields[4]
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let frequency = freq_mhz / 1000.0;
    let channel: u8 = fields[5].parse().unwrap_or(0);

    let security = unescape_nmcli_value(&fields[6]);
    let security = if security.trim().is_empty() {
        "Open".to_string()
    } else {
        security
    };

    let vendor = oui_db.lookup(&bssid);
    let angle = stable_angle_from_identity(&bssid, channel);

    let mut signal_history = VecDeque::new();
    signal_history.push_back((Local::now(), signal_dbm));

    Some(Network {
        ssid,
        bssid,
        signal_strength: signal_dbm,
        channel,
        frequency,
        security,
        vendor,
        angle,
        signal_history,
        is_connected,
    })
}

fn scan_connection(iface: &str) -> Option<ConnectionInfo> {
    let output = Command::new("nmcli")
        .args([
            "-t",
            "-f",
            "GENERAL.DEVICE,GENERAL.HWADDR,GENERAL.CONNECTION,GENERAL.TYPE,IP4.ADDRESS,IP4.GATEWAY",
            "device",
            "show",
            iface,
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let mut connection = ConnectionInfo {
        interface: iface.to_string(),
        ..ConnectionInfo::default()
    };

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };

        let value = normalize_optional(value.trim());
        match key {
            "GENERAL.DEVICE" => {
                if let Some(value) = value {
                    connection.interface = value;
                }
            }
            "GENERAL.HWADDR" => connection.local_hwaddr = value.map(|mac| normalize_mac(&mac)),
            "GENERAL.CONNECTION" => connection.connection_name = value.unwrap_or_default(),
            key if key.starts_with("IP4.ADDRESS") => {
                connection.local_ipv4 = value.map(|address| strip_cidr_suffix(&address));
            }
            "IP4.GATEWAY" => connection.gateway = value,
            _ => {}
        }
    }

    if let Some((ssid, bssid)) = current_association() {
        connection.ssid = normalize_optional(&ssid);
        connection.bssid = normalize_optional(&bssid).map(|mac| normalize_mac(&mac));
    } else if !connection.connection_name.is_empty() {
        connection.ssid = Some(connection.connection_name.clone());
    }

    if connection.interface.is_empty()
        && connection.connection_name.is_empty()
        && connection.local_hwaddr.is_none()
        && connection.local_ipv4.is_none()
    {
        None
    } else {
        Some(connection)
    }
}

fn current_association() -> Option<(String, String)> {
    let output = Command::new("nmcli")
        .args(["-t", "-f", "ACTIVE,SSID,BSSID", "dev", "wifi", "list"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let fields = split_nmcli_fields(line);
        if fields.len() < 3 || fields[0] != "yes" {
            continue;
        }

        return Some((
            unescape_nmcli_value(&fields[1]),
            unescape_nmcli_value(&fields[2]),
        ));
    }

    None
}

fn mark_connected_networks(networks: &mut [Network], connection: Option<&ConnectionInfo>) {
    let Some(connection) = connection else {
        return;
    };

    for network in networks {
        let bssid_match = connection
            .bssid
            .as_deref()
            .map(|bssid| bssid.eq_ignore_ascii_case(&network.bssid))
            .unwrap_or(false);
        let ssid_match = connection
            .ssid
            .as_deref()
            .map(|ssid| !ssid.is_empty() && ssid == network.ssid)
            .unwrap_or(false);

        if bssid_match || ssid_match {
            network.is_connected = true;
        }
    }
}

fn scan_connected_devices(
    iface: &str,
    connection: Option<&ConnectionInfo>,
    oui_db: &OuiDatabase,
) -> Vec<ConnectedDevice> {
    let output = match Command::new("ip").args(["neigh"]).output() {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log::warn!("ip neigh failed: {}", stderr.trim());
            return Vec::new();
        }
        Err(error) => {
            log::warn!("Could not execute ip neigh: {error}");
            return Vec::new();
        }
    };

    let gateway = connection.and_then(|connection| connection.gateway.as_deref());
    coalesce_neighbors(
        &String::from_utf8_lossy(&output.stdout),
        iface,
        gateway,
        oui_db,
    )
}

fn split_nmcli_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() && chars[i + 1] == ':' {
            current.push('\\');
            current.push(':');
            i += 2;
        } else if chars[i] == ':' {
            fields.push(current.clone());
            current.clear();
            i += 1;
        } else {
            current.push(chars[i]);
            i += 1;
        }
    }

    fields.push(current);
    fields
}

fn unescape_nmcli_value(value: &str) -> String {
    value.replace("\\:", ":")
}

fn normalize_mac(value: &str) -> String {
    value.replace('-', ":").to_uppercase()
}

fn normalize_optional(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "--" {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn strip_cidr_suffix(address: &str) -> String {
    address.split('/').next().unwrap_or(address).to_string()
}

fn percent_to_dbm(pct: f32) -> i32 {
    let clamped = pct.clamp(0.0, 100.0);
    (-100.0 + clamped * 0.7).round() as i32
}

fn parse_c_string(bytes: &[u8; 33]) -> String {
    let mut len = 0;
    while len < 32 && bytes[len] != 0 {
        len += 1;
    }
    String::from_utf8_lossy(&bytes[0..len]).to_string()
}

fn stable_angle_from_identity(bssid: &str, channel: u8) -> f32 {
    let mut hash = channel as u32;
    for byte in bssid.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(byte as u32);
    }
    (hash % 360) as f32
}

fn prefer_scan_error(primary: ScanError, secondary: ScanError) -> ScanError {
    match primary {
        ScanError::NoNetworksFound => secondary,
        _ => primary,
    }
}

#[derive(Debug, Clone)]
struct NeighborEntry {
    address: String,
    interface: String,
    mac_address: Option<String>,
    state: String,
    is_router: bool,
}

#[derive(Debug, Default)]
struct DeviceAccumulator {
    addresses: Vec<String>,
    interface: String,
    mac_address: Option<String>,
    vendor: Option<String>,
    states: Vec<String>,
    is_router: bool,
}

fn coalesce_neighbors(
    output: &str,
    iface: &str,
    gateway: Option<&str>,
    oui_db: &OuiDatabase,
) -> Vec<ConnectedDevice> {
    let entries = parse_neighbor_entries(output, iface);
    let mut grouped: HashMap<String, DeviceAccumulator> = HashMap::new();

    for entry in entries {
        let key = entry
            .mac_address
            .clone()
            .unwrap_or_else(|| entry.address.clone());

        let device = grouped.entry(key).or_insert_with(|| DeviceAccumulator {
            interface: entry.interface.clone(),
            mac_address: entry.mac_address.clone(),
            vendor: entry
                .mac_address
                .as_deref()
                .and_then(|mac_address| oui_db.lookup(mac_address)),
            ..DeviceAccumulator::default()
        });

        if !device
            .addresses
            .iter()
            .any(|address| address == &entry.address)
        {
            device.addresses.push(entry.address.clone());
        }

        if !entry.state.is_empty() && !device.states.iter().any(|state| state == &entry.state) {
            device.states.push(entry.state.clone());
        }

        if entry.is_router {
            device.is_router = true;
        }
    }

    let mut devices: Vec<ConnectedDevice> = grouped
        .into_values()
        .map(|device| {
            let role = infer_role(&device, gateway);
            let fingerprint = infer_device_fingerprint(device.vendor.as_deref(), &role);
            ConnectedDevice {
                addresses: device.addresses,
                interface: device.interface,
                mac_address: device.mac_address,
                vendor: device.vendor,
                state: if device.states.is_empty() {
                    "UNKNOWN".to_string()
                } else {
                    device.states.join(" / ")
                },
                role,
                fingerprint,
            }
        })
        .collect();

    devices.sort_by(|left, right| {
        left.role
            .sort_key()
            .cmp(&right.role.sort_key())
            .then_with(|| left.primary_address().cmp(right.primary_address()))
    });
    devices
}

fn parse_neighbor_entries(output: &str, iface: &str) -> Vec<NeighborEntry> {
    let mut entries = Vec::new();

    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }

        let address = parts[0].to_string();
        let mut interface = String::new();
        let mut mac_address = None;
        let mut is_router = false;
        let state = parts.last().copied().unwrap_or("").to_string();

        let mut index = 1;
        while index < parts.len() {
            match parts[index] {
                "dev" if index + 1 < parts.len() => {
                    interface = parts[index + 1].to_string();
                    index += 2;
                }
                "lladdr" if index + 1 < parts.len() => {
                    mac_address = Some(normalize_mac(parts[index + 1]));
                    index += 2;
                }
                "router" => {
                    is_router = true;
                    index += 1;
                }
                _ => {
                    index += 1;
                }
            }
        }

        if interface != iface {
            continue;
        }

        entries.push(NeighborEntry {
            address,
            interface,
            mac_address,
            state,
            is_router,
        });
    }

    entries
}

fn infer_role(device: &DeviceAccumulator, gateway: Option<&str>) -> DeviceRole {
    if gateway
        .map(|gateway| device.addresses.iter().any(|address| address == gateway))
        .unwrap_or(false)
    {
        DeviceRole::Gateway
    } else if device.is_router {
        DeviceRole::Router
    } else if !device.addresses.is_empty() {
        DeviceRole::Peer
    } else {
        DeviceRole::Unknown
    }
}

fn infer_device_fingerprint(vendor: Option<&str>, role: &DeviceRole) -> String {
    let role_hint = match role {
        DeviceRole::Gateway => "gateway / access point",
        DeviceRole::Router => "router-adjacent node",
        DeviceRole::Peer => "peer on local network",
        DeviceRole::Unknown => "unclassified radio peer",
    };

    let device_hint = vendor
        .map(|vendor| {
            let vendor = vendor.to_ascii_lowercase();
            if vendor.contains("apple") {
                "likely Apple phone, tablet, or laptop"
            } else if vendor.contains("intel") {
                "likely laptop or desktop WiFi chipset"
            } else if vendor.contains("xiaomi")
                || vendor.contains("oneplus")
                || vendor.contains("oppo")
                || vendor.contains("samsung")
                || vendor.contains("vivo")
            {
                "likely mobile device"
            } else if vendor.contains("raspberry") || vendor.contains("espressif") {
                "likely IoT or embedded device"
            } else if vendor.contains("tp-link")
                || vendor.contains("ubiquiti")
                || vendor.contains("mikrotik")
                || vendor.contains("cisco")
                || vendor.contains("huawei")
            {
                "likely network infrastructure"
            } else if vendor.contains("dell")
                || vendor.contains("lenovo")
                || vendor.contains("asus")
                || vendor.contains("liteon")
            {
                "likely laptop or workstation"
            } else {
                "vendor-class fingerprint only"
            }
        })
        .unwrap_or("unknown device family");

    format!("{role_hint} · {device_hint}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_nmcli_line_tracks_connected_state() {
        let oui = OuiDatabase::load();
        let line = "yes:test-net:C8\\:9C\\:BB\\:49\\:31\\:0D:90:5785 MHz:157:WPA2";
        let parsed = parse_nmcli_line(line, &oui).expect("nmcli line should parse");

        assert!(parsed.is_connected);
        assert_eq!(parsed.bssid, "C8:9C:BB:49:31:0D");
        assert_eq!(parsed.channel, 157);
    }

    #[test]
    fn coalesce_neighbors_merges_ipv4_and_ipv6_for_same_mac() {
        let oui = OuiDatabase::load();
        let output = "\
192.168.1.254 dev wlp0s20f3 lladdr c8:9c:bb:49:31:00 REACHABLE\n\
fe80::1 dev wlp0s20f3 lladdr c8:9c:bb:49:31:00 router REACHABLE\n";

        let devices = coalesce_neighbors(output, "wlp0s20f3", Some("192.168.1.254"), &oui);

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].role, DeviceRole::Gateway);
        assert_eq!(devices[0].addresses.len(), 2);
    }
}
