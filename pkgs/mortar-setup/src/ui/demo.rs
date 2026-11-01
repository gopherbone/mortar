// SPDX-License-Identifier: GPL-2.0-or-later
//! A stand-in device with the plan mockups' values, for screenshots, the
//! desktop dev loop and tests. Actions change its state the way the daemon
//! would (roughly).

use super::Device;
use crate::control::{BtDeviceView, BtView, KeyView, NetView, PairingView, Request, SetupSummary, Status, WifiView};

pub struct DemoDevice {
    pub status: Status,
}

pub fn laptop_key() -> KeyView {
    KeyView {
        kind: "ed25519".into(),
        comment: "you@laptop".into(),
        fingerprint: "SHA256:dI8zU7wX0jq5Sg0mWbW1hTzE1o3lY+qS7m8nVb2cK4E".into(),
        source: "card".into(),
    }
}

pub fn desktop_key() -> KeyView {
    KeyView {
        kind: "ed25519".into(),
        comment: "you@desktop".into(),
        fingerprint: "SHA256:Lm2bq9R0yQe1kT5vV7x3sZ8hP4nJ6cW2aD9fG1uE0oA".into(),
        source: "card".into(),
    }
}

fn nets() -> Vec<NetView> {
    let n = |ssid: &str, bars: u8, security: &str, known: bool| NetView { ssid: ssid.into(), bars, security: security.into(), known, connected: false };
    vec![
        n("Gloria_5G", 4, "psk", false),
        n("Gloria-Guest", 3, "open", false),
        n("NETGEAR42", 2, "psk", false),
        n("DIRECT-7F-Printer", 2, "psk", false),
        n("Office", 1, "8021x", false),
    ]
}

/// Headphones and a controller paired, more in range (Settings > Bluetooth).
pub fn bluetooth() -> BtView {
    let d = |name: &str, address: &str, kind: &str, paired: bool, connected: bool, rssi: Option<i16>, battery: Option<u8>| BtDeviceView {
        name: name.into(),
        address: address.into(),
        kind: kind.into(),
        paired,
        connected,
        rssi,
        battery,
    };
    BtView {
        available: true,
        powered: true,
        discovering: true,
        devices: vec![
            d("Studio headphones", "38:18:4C:12:34:56", "headphones", true, true, Some(-48), Some(80)),
            d("Retro Pad Pro", "E4:17:D8:AB:CD:EF", "gamepad", true, false, None, None),
            d("Kitchen speaker", "00:1A:7D:DA:71:13", "speaker", false, false, Some(-71), None),
            d("Travel keyboard", "DC:2C:26:44:55:66", "keyboard", false, false, Some(-80), None),
            d("Wireless Controller", "A4:53:85:01:02:03", "gamepad", false, false, Some(-55), None),
        ],
        ..Default::default()
    }
}

impl DemoDevice {
    /// A Brick on its first boot, plugged in, no keys yet.
    pub fn first_boot() -> DemoDevice {
        DemoDevice {
            status: Status {
                hostname: "mortar".into(),
                setup_done: false,
                setup_file: Some(SetupSummary { has_content: false, path: "/boot/mortar.toml".into(), ..Default::default() }),
                pairing: Some(PairingView { code: "TIDE-4782".into(), expires_in_s: 14 * 60 + 32, tries_left: 10 }),
                pairing_locked: false,
                pairing_interfaces: vec!["usb0".into()],
                host_key_fingerprint: Some("SHA256:Qx3nB0r7KfUu2mZ9dH6sV1pLc4tYw5eRj8aGi3oNw8Ka".into()),
                keys: vec![],
                last_added: None,
                password_login: true,
                usb_network: true,
                ssh_server: true,
                usb_connected: true,
                usb_address: "10.55.0.1".into(),
                wifi: WifiView { available: true, state: "disconnected".into(), networks: nets(), ..Default::default() },
                bluetooth: BtView { available: true, powered: true, ..Default::default() },
            },
        }
    }

    /// Screen 2: the card had the mockup's mortar.toml.
    pub fn setup_file() -> DemoDevice {
        let mut d = DemoDevice::first_boot();
        let s = &mut d.status;
        s.hostname = "my-brick".into();
        s.setup_file = Some(SetupSummary {
            has_content: true,
            path: "/boot/mortar.toml".into(),
            hostname: Some("my-brick".into()),
            wifi_ssid: Some("Gloria_5G".into()),
            wifi_password_scrubbed: true,
            keys: vec!["you@laptop".into(), "you@desktop".into()],
            password_login: Some(false),
            usb_network: Some(true),
            warnings: vec![],
        });
        s.keys = vec![laptop_key(), desktop_key()];
        s.password_login = false;
        s.pairing = None;
        s.usb_connected = false;
        d
    }

    /// Screen 6: paired over USB, on Wi-Fi too.
    pub fn ready() -> DemoDevice {
        let mut d = DemoDevice::first_boot();
        let s = &mut d.status;
        s.hostname = "my-brick".into();
        s.keys = vec![laptop_key()];
        s.last_added = Some(laptop_key());
        s.password_login = false;
        s.pairing = None;
        s.wifi.state = "connected".into();
        s.wifi.ssid = Some("Gloria_5G".into());
        s.wifi.address = Some("192.168.1.42".into());
        s.wifi.networks[0].connected = true;
        s.wifi.networks[0].known = true;
        d
    }

    /// Screen 8: an everyday Brick with two keys.
    pub fn settings() -> DemoDevice {
        let mut d = DemoDevice::ready();
        d.status.setup_done = true;
        d.status.keys = vec![laptop_key(), desktop_key()];
        d.status.last_added = None;
        d.status.usb_connected = false;
        d.status.bluetooth = bluetooth();
        d
    }
}

impl Device for DemoDevice {
    fn status(&self) -> &Status {
        &self.status
    }
    fn refresh(&mut self) -> bool {
        false
    }
    fn act(&mut self, req: Request) -> Result<(), String> {
        let s = &mut self.status;
        match req {
            Request::NewCode => {
                s.pairing = Some(PairingView { code: "MOSS-0417".into(), expires_in_s: 15 * 60, tries_left: 10 });
                s.pairing_locked = false;
            }
            Request::EndPairing => s.pairing = None,
            Request::SetPasswordLogin { on } => s.password_login = on,
            Request::SetUsbNetwork { on } => s.usb_network = on,
            Request::SetSshServer { on } => s.ssh_server = on,
            Request::RemoveKey { fingerprint } => s.keys.retain(|k| k.fingerprint != fingerprint),
            Request::WifiConnect { ssid, .. } => {
                s.wifi.state = "connecting".into();
                s.wifi.ssid = Some(ssid);
            }
            Request::Finish => s.setup_done = true,
            Request::WifiScan | Request::Status => {}
            Request::WifiWps => s.wifi.wps = true,
            Request::CheckPassword { .. } => return Err("not here".into()),
            Request::BtPower { on } => {
                s.bluetooth.powered = on;
                if !on {
                    s.bluetooth.discovering = false;
                }
            }
            Request::BtScan { on } => s.bluetooth.discovering = on && s.bluetooth.powered,
            Request::BtPair { address } => {
                s.bluetooth.discovering = false;
                s.bluetooth.busy = Some(address);
            }
            Request::BtConnect { address } => {
                if let Some(d) = s.bluetooth.devices.iter_mut().find(|d| d.address == address) {
                    d.connected = true;
                    s.bluetooth.last_connected = Some(d.name.clone());
                }
            }
            Request::BtDisconnect { address } => {
                if let Some(d) = s.bluetooth.devices.iter_mut().find(|d| d.address == address) {
                    d.connected = false;
                }
            }
            Request::BtForget { address } => s.bluetooth.devices.retain(|d| d.address != address),
        }
        Ok(())
    }
}
