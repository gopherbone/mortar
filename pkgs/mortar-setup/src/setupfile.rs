// SPDX-License-Identifier: GPL-2.0-or-later
//! `mortar.toml`: the setup file on the card's FAT boot partition, the way
//! Raspberry Pi images take Wi-Fi and keys before first boot. Format:
//! docs/setup-file.md. Parsing keeps the document (comments, layout) so the
//! Wi-Fi password can be blanked in place after it is stored.

use toml_edit::{DocumentMut, Item, Value};

/// The file as shipped on a fresh image: everything blank, so a Brick that
/// boots with it shows the Welcome screen.
pub const TEMPLATE: &str = include_str!("../mortar.toml");

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wifi {
    pub ssid: String,
    pub password: String,
    pub country: String,
    pub hidden: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SetupFile {
    pub hostname: Option<String>,
    pub wifi: Option<Wifi>,
    pub authorized_keys: Vec<String>,
    pub password_login: Option<bool>,
    pub usb_network: Option<bool>,
    /// Keys and values that were ignored, for people.
    pub warnings: Vec<String>,
}

fn str_at<'a>(doc: &'a DocumentMut, table: Option<&str>, key: &str) -> Option<&'a str> {
    let item = match table {
        Some(t) => doc.get(t)?.get(key)?,
        None => doc.get(key)?,
    };
    item.as_str().map(str::trim).filter(|s| !s.is_empty())
}

fn bool_at(doc: &DocumentMut, table: &str, key: &str, warnings: &mut Vec<String>) -> Option<bool> {
    let item = doc.get(table)?.get(key)?;
    match item.as_bool() {
        Some(b) => Some(b),
        None => {
            warnings.push(format!("[{table}] {key} must be true or false"));
            None
        }
    }
}

/// RFC 1123 label: what avahi and DHCP accept.
pub fn valid_hostname(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 63
        && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !h.starts_with('-')
        && !h.ends_with('-')
}

impl SetupFile {
    pub fn parse(text: &str) -> Result<SetupFile, String> {
        let doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
        let mut f = SetupFile::default();
        if let Some(h) = str_at(&doc, None, "hostname") {
            let h = h.trim_end_matches(".local").to_ascii_lowercase();
            if valid_hostname(&h) {
                f.hostname = Some(h);
            } else {
                f.warnings.push(format!("hostname '{h}' is not valid (letters, digits, dashes)"));
            }
        }
        if let Some(ssid) = str_at(&doc, Some("wifi"), "ssid") {
            let password = doc.get("wifi").and_then(|w| w.get("password")).and_then(|p| p.as_str()).unwrap_or("").to_string();
            if !password.is_empty() && !(8..=63).contains(&password.len()) {
                f.warnings.push("Wi-Fi password must be 8 to 63 characters".into());
            }
            let country = str_at(&doc, Some("wifi"), "country").unwrap_or("").to_ascii_uppercase();
            let country = if country.len() == 2 && country.chars().all(|c| c.is_ascii_uppercase()) {
                country
            } else {
                if !country.is_empty() {
                    f.warnings.push(format!("Wi-Fi country '{country}' is not a two-letter code"));
                }
                String::new()
            };
            let hidden = doc.get("wifi").and_then(|w| w.get("hidden")).and_then(|h| h.as_bool()).unwrap_or(false);
            if ssid.len() > 32 {
                f.warnings.push("Wi-Fi name is longer than 32 bytes".into());
            } else {
                f.wifi = Some(Wifi { ssid: ssid.to_string(), password, country, hidden });
            }
        } else if let Some(c) = str_at(&doc, Some("wifi"), "country") {
            // country alone is still useful
            let c = c.to_ascii_uppercase();
            if c.len() == 2 {
                f.wifi = Some(Wifi { country: c, ..Default::default() });
            }
        }
        if let Some(keys) = doc.get("ssh").and_then(|s| s.get("authorized_keys")) {
            match keys.as_array() {
                Some(arr) => {
                    for v in arr.iter() {
                        match v.as_str() {
                            Some(s) if !s.trim().is_empty() => f.authorized_keys.push(s.trim().to_string()),
                            Some(_) => {}
                            None => f.warnings.push("[ssh] authorized_keys must be a list of strings".into()),
                        }
                    }
                }
                None => match keys.as_str() {
                    // a single key pasted as a string is fine too
                    Some(s) if !s.trim().is_empty() => f.authorized_keys.push(s.trim().to_string()),
                    _ => f.warnings.push("[ssh] authorized_keys must be a list of strings".into()),
                },
            }
        }
        let mut w = Vec::new();
        f.password_login = bool_at(&doc, "ssh", "password_login", &mut w);
        f.usb_network = bool_at(&doc, "usb", "network", &mut w);
        f.warnings.extend(w);
        Ok(f)
    }

    /// Anything that would change the Brick (the blank template has none).
    /// Only explicit `false` booleans count: the template's defaults don't.
    pub fn has_content(&self) -> bool {
        self.hostname.is_some()
            || self.wifi.as_ref().is_some_and(|w| !w.ssid.is_empty())
            || !self.authorized_keys.is_empty()
            || self.password_login == Some(false)
            || self.usb_network == Some(false)
    }
}

/// Blank the Wi-Fi password in the file text, keeping everything else
/// (comments, order, other values). Returns None if there was nothing to
/// blank.
pub fn scrub_password(text: &str) -> Option<String> {
    let mut doc: DocumentMut = text.parse().ok()?;
    let wifi = doc.get_mut("wifi")?;
    let pw = wifi.get_mut("password")?;
    if pw.as_str().is_none_or(|s| s.is_empty()) {
        return None;
    }
    let mut v = Value::from("");
    // keep the key's position; say what happened on the same line
    let suffix = "  # saved on the Brick and removed from this file";
    v.decor_mut().set_suffix(suffix);
    *pw = Item::Value(v);
    Some(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOCKUP: &str = r#"# Edit before first boot. Leave anything blank to set it on the Brick.
hostname = "my-brick"

[wifi]
ssid = "Gloria_5G"
password = "correct horse battery" # removed after first boot
country = "US"

[ssh]
# Paste keys here, or drop .pub files into MORTAR/ssh-keys/
authorized_keys = [
  "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl you@laptop",
]
password_login = false

[usb]
network = true # Brick shows up as a network adapter
"#;

    #[test]
    fn template_is_empty() {
        let f = SetupFile::parse(TEMPLATE).unwrap();
        assert!(!f.has_content(), "{f:?}");
        assert!(f.warnings.is_empty(), "{:?}", f.warnings);
        assert_eq!(f.password_login, Some(true));
        assert_eq!(f.usb_network, Some(true));
    }

    #[test]
    fn mockup_file() {
        let f = SetupFile::parse(MOCKUP).unwrap();
        assert!(f.has_content());
        assert_eq!(f.hostname.as_deref(), Some("my-brick"));
        let w = f.wifi.unwrap();
        assert_eq!((w.ssid.as_str(), w.password.as_str(), w.country.as_str()), ("Gloria_5G", "correct horse battery", "US"));
        assert_eq!(f.authorized_keys.len(), 1);
        assert_eq!(f.password_login, Some(false));
        assert_eq!(f.usb_network, Some(true));
    }

    #[test]
    fn scrub_keeps_the_rest() {
        let out = scrub_password(MOCKUP).unwrap();
        assert!(!out.contains("correct horse"));
        assert!(out.contains("password = \"\"  # saved on the Brick"));
        assert!(out.contains("# Paste keys here"));
        assert!(out.contains("network = true # Brick shows up"));
        let f = SetupFile::parse(&out).unwrap();
        assert_eq!(f.wifi.unwrap().password, "");
        assert!(scrub_password(&out).is_none(), "second scrub is a no-op");
        assert!(scrub_password(TEMPLATE).is_none());
    }

    #[test]
    fn bad_values_warn() {
        let f = SetupFile::parse("hostname = \"no spaces\"\n[wifi]\nssid=\"x\"\npassword=\"short\"\ncountry=\"usa\"\n[usb]\nnetwork=\"yes\"\n").unwrap();
        assert!(f.hostname.is_none());
        assert_eq!(f.warnings.len(), 4, "{:?}", f.warnings);
        assert!(SetupFile::parse("[wifi\n").is_err());
        let f = SetupFile::parse("hostname = \"My-Brick.local\"\n").unwrap();
        assert_eq!(f.hostname.as_deref(), Some("my-brick"));
    }
}
