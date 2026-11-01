// SPDX-License-Identifier: GPL-2.0-or-later
//! Settings > Bluetooth: the adapter switch, paired devices (connect,
//! disconnect, forget) and devices in range (pair). Opening the page
//! starts a scan (the daemon stops it after a minute, or when B leaves).
//! Everything goes through mortar-setupd (bluez.rs); this only draws its
//! `BtView` and sends requests.

use super::pages::{note_line, signal, sub, t, Confirm};
use super::{Cx, Nav, Page};
use crate::control::{BtDeviceView, BtView, Request};
use mortar_ui::gfx::{Rect, Ui};
use mortar_ui::input::Button;
use mortar_ui::toolkit::{self, hint, ListState, PillKind, Row, RowValue};

/// A row: the power switch, or a device by address (so the selection stays
/// on the same device while the list changes).
#[derive(Clone, Debug, PartialEq)]
enum Item {
    Power,
    Device(String),
}

pub struct BtList {
    list: ListState,
    sel: Item,
    confirm: Confirm,
    /// The last `last_connected` we toasted.
    told: Option<String>,
}

impl BtList {
    pub fn new(cx: &mut Cx) -> BtList {
        if cx.st().bluetooth.powered {
            let _ = cx.dev.act(Request::BtScan { on: true });
        }
        BtList { list: ListState::default(), sel: Item::Power, confirm: Confirm::default(), told: None }
    }

    fn items(bt: &BtView) -> Vec<Item> {
        let mut v = vec![Item::Power];
        if bt.powered {
            v.extend(bt.devices.iter().map(|d| Item::Device(d.address.clone())));
        }
        v
    }

    /// Keep `list.sel` on `sel` (or the nearest row if it went away).
    fn sync(&mut self, items: &[Item]) {
        match items.iter().position(|i| *i == self.sel) {
            Some(i) => self.list.sel = i,
            None => {
                self.list.clamp(items.len());
                self.sel = items.get(self.list.sel).cloned().unwrap_or(Item::Power);
            }
        }
    }

    fn device<'a>(&self, bt: &'a BtView) -> Option<&'a BtDeviceView> {
        match &self.sel {
            Item::Device(a) => bt.devices.iter().find(|d| &d.address == a),
            Item::Power => None,
        }
    }
}

fn state_text(bt: &BtView, d: &BtDeviceView) -> String {
    if bt.busy.as_deref() == Some(d.address.as_str()) {
        return if d.paired { "Connecting…".into() } else { "Pairing…".into() };
    }
    if d.connected {
        "Connected".into()
    } else if d.paired {
        "Paired".into()
    } else {
        String::new()
    }
}

fn desc(d: &BtDeviceView) -> String {
    let mut s = d.kind_label().to_string();
    if let Some(b) = d.battery {
        s.push_str(&format!(" · battery {b}%"));
    }
    if !d.paired {
        s.push_str(" · A to pair");
    }
    s
}

/// dBm to 0..=4 bars.
fn bars(rssi: i16) -> u8 {
    match rssi {
        r if r >= -55 => 4,
        r if r >= -67 => 3,
        r if r >= -78 => 2,
        r if r >= -90 => 1,
        _ => 0,
    }
}

impl Page for BtList {
    fn live(&self) -> bool {
        true
    }

    fn draw(&mut self, ui: &mut Ui, cx: &Cx) {
        let bt = &cx.st().bluetooth;
        let items = BtList::items(bt);
        self.sync(&items);
        let dev = self.device(bt);
        let mut hints = Vec::new();
        if dev.is_some_and(|d| d.paired) {
            hints.push(hint("X", "Forget"));
        }
        if bt.powered {
            hints.push(hint("Y", if bt.discovering { "Stop" } else { "Scan" }));
        }
        hints.push(hint("B", "Back"));
        hints.push(hint(
            "A",
            match dev {
                None => "Toggle",
                Some(d) if d.connected => "Disconnect",
                Some(d) if d.paired => "Connect",
                Some(_) => "Pair",
            },
        ));
        let body = cx.frame(ui, false, &hints);
        let mut y = body.y + ui.cq(1.0);
        y += toolkit::crumb(ui, body.x, y, "Settings /");
        y += toolkit::title_h2(ui, body.x, y, "Bluetooth");
        y += ui.cq(0.4);
        let subst = sub(ui, 2.2);
        let line = if !bt.available {
            "Bluetooth is not available right now (bluetoothd is not running)."
        } else if !bt.powered {
            "Off. Turn it on for headphones and controllers."
        } else if bt.discovering {
            "Looking for devices… Put yours in pairing mode."
        } else {
            "Headphones and controllers. Y looks for new ones."
        };
        ui.text_fit(body.x, y, line, &subst, body.w);
        y += subst.line_px() + ui.cq(1.6);
        if let Some(e) = &bt.last_error {
            y += note_line(ui, body.x, y, body.w, e, PillKind::Orange) + ui.cq(1.0);
        } else if let Some(p) = &bt.passkey {
            y += note_line(ui, body.x, y, body.w, &format!("Code {p}. If the device shows it too, confirm it there."), PillKind::Amber) + ui.cq(1.0);
        } else if let Some(n) = &bt.last_connected {
            y += note_line(ui, body.x, y, body.w, &format!("{n} is connected."), PillKind::Ok) + ui.cq(1.0);
        }
        if !bt.available {
            return;
        }
        let rows: Vec<Row> = items
            .iter()
            .enumerate()
            .map(|(i, it)| {
                let mut r = match it {
                    Item::Power => Row::new("Bluetooth", RowValue::Toggle(bt.powered)),
                    Item::Device(a) => {
                        let d = bt.devices.iter().find(|d| &d.address == a).expect("listed");
                        let st = state_text(bt, d);
                        let mut r = Row::new(&d.name, if st.is_empty() { RowValue::None } else { RowValue::Text(st) });
                        r.desc = Some(desc(d));
                        r
                    }
                };
                r.selected = i == self.list.sel;
                r
            })
            .collect();
        let area = Rect::new(body.x, y, body.w, body.bottom() - y - ui.cq(1.0));
        toolkit::rows(ui, area, &rows, &mut self.list);
        // signal bars on devices in range that are not paired yet
        let mut ry = y;
        for (i, (it, r)) in items.iter().zip(&rows).enumerate().skip(self.list.scroll) {
            let rh = toolkit::row_h(ui, r.desc.is_some());
            if ry + rh > area.bottom() + 0.5 {
                break;
            }
            if let Item::Device(a) = it
                && let Some(d) = bt.devices.iter().find(|d| &d.address == a)
                && !d.paired
                && bt.busy.as_deref() != Some(a.as_str())
                && let Some(rssi) = d.rssi
            {
                let sel = i == self.list.sel;
                let ink = if sel { t().lime_ink } else { t().line };
                let faint = if sel { t().lime_ink.with_alpha(0.25) } else { t().line_faint };
                signal(ui, body.right() - ui.cq(2.4), ry + rh / 2.0, bars(rssi), ink, faint);
            }
            ry += rh;
        }
        if bt.powered && bt.devices.is_empty() {
            let ny = y + toolkit::row_h(ui, false) + ui.cq(1.6);
            note_line(ui, body.x, ny, body.w, "No devices yet. Put headphones or a controller in pairing mode.", PillKind::Amber);
        }
    }

    fn button(&mut self, b: Button, cx: &mut Cx) -> Nav {
        let bt = cx.st().bluetooth.clone();
        let items = BtList::items(&bt);
        self.sync(&items);
        match b {
            Button::Up | Button::Down => {
                if b == Button::Up {
                    self.list.up(items.len());
                } else {
                    self.list.down(items.len());
                }
                self.sel = items.get(self.list.sel).cloned().unwrap_or(Item::Power);
            }
            Button::Y if bt.powered => {
                cx.act(Request::BtScan { on: !bt.discovering });
            }
            Button::X => {
                if let Some(d) = self.device(&bt)
                    && d.paired
                    && self.confirm.ok(&d.address, cx, &format!("forget {}", d.name))
                {
                    cx.act(Request::BtForget { address: d.address.clone() });
                }
            }
            Button::B => {
                if bt.discovering {
                    let _ = cx.dev.act(Request::BtScan { on: false });
                }
                return Nav::Pop;
            }
            Button::A | Button::Start => match self.device(&bt) {
                None => {
                    if cx.act(Request::BtPower { on: !bt.powered }) && !bt.powered {
                        let _ = cx.dev.act(Request::BtScan { on: true });
                    }
                }
                Some(d) if d.connected => {
                    cx.act(Request::BtDisconnect { address: d.address.clone() });
                }
                Some(d) if d.paired => {
                    cx.act(Request::BtConnect { address: d.address.clone() });
                }
                Some(d) => {
                    cx.act(Request::BtPair { address: d.address.clone() });
                }
            },
            _ => {}
        }
        Nav::Stay
    }

    fn tick(&mut self, cx: &mut Cx) -> Nav {
        let n = cx.st().bluetooth.last_connected.clone();
        if n.is_some() && n != self.told {
            self.told = n.clone();
            if let Some(n) = n {
                cx.toast(format!("{n} is connected"));
            }
        }
        Nav::Stay
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::demo::DemoDevice;
    use crate::ui::{Mode, SetupUi};
    use mortar_ui::status::Sys;

    fn ui() -> SetupUi {
        SetupUi::named("bluetooth", Box::new(DemoDevice::settings()), Sys::demo(None), Mode::Settings).unwrap()
    }

    #[test]
    fn selection_follows_the_device() {
        let mut s = ui();
        s.button(Button::Down); // headphones (connected)
        s.button(Button::Down); // controller (paired)
        s.button(Button::A); // connect
        assert!(s.cx.st().bluetooth.devices.iter().any(|d| d.name == "Retro Pad Pro" && d.connected));
        s.button(Button::A); // now connected: disconnect
        assert!(s.cx.st().bluetooth.devices.iter().any(|d| d.name == "Retro Pad Pro" && !d.connected));
    }

    #[test]
    fn forget_needs_two_presses() {
        let mut s = ui();
        s.button(Button::Down);
        s.button(Button::X);
        assert_eq!(s.cx.st().bluetooth.devices.len(), 5);
        s.button(Button::X);
        assert_eq!(s.cx.st().bluetooth.devices.len(), 4);
    }

    #[test]
    fn pair_and_power() {
        let mut s = ui();
        for _ in 0..3 {
            s.button(Button::Down);
        }
        s.button(Button::A); // the first device in range
        assert!(s.cx.st().bluetooth.busy.is_some());
        let mut s = ui();
        s.button(Button::A); // power off
        assert!(!s.cx.st().bluetooth.powered);
        s.button(Button::B);
        assert!(s.exited());
    }
}
