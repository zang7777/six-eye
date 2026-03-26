#![allow(dead_code)]

use std::ffi::c_char;

#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SecurityType {
    Open = 0,
    WEP = 1,
    WPA = 2,
    WPA2 = 3,
    WPA3 = 4,
}

impl SecurityType {
    pub fn from_raw(value: u8) -> Option<Self> {
        match value {
            0 => Some(SecurityType::Open),
            1 => Some(SecurityType::WEP),
            2 => Some(SecurityType::WPA),
            3 => Some(SecurityType::WPA2),
            4 => Some(SecurityType::WPA3),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SecurityType::Open => "Open",
            SecurityType::WEP => "WEP",
            SecurityType::WPA => "WPA",
            SecurityType::WPA2 => "WPA2",
            SecurityType::WPA3 => "WPA3",
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct CWifiNetwork {
    pub ssid: [u8; 33],
    pub bssid: [u8; 6],
    pub signal_dbm: i32,
    pub frequency: u32,
    pub channel: u8,
    pub security: u8,
    pub bss_status: u8,
    pub beacon_interval: u16,
    pub _pad: [u8; 1],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct CScanResult {
    pub networks: *const CWifiNetwork,
    pub count: u32,
    pub error_code: i32,
}

extern "C" {
    /// Perform a WiFi scan and return results.
    /// `iface_name` is the WiFi interface (e.g., "wlan0\0").
    /// Returns a pointer to a CScanResult struct.
    pub fn wifi_scan(iface_name: *const c_char) -> *const CScanResult;

    /// Just get cached scan results WITHOUT triggering a new scan.
    pub fn wifi_get_cached(iface_name: *const c_char) -> *const CScanResult;

    /// Returns the library version
    pub fn wifi_scanner_version() -> u32;
}

#[cfg(test)]
mod tests {
    use super::CWifiNetwork;
    use std::mem::{align_of, offset_of, size_of};

    #[test]
    fn c_wifi_network_layout_matches_zig() {
        assert_eq!(size_of::<CWifiNetwork>(), 56);
        assert_eq!(align_of::<CWifiNetwork>(), 4);
        assert_eq!(offset_of!(CWifiNetwork, ssid), 0);
        assert_eq!(offset_of!(CWifiNetwork, bssid), 33);
        assert_eq!(offset_of!(CWifiNetwork, signal_dbm), 40);
        assert_eq!(offset_of!(CWifiNetwork, frequency), 44);
        assert_eq!(offset_of!(CWifiNetwork, channel), 48);
        assert_eq!(offset_of!(CWifiNetwork, security), 49);
        assert_eq!(offset_of!(CWifiNetwork, bss_status), 50);
        assert_eq!(offset_of!(CWifiNetwork, beacon_interval), 52);
        assert_eq!(offset_of!(CWifiNetwork, _pad), 54);
    }
}
