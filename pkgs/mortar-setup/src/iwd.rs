// SPDX-License-Identifier: GPL-2.0-or-later
//! Wi-Fi through iwd's D-Bus API (net.connman.iwd): scan, list networks,
//! connect with a passphrase (we register an agent that answers iwd's
//! RequestPassphrase), and WPS push-button.
//!
//! A worker thread owns the bus connection; callers get a snapshot
//! (`WifiView`) and send commands. Long calls (Connect, PushButton) run on
//! their own threads so the snapshot stays fresh.

use crate::control::{NetView, WifiView};
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use zbus::blocking::{fdo::ObjectManagerProxy, Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};

const SVC: &str = "net.connman.iwd";
const STATION: &str = "net.connman.iwd.Station";
const NETWORK: &str = "net.connman.iwd.Network";
const DEVICE: &str = "net.connman.iwd.Device";
const WPS: &str = "net.connman.iwd.SimpleConfiguration";
const AGENT_PATH: &str = "/mortar/setup/agent";

const KNOWN: &str = "net.connman.iwd.KnownNetwork";
const DIAG: &str = "net.connman.iwd.StationDiagnostic";

enum Cmd {
    Scan,
    Connect { ssid: String, passphrase: Option<String> },
    Wps,
    Forget { ssid: String },
    Disconnect,
    /// Someone looks again after a quiet while: refresh now.
    Refresh,
}

#[derive(Clone)]
pub struct Wifi {
    view: Arc<Mutex<WifiView>>,
    tx: Sender<Cmd>,
    watched: Watched,
}

/// When the view was last asked for. The workers refresh every 2 s (1 s
/// while discovering) while someone looks (Settings pages, setup screens),
/// and once a minute otherwise: each refresh is a few D-Bus calls that
/// wake iwd or bluetoothd and the bus, and nobody reads them at idle.
#[derive(Clone)]
pub(crate) struct Watched(Arc<Mutex<Option<std::time::Instant>>>);

/// The view counts as watched this long after it was last asked for.
const WATCHED_FOR: Duration = Duration::from_secs(20);
/// How often the workers refresh when nobody looks.
pub(crate) const IDLE_REFRESH: Duration = Duration::from_secs(60);

impl Watched {
    pub(crate) fn new() -> Watched {
        Watched(Arc::new(Mutex::new(None)))
    }

    /// Note a look; true when it ends a quiet while (time to refresh).
    pub(crate) fn touch(&self) -> bool {
        let mut g = self.0.lock().unwrap();
        let quiet = g.is_none_or(|t| t.elapsed() >= WATCHED_FOR);
        *g = Some(std::time::Instant::now());
        quiet
    }

    /// How long a worker may wait for a command before refreshing.
    pub(crate) fn wait(&self, busy: Duration) -> Duration {
        match *self.0.lock().unwrap() {
            Some(t) if t.elapsed() < WATCHED_FOR => busy,
            _ => IDLE_REFRESH,
        }
    }
}

/// Passphrases waiting for iwd to ask, by network object path.
type Pending = Arc<Mutex<HashMap<String, String>>>;

struct Agent {
    pending: Pending,
}

#[zbus::interface(name = "net.connman.iwd.Agent")]
impl Agent {
    fn release(&self) {}

    fn request_passphrase(&self, network: ObjectPath<'_>) -> zbus::fdo::Result<String> {
        self.pending
            .lock()
            .unwrap()
            .get(network.as_str())
            .cloned()
            .ok_or_else(|| zbus::fdo::Error::Failed("no passphrase".into()))
    }

    fn request_private_key_passphrase(&self, _network: ObjectPath<'_>) -> zbus::fdo::Result<String> {
        Err(zbus::fdo::Error::NotSupported("not supported".into()))
    }

    fn request_user_name_and_password(&self, _network: ObjectPath<'_>) -> zbus::fdo::Result<(String, String)> {
        Err(zbus::fdo::Error::NotSupported("enterprise networks are not supported here".into()))
    }

    fn request_user_password(&self, _network: ObjectPath<'_>, _user: String) -> zbus::fdo::Result<String> {
        Err(zbus::fdo::Error::NotSupported("enterprise networks are not supported here".into()))
    }

    fn cancel(&self, _reason: String) {}
}

fn get<T: TryFrom<OwnedValue>>(props: &HashMap<String, OwnedValue>, k: &str) -> Option<T> {
    props.get(k).and_then(|v| v.try_clone().ok()).and_then(|v| T::try_from(v).ok())
}

fn bars(signal: i16) -> u8 {
    match signal {
        s if s >= -6000 => 4,
        s if s >= -6700 => 3,
        s if s >= -7500 => 2,
        s if s >= -8500 => 1,
        _ => 0,
    }
}

/// Words for iwd's errors.
fn explain(e: &zbus::Error) -> String {
    let s = e.to_string();
    if s.contains("Failed") || s.contains("InvalidFormat") {
        "Could not join. Check the password.".into()
    } else if s.contains("NotFound") {
        "That network is out of range.".into()
    } else if s.contains("Aborted") || s.contains("Canceled") {
        "Stopped.".into()
    } else if s.contains("NotSupported") {
        "This kind of network is not supported here.".into()
    } else if s.contains("Timeout") || s.contains("NoReply") {
        "No answer in time.".into()
    } else {
        s
    }
}

struct Snapshot {
    station: Option<OwnedObjectPath>,
    device: Option<OwnedObjectPath>,
    networks: HashMap<String, OwnedObjectPath>,
    /// Known-network objects, by SSID (what Forget is called on).
    known: HashMap<String, OwnedObjectPath>,
}

fn refresh(conn: &Connection, view: &Mutex<WifiView>) -> zbus::Result<Snapshot> {
    let om = ObjectManagerProxy::builder(conn).destination(SVC)?.path("/")?.build()?;
    let objs = om.get_managed_objects()?;
    let mut snap = Snapshot { station: None, device: None, networks: HashMap::new(), known: HashMap::new() };
    let mut v = WifiView { state: "off".into(), ..Default::default() };
    let mut net_props: HashMap<String, &HashMap<String, OwnedValue>> = HashMap::new();
    for (path, ifaces) in &objs {
        for (iface, props) in ifaces {
            match iface.as_str() {
                STATION => {
                    snap.station = Some(path.clone());
                    snap.device = Some(path.clone());
                    v.available = true;
                    v.state = get::<String>(props, "State").unwrap_or_else(|| "disconnected".into());
                    v.scanning = get::<bool>(props, "Scanning").unwrap_or(false);
                }
                NETWORK => {
                    net_props.insert(path.as_str().to_string(), props);
                }
                DEVICE if snap.device.is_none() => {
                    snap.device = Some(path.clone());
                }
                _ => {}
            }
        }
    }
    if let Some(st) = &snap.station {
        let p = Proxy::new(conn, SVC, st.as_str(), STATION)?;
        let ordered: Vec<(OwnedObjectPath, i16)> = p.call("GetOrderedNetworks", &())?;
        for (np, sig) in ordered {
            let Some(props) = net_props.get(np.as_str()) else { continue };
            let ssid: String = get(props, "Name").unwrap_or_default();
            let connected = get::<bool>(props, "Connected").unwrap_or(false);
            if connected {
                v.ssid = Some(ssid.clone());
            }
            if let Some(k) = get::<OwnedObjectPath>(props, "KnownNetwork") {
                snap.known.insert(ssid.clone(), k);
            }
            v.networks.push(NetView {
                ssid: ssid.clone(),
                bars: bars(sig),
                security: get(props, "Type").unwrap_or_else(|| "psk".into()),
                known: props.contains_key("KnownNetwork"),
                connected,
            });
            snap.networks.insert(ssid, np);
        }
    }
    v.address = crate::sys::addr_of("wlan0").map(|a| a.to_string());
    // the channel of the connected network (StationDiagnostic; older iwd
    // or no connection: none)
    if v.state == "connected"
        && let Some(st) = &snap.station
    {
        v.frequency = Proxy::new(conn, SVC, st.as_str(), DIAG)
            .and_then(|p| p.call::<_, _, HashMap<String, OwnedValue>>("GetDiagnostics", &()))
            .ok()
            .and_then(|d| get::<u32>(&d, "Frequency"));
    }
    let mut cur = view.lock().unwrap();
    v.last_error = cur.last_error.clone();
    v.wps = cur.wps;
    *cur = v;
    Ok(snap)
}

impl Wifi {
    /// Start the worker. Without iwd the view just says unavailable.
    pub fn start() -> Wifi {
        let view = Arc::new(Mutex::new(WifiView { state: "off".into(), ..Default::default() }));
        let (tx, rx) = channel();
        let v = view.clone();
        let watched = Watched::new();
        let w = watched.clone();
        thread::Builder::new()
            .name("iwd".into())
            .spawn(move || worker(v, rx, w))
            .expect("spawn iwd worker");
        Wifi { view, tx, watched }
    }

    pub fn view(&self) -> WifiView {
        if self.watched.touch() {
            let _ = self.tx.send(Cmd::Refresh);
        }
        self.view.lock().unwrap().clone()
    }

    pub fn scan(&self) {
        let _ = self.tx.send(Cmd::Scan);
    }

    pub fn connect(&self, ssid: String, passphrase: Option<String>) {
        self.view.lock().unwrap().last_error = None;
        let _ = self.tx.send(Cmd::Connect { ssid, passphrase });
    }

    pub fn wps(&self) {
        self.view.lock().unwrap().last_error = None;
        let _ = self.tx.send(Cmd::Wps);
    }

    pub fn forget(&self, ssid: String) {
        self.view.lock().unwrap().last_error = None;
        let _ = self.tx.send(Cmd::Forget { ssid });
    }

    pub fn disconnect(&self) {
        let _ = self.tx.send(Cmd::Disconnect);
    }
}

fn worker(view: Arc<Mutex<WifiView>>, rx: Receiver<Cmd>, watched: Watched) {
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    loop {
        let conn = match Connection::system() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("iwd: no system bus: {e}");
                thread::sleep(Duration::from_secs(10));
                continue;
            }
        };
        if let Err(e) = conn.object_server().at(AGENT_PATH, Agent { pending: pending.clone() }) {
            eprintln!("iwd: agent: {e}");
        }
        let mut agent_registered = false;
        let mut snap = Snapshot { station: None, device: None, networks: HashMap::new(), known: HashMap::new() };
        loop {
            match refresh(&conn, &view) {
                Ok(s) => snap = s,
                Err(_) => {
                    // iwd not (yet) running; keep trying
                    let mut v = view.lock().unwrap();
                    v.available = false;
                    v.state = "off".into();
                    agent_registered = false;
                }
            }
            if !agent_registered && snap.station.is_some() {
                agent_registered = Proxy::new(&conn, SVC, "/net/connman/iwd", "net.connman.iwd.AgentManager")
                    .and_then(|p| p.call_method("RegisterAgent", &(ObjectPath::try_from(AGENT_PATH).unwrap(),)).map(|_| ()))
                    .map_err(|e| eprintln!("iwd: RegisterAgent: {e}"))
                    .is_ok();
            }
            let cmd = match rx.recv_timeout(watched.wait(Duration::from_secs(2))) {
                Ok(c) => c,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return,
            };
            match cmd {
                Cmd::Refresh => {}
                Cmd::Scan => {
                    if let Some(st) = &snap.station
                        && let Err(e) = Proxy::new(&conn, SVC, st.as_str(), STATION).and_then(|p| p.call_method("Scan", &()).map(|_| ()))
                        && !e.to_string().contains("Busy")
                    {
                        view.lock().unwrap().last_error = Some(explain(&e));
                    }
                }
                Cmd::Connect { ssid, passphrase } => {
                    let Some(np) = snap.networks.get(&ssid).cloned() else {
                        view.lock().unwrap().last_error = Some(format!("'{ssid}' is out of range."));
                        continue;
                    };
                    if let Some(pw) = passphrase {
                        pending.lock().unwrap().insert(np.as_str().to_string(), pw);
                    }
                    view.lock().unwrap().state = "connecting".into();
                    let (c, v, pend) = (conn.clone(), view.clone(), pending.clone());
                    thread::spawn(move || {
                        let r = Proxy::new(&c, SVC, np.as_str(), NETWORK).and_then(|p| p.call_method("Connect", &()).map(|_| ()));
                        pend.lock().unwrap().remove(np.as_str());
                        if let Err(e) = r {
                            v.lock().unwrap().last_error = Some(explain(&e));
                        }
                    });
                }
                Cmd::Forget { ssid } => {
                    let r = match snap.known.get(&ssid) {
                        Some(k) => Proxy::new(&conn, SVC, k.as_str(), KNOWN).and_then(|p| p.call_method("Forget", &()).map(|_| ())),
                        None => {
                            view.lock().unwrap().last_error = Some(format!("'{ssid}' is not a saved network."));
                            continue;
                        }
                    };
                    if let Err(e) = r {
                        view.lock().unwrap().last_error = Some(explain(&e));
                    }
                }
                Cmd::Disconnect => {
                    if let Some(st) = &snap.station
                        && let Err(e) = Proxy::new(&conn, SVC, st.as_str(), STATION).and_then(|p| p.call_method("Disconnect", &()).map(|_| ()))
                        && !e.to_string().contains("NotConnected")
                    {
                        view.lock().unwrap().last_error = Some(explain(&e));
                    }
                }
                Cmd::Wps => {
                    let Some(dev) = snap.device.clone() else { continue };
                    view.lock().unwrap().wps = true;
                    let (c, v) = (conn.clone(), view.clone());
                    thread::spawn(move || {
                        let r = Proxy::new(&c, SVC, dev.as_str(), WPS).and_then(|p| p.call_method("PushButton", &()).map(|_| ()));
                        let mut g = v.lock().unwrap();
                        g.wps = false;
                        if let Err(e) = r {
                            g.last_error = Some(if e.to_string().contains("WalkTimeExpired") || e.to_string().contains("Timeout") {
                                "No router answered. Press its WPS button, then try again.".into()
                            } else {
                                explain(&e)
                            });
                        }
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refreshes_slowly_until_someone_looks() {
        let w = Watched::new();
        assert_eq!(w.wait(Duration::from_secs(2)), IDLE_REFRESH);
        assert!(w.touch(), "the first look ends the quiet");
        assert!(!w.touch(), "a look right after does not");
        assert_eq!(w.wait(Duration::from_secs(2)), Duration::from_secs(2));
    }
}
