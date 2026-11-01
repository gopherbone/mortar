// SPDX-License-Identifier: GPL-2.0-or-later
//! Bluetooth through BlueZ's D-Bus API (org.bluez): adapter power,
//! discovery, pair (then trust and connect), connect, disconnect, forget.
//!
//! Like iwd.rs: a worker thread owns the system bus connection and keeps a
//! snapshot (`BtView`) fresh; callers send commands. Pair and connect run
//! on threads of their own (they take seconds), so the list keeps updating.
//!
//! We are BlueZ's default agent (org.bluez.Agent1), with the
//! "NoInputNoOutput" capability: headphones and controllers pair with
//! "just works", keyboards too (no passkey to type on a pad). Pairing
//! requests are only accepted for the device the person picked on the
//! Brick (`pairing`), never for one that asks by itself; services of paired
//! devices are allowed (they are trusted anyway). Discovery stops by itself
//! after `SCAN_SECS` (it slows the 2.4 GHz Wi-Fi on the shared XR829).
//!
//! Audio (A2DP) and controllers need nothing more here: PipeWire's BlueZ
//! plugin in the session picks up headphones, the kernel's HID drivers make
//! controllers evdev devices, which mortar-shell grabs and forwards like the
//! built-in pad.

use crate::control::{BtDeviceView, BtView};
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use zbus::blocking::{fdo::ObjectManagerProxy, Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

const SVC: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const BATTERY: &str = "org.bluez.Battery1";
const AGENT_PATH: &str = "/mortar/setup/bluetooth_agent";
const SCAN_SECS: u64 = 60;
/// Unpaired devices shown at most (discovery finds every beacon around).
const MAX_NEARBY: usize = 24;

enum Cmd {
    Power(bool),
    Scan(bool),
    Pair(String),
    Connect(String),
    Disconnect(String),
    Forget(String),
}

#[derive(Clone)]
pub struct Bluetooth {
    view: Arc<Mutex<BtView>>,
    tx: Sender<Cmd>,
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Rejected(String),
}

/// What the agent needs to know: which device the person is pairing, and
/// where to show a passkey.
struct Agent {
    pairing: Arc<Mutex<Option<String>>>,
    view: Arc<Mutex<BtView>>,
    paired: Arc<Mutex<Vec<String>>>,
}

impl Agent {
    fn ours(&self, dev: &ObjectPath<'_>) -> Result<(), AgentError> {
        if self.pairing.lock().unwrap().as_deref() == Some(dev.as_str()) {
            Ok(())
        } else {
            Err(AgentError::Rejected("not pairing with this device".into()))
        }
    }
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    fn release(&self) {}

    fn request_pin_code(&self, device: ObjectPath<'_>) -> Result<String, AgentError> {
        self.ours(&device)?;
        Ok("0000".into())
    }

    fn display_pin_code(&self, device: ObjectPath<'_>, pincode: String) -> Result<(), AgentError> {
        self.ours(&device)?;
        self.view.lock().unwrap().passkey = Some(pincode);
        Ok(())
    }

    fn request_passkey(&self, device: ObjectPath<'_>) -> Result<u32, AgentError> {
        self.ours(&device)?;
        Ok(0)
    }

    fn display_passkey(&self, device: ObjectPath<'_>, passkey: u32, _entered: u16) -> Result<(), AgentError> {
        self.ours(&device)?;
        self.view.lock().unwrap().passkey = Some(format!("{passkey:06}"));
        Ok(())
    }

    fn request_confirmation(&self, device: ObjectPath<'_>, passkey: u32) -> Result<(), AgentError> {
        self.ours(&device)?;
        self.view.lock().unwrap().passkey = Some(format!("{passkey:06}"));
        Ok(())
    }

    fn request_authorization(&self, device: ObjectPath<'_>) -> Result<(), AgentError> {
        self.ours(&device)
    }

    fn authorize_service(&self, device: ObjectPath<'_>, _uuid: String) -> Result<(), AgentError> {
        if self.paired.lock().unwrap().iter().any(|p| p == device.as_str()) {
            return Ok(());
        }
        self.ours(&device)
    }

    fn cancel(&self) {
        self.view.lock().unwrap().passkey = None;
    }
}

fn get<T: TryFrom<OwnedValue>>(props: &HashMap<String, OwnedValue>, k: &str) -> Option<T> {
    props.get(k).and_then(|v| v.try_clone().ok()).and_then(|v| T::try_from(v).ok())
}

/// BlueZ's Icon (freedesktop icon names), else the Class of Device.
pub fn kind(icon: Option<&str>, class: Option<u32>) -> &'static str {
    match icon.unwrap_or("") {
        "audio-headset" | "audio-headphones" => return "headphones",
        "audio-card" | "audio-speakers" => return "speaker",
        "input-gaming" => return "gamepad",
        "input-keyboard" => return "keyboard",
        "input-mouse" | "input-tablet" => return "mouse",
        "phone" => return "phone",
        "computer" => return "computer",
        _ => {}
    }
    let Some(c) = class else { return "" };
    match (c >> 8) & 0x1f {
        0x01 => "computer",
        0x02 => "phone",
        0x04 => match (c >> 2) & 0x3f {
            0x01 | 0x02 | 0x06 => "headphones",
            _ => "speaker",
        },
        0x05 => match ((c >> 6) & 0x03, (c >> 2) & 0x0f) {
            (_, 0x01 | 0x02) => "gamepad",
            (0x01, _) => "keyboard",
            (0x02, _) => "mouse",
            _ => "",
        },
        _ => "",
    }
}

/// Words for BlueZ's errors.
fn explain(e: &zbus::Error) -> String {
    let s = e.to_string();
    if s.contains("AuthenticationFailed") || s.contains("AuthenticationRejected") || s.contains("AuthenticationCanceled") {
        "Pairing failed. Put the device in pairing mode and try again.".into()
    } else if s.contains("AuthenticationTimeout") || s.contains("ConnectionAttemptFailed") || s.contains("page-timeout") || s.contains("Timeout") || s.contains("NoReply") {
        "No answer. Is it on, close by and in pairing mode?".into()
    } else if s.contains("InProgress") || s.contains("Busy") {
        "Busy with another device. Try again in a moment.".into()
    } else if s.contains("NotReady") {
        "Bluetooth is off.".into()
    } else if s.contains("DoesNotExist") || s.contains("UnknownObject") {
        "That device is gone. Scan again.".into()
    } else if s.contains("br-connection-profile-unavailable") || s.contains("NotAvailable") {
        "Connected, but it offers nothing the Brick can use.".into()
    } else {
        s
    }
}

/// Paired devices (connected first), then the strongest `MAX_NEARBY` in
/// range; each group by name, so rows don't jump while signals change.
fn order(list: Vec<BtDeviceView>) -> Vec<BtDeviceView> {
    let by_name = |a: &BtDeviceView, b: &BtDeviceView| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.address.cmp(&b.address));
    let (mut paired, mut nearby): (Vec<_>, Vec<_>) = list.into_iter().partition(|d| d.paired);
    paired.sort_by(|a, b| b.connected.cmp(&a.connected).then_with(|| by_name(a, b)));
    nearby.sort_by_key(|d| std::cmp::Reverse(d.rssi.unwrap_or(i16::MIN)));
    nearby.truncate(MAX_NEARBY);
    nearby.sort_by(by_name);
    paired.extend(nearby);
    paired
}

struct Snapshot {
    adapter: Option<OwnedObjectPath>,
    devices: HashMap<String, OwnedObjectPath>,
}

fn refresh(conn: &Connection, view: &Mutex<BtView>, paired: &Mutex<Vec<String>>) -> zbus::Result<Snapshot> {
    let om = ObjectManagerProxy::builder(conn).destination(SVC)?.path("/")?.build()?;
    let objs = om.get_managed_objects()?;
    let mut snap = Snapshot { adapter: None, devices: HashMap::new() };
    let mut v = BtView::default();
    let mut list: Vec<BtDeviceView> = Vec::new();
    let mut paired_paths = Vec::new();
    for (path, ifaces) in &objs {
        if let Some(a) = ifaces.get(ADAPTER) {
            if snap.adapter.is_none() {
                snap.adapter = Some(path.clone());
                v.available = true;
                v.powered = get::<bool>(a, "Powered").unwrap_or(false);
                v.discovering = get::<bool>(a, "Discovering").unwrap_or(false);
            }
            continue;
        }
        let Some(d) = ifaces.get(DEVICE) else { continue };
        let address: String = get(d, "Address").unwrap_or_default();
        let name: Option<String> = get(d, "Name");
        let alias: Option<String> = get(d, "Alias");
        let p = get::<bool>(d, "Paired").unwrap_or(false);
        // nameless beacons and LE gadgets clutter the list
        if !p && name.is_none() {
            continue;
        }
        if p {
            paired_paths.push(path.as_str().to_string());
        }
        let icon: Option<String> = get(d, "Icon");
        let class: Option<u32> = get(d, "Class");
        list.push(BtDeviceView {
            name: alias.or(name).unwrap_or_else(|| address.clone()),
            kind: kind(icon.as_deref(), class).into(),
            paired: p,
            connected: get::<bool>(d, "Connected").unwrap_or(false),
            rssi: get::<i16>(d, "RSSI"),
            battery: ifaces.get(BATTERY).and_then(|b| get::<u8>(b, "Percentage")),
            address: address.clone(),
        });
        snap.devices.insert(address, path.clone());
    }
    v.devices = order(list);
    *paired.lock().unwrap() = paired_paths;
    let mut cur = view.lock().unwrap();
    v.busy = cur.busy.clone();
    v.passkey = cur.passkey.clone();
    v.last_error = cur.last_error.clone();
    v.last_connected = cur.last_connected.clone();
    *cur = v;
    Ok(snap)
}

impl Bluetooth {
    /// Start the worker. Without bluetoothd the view just says unavailable.
    pub fn start() -> Bluetooth {
        let view = Arc::new(Mutex::new(BtView::default()));
        let (tx, rx) = channel();
        let v = view.clone();
        thread::Builder::new()
            .name("bluez".into())
            .spawn(move || worker(v, rx))
            .expect("spawn bluez worker");
        Bluetooth { view, tx }
    }

    pub fn view(&self) -> BtView {
        self.view.lock().unwrap().clone()
    }

    fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }

    pub fn power(&self, on: bool) {
        self.send(Cmd::Power(on));
    }
    pub fn scan(&self, on: bool) {
        self.send(Cmd::Scan(on));
    }
    pub fn pair(&self, addr: String) {
        self.clear();
        self.send(Cmd::Pair(addr));
    }
    pub fn connect(&self, addr: String) {
        self.clear();
        self.send(Cmd::Connect(addr));
    }
    pub fn disconnect(&self, addr: String) {
        self.clear();
        self.send(Cmd::Disconnect(addr));
    }
    pub fn forget(&self, addr: String) {
        self.clear();
        self.send(Cmd::Forget(addr));
    }
    fn clear(&self) {
        let mut v = self.view.lock().unwrap();
        v.last_error = None;
        v.last_connected = None;
    }
}

fn dev_proxy<'a>(conn: &'a Connection, path: &'a OwnedObjectPath) -> zbus::Result<Proxy<'a>> {
    Proxy::new(conn, SVC, path.as_str(), DEVICE)
}

/// Pair (unless paired), trust, connect. Runs on its own thread.
fn pair_and_connect(conn: Connection, path: OwnedObjectPath, pair: bool, view: Arc<Mutex<BtView>>, pairing: Arc<Mutex<Option<String>>>) {
    let r = (|| -> zbus::Result<()> {
        let p = dev_proxy(&conn, &path)?;
        if pair {
            *pairing.lock().unwrap() = Some(path.as_str().to_string());
            let r = p.call_method("Pair", &());
            *pairing.lock().unwrap() = None;
            match r {
                Ok(_) => {}
                Err(e) if e.to_string().contains("AlreadyExists") => {}
                Err(e) => return Err(e),
            }
        }
        p.set_property("Trusted", true).map_err(zbus::Error::from)?;
        p.call_method("Connect", &())?;
        Ok(())
    })();
    let name = dev_proxy(&conn, &path).ok().and_then(|p| p.get_property::<String>("Alias").ok());
    let mut v = view.lock().unwrap();
    v.busy = None;
    v.passkey = None;
    match r {
        Ok(()) => v.last_connected = name.or_else(|| Some("the device".into())),
        Err(e) => {
            eprintln!("bluez: {}: {e}", path.as_str());
            v.last_error = Some(explain(&e));
        }
    }
}

fn worker(view: Arc<Mutex<BtView>>, rx: Receiver<Cmd>) {
    let pairing: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let paired: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    loop {
        let conn = match Connection::system() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("bluez: no system bus: {e}");
                thread::sleep(Duration::from_secs(10));
                continue;
            }
        };
        let agent = Agent { pairing: pairing.clone(), view: view.clone(), paired: paired.clone() };
        if let Err(e) = conn.object_server().at(AGENT_PATH, agent) {
            eprintln!("bluez: agent: {e}");
        }
        let mut agent_registered = false;
        let mut snap = Snapshot { adapter: None, devices: HashMap::new() };
        let mut scan_until: Option<Instant> = None;
        loop {
            match refresh(&conn, &view, &paired) {
                Ok(s) => snap = s,
                Err(_) => {
                    // bluetoothd not (yet) running, or restarted
                    let mut v = view.lock().unwrap();
                    v.available = false;
                    v.powered = false;
                    agent_registered = false;
                }
            }
            if !agent_registered && snap.adapter.is_some() {
                agent_registered = Proxy::new(&conn, SVC, "/org/bluez", "org.bluez.AgentManager1")
                    .and_then(|p| {
                        let path = ObjectPath::try_from(AGENT_PATH).unwrap();
                        p.call_method("RegisterAgent", &(&path, "NoInputNoOutput"))?;
                        p.call_method("RequestDefaultAgent", &(&path,)).map(|_| ())
                    })
                    .map_err(|e| eprintln!("bluez: RegisterAgent: {e}"))
                    .is_ok();
            }
            let adapter = snap.adapter.clone();
            let ad = |m: &str| -> zbus::Result<()> {
                let a = adapter.as_ref().ok_or(zbus::Error::Failure("no Bluetooth adapter".into()))?;
                Proxy::new(&conn, SVC, a.as_str(), ADAPTER)?.call_method(m, &()).map(|_| ())
            };
            if scan_until.is_some_and(|t| Instant::now() >= t) {
                scan_until = None;
                let _ = ad("StopDiscovery");
            }
            let discovering = view.lock().unwrap().discovering;
            let wait = if discovering { 1 } else { 2 };
            let cmd = match rx.recv_timeout(Duration::from_secs(wait)) {
                Ok(c) => c,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return,
            };
            let fail = |e: zbus::Error| {
                eprintln!("bluez: {e}");
                view.lock().unwrap().last_error = Some(explain(&e));
            };
            let device = |addr: &str| {
                let p = snap.devices.get(addr).cloned();
                if p.is_none() {
                    view.lock().unwrap().last_error = Some("That device is gone. Scan again.".into());
                }
                p
            };
            match cmd {
                Cmd::Power(on) => {
                    let r = adapter
                        .as_ref()
                        .ok_or(zbus::Error::Failure("no Bluetooth adapter".into()))
                        .and_then(|a| Proxy::new(&conn, SVC, a.as_str(), ADAPTER))
                        .and_then(|p| p.set_property("Powered", on).map_err(zbus::Error::from));
                    if let Err(e) = r {
                        fail(e);
                    }
                    if !on {
                        scan_until = None;
                    }
                }
                Cmd::Scan(true) => {
                    if let Some(a) = &adapter
                        && let Ok(p) = Proxy::new(&conn, SVC, a.as_str(), ADAPTER)
                    {
                        let filter: HashMap<&str, Value> = HashMap::from([("Transport", Value::from("auto"))]);
                        let _ = p.call_method("SetDiscoveryFilter", &(filter,));
                    }
                    match ad("StartDiscovery") {
                        Ok(()) => {}
                        Err(e) if e.to_string().contains("InProgress") => {}
                        Err(e) => fail(e),
                    }
                    scan_until = Some(Instant::now() + Duration::from_secs(SCAN_SECS));
                }
                Cmd::Scan(false) => {
                    scan_until = None;
                    let _ = ad("StopDiscovery");
                }
                Cmd::Pair(addr) | Cmd::Connect(addr) if view.lock().unwrap().busy.is_some() => {
                    let _ = addr;
                    view.lock().unwrap().last_error = Some("Busy with another device. Try again in a moment.".into());
                }
                Cmd::Pair(addr) => {
                    let Some(path) = device(&addr) else { continue };
                    // discovery makes pairing slow and flaky; it is done anyway
                    scan_until = None;
                    let _ = ad("StopDiscovery");
                    view.lock().unwrap().busy = Some(addr);
                    let (c, v, pg) = (conn.clone(), view.clone(), pairing.clone());
                    thread::spawn(move || pair_and_connect(c, path, true, v, pg));
                }
                Cmd::Connect(addr) => {
                    let Some(path) = device(&addr) else { continue };
                    view.lock().unwrap().busy = Some(addr);
                    let (c, v, pg) = (conn.clone(), view.clone(), pairing.clone());
                    thread::spawn(move || pair_and_connect(c, path, false, v, pg));
                }
                Cmd::Disconnect(addr) => {
                    let Some(path) = device(&addr) else { continue };
                    if let Err(e) = dev_proxy(&conn, &path).and_then(|p| p.call_method("Disconnect", &()).map(|_| ())) {
                        fail(e);
                    }
                }
                Cmd::Forget(addr) => {
                    let Some(path) = device(&addr) else { continue };
                    let r = adapter
                        .as_ref()
                        .ok_or(zbus::Error::Failure("no Bluetooth adapter".into()))
                        .and_then(|a| Proxy::new(&conn, SVC, a.as_str(), ADAPTER))
                        .and_then(|p| p.call_method("RemoveDevice", &(&path,)).map(|_| ()));
                    if let Err(e) = r {
                        fail(e);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::kind;

    #[test]
    fn kinds() {
        assert_eq!(kind(Some("audio-headset"), None), "headphones");
        assert_eq!(kind(Some("input-gaming"), None), "gamepad");
        // Class of Device: audio/video, headphones
        assert_eq!(kind(None, Some(0x240418)), "headphones");
        // peripheral, gamepad (DualShock 4: 0x002508)
        assert_eq!(kind(None, Some(0x002508)), "gamepad");
        // peripheral, keyboard
        assert_eq!(kind(None, Some(0x000540)), "keyboard");
        assert_eq!(kind(None, None), "");
    }
}
