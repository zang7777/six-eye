use std::collections::HashMap;
use std::fs;

/// OUI (Organizationally Unique Identifier) lookup table.
///
/// Parses the IEEE OUI database from `assets/oui/oui.txt` and maps
/// the first 3 bytes of a MAC address to the manufacturer name.
pub struct OuiDatabase {
    entries: HashMap<[u8; 3], String>,
}

impl OuiDatabase {
    /// Load and parse the OUI text file.
    ///
    /// The file format has lines like:
    ///   28-6F-B9   (hex)    Nokia Shanghai Bell Co., Ltd.
    ///
    /// We look for lines containing "(hex)" and extract the prefix + vendor.
    pub fn load() -> Self {
        let path = "assets/oui/oui.txt";
        let entries = match fs::read_to_string(path) {
            Ok(contents) => parse_oui_file(&contents),
            Err(error) => {
                log::warn!("Could not load OUI database from {path}: {error}");
                HashMap::new()
            }
        };

        log::info!("OUI database loaded with {} vendor entries.", entries.len());
        OuiDatabase { entries }
    }

    /// Look up the vendor name for a BSSID like "AA:BB:CC:DD:EE:FF"
    /// or "AA-BB-CC-DD-EE-FF".
    pub fn lookup(&self, bssid: &str) -> Option<String> {
        let prefix = parse_mac_prefix(bssid)?;
        self.entries.get(&prefix).cloned()
    }
}

/// Parse the first 3 hex bytes from a MAC address string.
///
/// Accepts both "AA:BB:CC:..." and "AA-BB-CC:..." formats.
fn parse_mac_prefix(mac: &str) -> Option<[u8; 3]> {
    // Replace colons with dashes to normalize, then split
    let normalized = mac.replace(':', "-");
    let parts: Vec<&str> = normalized.split('-').collect();

    if parts.len() < 3 {
        return None;
    }

    let a = u8::from_str_radix(parts[0], 16).ok()?;
    let b = u8::from_str_radix(parts[1], 16).ok()?;
    let c = u8::from_str_radix(parts[2], 16).ok()?;

    Some([a, b, c])
}

/// Parse the entire OUI text file into a map of [u8; 3] -> vendor name.
fn parse_oui_file(contents: &str) -> HashMap<[u8; 3], String> {
    let mut map = HashMap::with_capacity(32_000);

    for line in contents.lines() {
        // We only care about lines containing "(hex)" — those have the
        // short vendor name right after the prefix.
        //
        // Format: "28-6F-B9   (hex)\t\tNokia Shanghai Bell Co., Ltd."
        if !line.contains("(hex)") {
            continue;
        }

        let Some((prefix_part, rest)) = line.split_once("(hex)") else {
            continue;
        };

        let prefix_str = prefix_part.trim();
        let vendor = rest.trim().to_string();

        if vendor.is_empty() {
            continue;
        }

        if let Some(prefix) = parse_mac_prefix(prefix_str) {
            map.insert(prefix, vendor);
        }
    }

    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mac_prefix() {
        assert_eq!(parse_mac_prefix("28-6F-B9"), Some([0x28, 0x6F, 0xB9]));
        assert_eq!(
            parse_mac_prefix("28:6F:B9:AA:BB:CC"),
            Some([0x28, 0x6F, 0xB9])
        );
        assert_eq!(parse_mac_prefix("bad"), None);
    }

    #[test]
    fn test_parse_oui_line() {
        let sample = "28-6F-B9   (hex)\t\tNokia Shanghai Bell Co., Ltd.\n";
        let map = parse_oui_file(sample);
        assert_eq!(
            map.get(&[0x28, 0x6F, 0xB9]),
            Some(&"Nokia Shanghai Bell Co., Ltd.".to_string())
        );
    }
}
