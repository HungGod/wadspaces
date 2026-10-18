//! The Devices menu: what's plugged into the machine and which wadspaces
//! have it (a chip per wadspace, on or off: a wadspace that isn't running
//! gets it when it starts), what works in every wadspace already, and
//! Bluetooth (on or off, looking for devices, pair, connect, forget).
//!
//! Its questions come up by themselves, over everything: where a device
//! plugged in for the first time goes, what Bluetooth pairing asks (does
//! the device show this code? type this code on it; its PIN), and whether
//! one of the owner's devices on the tailnet may manage the machine.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use gtk::prelude::*;
use gtk4_layer_shell::KeyboardMode;
use wad_proto::v1::{BluetoothAsk, BluetoothDevice, BluetoothStatus, Device, DeviceKind, TailnetAsk, TailnetStatus};

use crate::model::Space;
use crate::wadd::Wadd;
use crate::{Place, background, button, dimmed, icon, label};

/// An Adwaita symbolic icon for a device on the tailnet, by its OS.
pub fn os_icon(os: &str) -> &'static str {
    match os.to_ascii_lowercase().as_str() {
        "android" | "ios" => "phone-symbolic",
        _ => "computer-symbolic",
    }
}

/// An Adwaita symbolic icon for a kind of device.
pub fn kind_icon(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Gamepad => "input-gaming-symbolic",
        DeviceKind::Webcam => "camera-web-symbolic",
        DeviceKind::SecurityKey => "auth-smartcard-symbolic",
        DeviceKind::Serial => "modem-symbolic",
        DeviceKind::Keyboard => "input-keyboard-symbolic",
        DeviceKind::Mouse => "input-mouse-symbolic",
        DeviceKind::Audio => "audio-headphones-symbolic",
        DeviceKind::Storage => "drive-removable-media-symbolic",
        DeviceKind::Other => "media-removable-symbolic",
    }
}

/// Where a device is, in words.
pub fn whereabouts(d: &Device, names: &dyn Fn(&str) -> String) -> String {
    if d.ejected {
        return "Ejected · safe to unplug".into();
    }
    if !d.attachable {
        return if d.present { "Works in every wadspace".into() } else { "Not connected".into() };
    }
    let list = |ids: &[String]| ids.iter().map(|i| names(i)).collect::<Vec<_>>().join(", ");
    match (d.present, d.attached.is_empty(), d.remembered.is_empty()) {
        (false, _, true) => "Not plugged in".into(),
        (false, _, false) => format!("Not plugged in · goes to {}", list(&d.remembered)),
        (true, false, _) if d.kind == DeviceKind::Storage => format!("In {} · ~/Drives", list(&d.attached)),
        (true, false, _) => format!("In {}", list(&d.attached)),
        (true, true, false) => format!("Goes to {} when it starts", list(&d.remembered)),
        (true, true, true) => "On the machine".into(),
    }
}

/// What a pairing question says.
pub fn question(ask: BluetoothAsk, name: &str, code: Option<&str>) -> String {
    let code = code.unwrap_or("");
    match ask {
        BluetoothAsk::Confirm => format!("Does {name} show {code}?"),
        BluetoothAsk::Display => format!("Type {code} on {name}, then press Enter."),
        BluetoothAsk::Pin => format!("{name} wants a PIN (often 0000 or 1234)."),
        BluetoothAsk::Authorize => format!("Let {name} connect?"),
    }
}

pub struct Panel {
    wadd: Wadd,
    pub win: gtk::Window,
    list: gtk::Box,
    everywhere_head: gtk::Label,
    everywhere: gtk::Box,
    bt_box: gtk::Box,
    bt_power: gtk::Switch,
    bt_scan: gtk::Button,
    bt_list: gtk::Box,
    error: gtk::Label,
    pub ask_win: gtk::Window,
    ask_card: gtk::Box,
    devices: RefCell<Vec<Device>>,
    bt: RefCell<BluetoothStatus>,
    /// Devices on the tailnet asking to manage the machine.
    tailnet_asks: RefCell<Vec<TailnetAsk>>,
    /// Native wadspaces: (id, name).
    spaces: RefCell<Vec<(String, String)>>,
    running: RefCell<HashSet<String>>,
    /// New devices asked about (or waved off) since the HUD started.
    asked: RefCell<HashSet<String>>,
    /// What the question card is showing: "bt:<prompt id>", "tn:<device
    /// id>" or "dev:<key>".
    asking: RefCell<Option<String>>,
    syncing: Cell<bool>,
}

impl Panel {
    pub fn build(app: &gtk::Application, wadd: Wadd) -> Rc<Panel> {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
        card.set_size_request(460, -1);
        let title = label("Devices", "title");
        title.set_halign(gtk::Align::Start);
        card.append(&title);
        let sub = label("Pick the wadspaces each one goes to. Drives are at ~/Drives.", "sub");
        sub.set_wrap(true);
        sub.set_xalign(0.0);
        card.append(&sub);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let everywhere_head = label("WORKS IN EVERY WADSPACE", "section");
        everywhere_head.set_halign(gtk::Align::Start);
        let everywhere = gtk::Box::new(gtk::Orientation::Vertical, 4);

        let bt_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let section = label("BLUETOOTH", "section");
        section.set_hexpand(true);
        section.set_halign(gtk::Align::Start);
        let bt_scan = button("Find devices", &["small-button"]);
        let bt_power = gtk::Switch::new();
        bt_power.set_valign(gtk::Align::Center);
        bt_power.set_margin_end(6);
        head.append(&section);
        head.append(&bt_scan);
        head.append(&bt_power);
        bt_box.append(&head);
        let bt_list = gtk::Box::new(gtk::Orientation::Vertical, 4);
        bt_box.append(&bt_list);

        let inner = gtk::Box::new(gtk::Orientation::Vertical, 10);
        inner.append(&list);
        inner.append(&everywhere_head);
        inner.append(&everywhere);
        inner.append(&bt_box);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .max_content_height(560)
            .propagate_natural_height(true)
            .child(&inner)
            .build();
        card.append(&scroller);
        let error = label("", "error");
        error.set_wrap(true);
        error.set_xalign(0.0);
        error.set_visible(false);
        card.append(&error);
        let close = button("Done", &["menu-button", "quiet"]);
        card.append(&close);
        let (win, dim) =
            dimmed(app, "wadspaces-devices", "wadspaces-devices", KeyboardMode::None, &card, Place::Centre);

        // The question card: a new device, or what pairing asks.
        let ask_card = gtk::Box::new(gtk::Orientation::Vertical, 10);
        ask_card.set_size_request(400, -1);
        let (ask_win, _) = dimmed(
            app,
            "wadspaces-device-ask",
            "wadspaces-device-ask",
            KeyboardMode::OnDemand,
            &ask_card,
            Place::Centre,
        );

        let p = Rc::new(Panel {
            wadd,
            win,
            list,
            everywhere_head,
            everywhere,
            bt_box,
            bt_power,
            bt_scan,
            bt_list,
            error,
            ask_win,
            ask_card,
            devices: RefCell::default(),
            bt: RefCell::default(),
            tailnet_asks: RefCell::default(),
            spaces: RefCell::default(),
            running: RefCell::default(),
            asked: RefCell::default(),
            asking: RefCell::default(),
            syncing: Cell::new(false),
        });
        for b in [&close, &dim] {
            let p2 = p.clone();
            b.connect_clicked(move |_| p2.win.set_visible(false));
        }
        let p2 = p.clone();
        p.bt_power.connect_active_notify(move |s| {
            if p2.syncing.get() {
                return;
            }
            let (w, on, p3) = (p2.wadd.clone(), s.is_active(), p2.clone());
            background(move || w.bluetooth_switch("power", on), move |r| p3.done(r));
        });
        let p2 = p.clone();
        p.bt_scan.connect_clicked(move |_| {
            let on = !p2.bt.borrow().discovering;
            let (w, p3) = (p2.wadd.clone(), p2.clone());
            background(move || w.bluetooth_switch("scan", on), move |r| p3.done(r));
        });
        p
    }

    pub fn open(self: &Rc<Self>) {
        self.error.set_visible(false);
        self.fill();
        self.win.present();
        let (w, p) = (self.wadd.clone(), self.clone());
        background(
            move || (w.devices(), w.bluetooth()),
            move |(d, b)| {
                if let Ok(d) = d {
                    p.on_devices(d);
                }
                if let Ok(b) = b {
                    p.on_bluetooth(b);
                }
            },
        );
    }

    fn done(self: &Rc<Self>, r: Result<(), String>) {
        match r {
            Ok(()) => self.error.set_visible(false),
            Err(e) => {
                self.error.set_label(&e);
                self.error.set_visible(true);
            }
        }
    }

    fn name_of(&self, id: &str) -> String {
        self.spaces.borrow().iter().find(|(i, _)| i == id).map_or(id.to_string(), |(_, n)| n.clone())
    }

    pub fn on_devices(self: &Rc<Self>, list: Vec<Device>) {
        self.devices.replace(list);
        if self.win.is_visible() {
            self.fill();
        }
        self.ask_next();
    }

    pub fn on_bluetooth(self: &Rc<Self>, s: BluetoothStatus) {
        self.bt.replace(s);
        if self.win.is_visible() {
            self.fill_bluetooth();
        }
        self.ask_next();
    }

    /// The tailnet: a device asking to manage the machine is asked about.
    pub fn on_tailnet(self: &Rc<Self>, s: TailnetStatus) {
        self.tailnet_asks.replace(s.asks);
        self.ask_next();
    }

    /// The wadspaces (only native ones can have devices).
    pub fn on_spaces(self: &Rc<Self>, list: &[Space]) {
        let spaces = list
            .iter()
            .filter(|s| !s.is_app() && s.enabled && s.display.as_deref() != Some("stream"))
            .map(|s| (s.id.clone(), s.name.clone()))
            .collect();
        self.spaces.replace(spaces);
        if self.win.is_visible() {
            self.fill();
        }
    }

    pub fn on_state(self: &Rc<Self>, id: &str, running: bool) {
        let changed = if running {
            self.running.borrow_mut().insert(id.to_string())
        } else {
            self.running.borrow_mut().remove(id)
        };
        if changed && self.win.is_visible() {
            self.fill();
        }
    }

    fn clear(b: &gtk::Box) {
        while let Some(c) = b.first_child() {
            b.remove(&c);
        }
    }

    /// The wadspaces' chips for a device: on where it goes.
    fn chips(self: &Rc<Self>, d: &Device) -> gtk::FlowBox {
        let flow = gtk::FlowBox::new();
        flow.set_selection_mode(gtk::SelectionMode::None);
        flow.set_max_children_per_line(4);
        flow.set_column_spacing(4);
        flow.set_row_spacing(4);
        let spaces = self.spaces.borrow().clone();
        if spaces.is_empty() {
            flow.insert(&label("No wadspaces here yet.", "sub"), -1);
        }
        for (id, name) in spaces {
            let chip = gtk::ToggleButton::with_label(&name);
            chip.add_css_class("chip");
            if self.running.borrow().contains(&id) {
                chip.add_css_class("running");
            }
            chip.set_active(d.remembered.contains(&id));
            let (p, key) = (self.clone(), d.key.clone());
            chip.connect_toggled(move |c| {
                let (w, key, id, on, p2) = (p.wadd.clone(), key.clone(), id.clone(), c.is_active(), p.clone());
                background(move || w.device_to(&key, &id, on), move |r| p2.done(r));
            });
            flow.insert(&chip, -1);
        }
        flow
    }

    fn row(name: &str, kind: DeviceKind, sub: &str) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.add_css_class("device-row");
        row.append(&icon(kind_icon(kind), 18));
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        let n = label(name, "row-title");
        n.set_xalign(0.0);
        n.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let s = label(sub, "sub");
        s.set_xalign(0.0);
        s.set_ellipsize(gtk::pango::EllipsizeMode::End);
        text.append(&n);
        text.append(&s);
        row.append(&text);
        row
    }

    fn fill(self: &Rc<Self>) {
        Self::clear(&self.list);
        Self::clear(&self.everywhere);
        let devices = self.devices.borrow().clone();
        let names = |id: &str| self.name_of(id);
        let handed: Vec<&Device> = devices.iter().filter(|d| d.attachable).collect();
        if handed.is_empty() {
            let l = label(
                "Plug in a drive, gamepad, webcam, security key or other USB device to hand it to a wadspace.",
                "sub",
            );
            l.set_wrap(true);
            l.set_xalign(0.0);
            self.list.append(&l);
        }
        for d in handed {
            let block = gtk::Box::new(gtk::Orientation::Vertical, 4);
            let row = Self::row(&d.name, d.kind, &whereabouts(d, &names));
            if !d.present || d.ejected {
                row.add_css_class("dim-row");
            }
            // A drive: Eject, before it's pulled out.
            if d.kind == DeviceKind::Storage && d.present && !d.ejected {
                let eject = button("Eject", &["small-button"]);
                eject.set_valign(gtk::Align::Center);
                eject.set_tooltip_text(Some("Take it out of its wadspaces and turn it off, to unplug it safely"));
                let (p, key) = (self.clone(), d.key.clone());
                eject.connect_clicked(move |b| {
                    b.set_sensitive(false);
                    let (w, key, p2) = (p.wadd.clone(), key.clone(), p.clone());
                    background(move || w.device_eject(&key), move |r| p2.done(r));
                });
                row.append(&eject);
            }
            block.append(&row);
            if !d.ejected {
                block.append(&self.chips(d));
            }
            self.list.append(&block);
        }
        let shared: Vec<&Device> = devices.iter().filter(|d| !d.attachable && d.present).collect();
        self.everywhere_head.set_visible(!shared.is_empty());
        for d in shared {
            self.everywhere.append(&Self::row(&d.name, d.kind, &whereabouts(d, &names)));
        }
        self.fill_bluetooth();
    }

    fn fill_bluetooth(self: &Rc<Self>) {
        let bt = self.bt.borrow().clone();
        self.bt_box.set_visible(bt.available);
        self.syncing.set(true);
        self.bt_power.set_active(bt.powered);
        self.syncing.set(false);
        self.bt_scan.set_sensitive(bt.powered);
        self.bt_scan.set_label(if bt.discovering { "Stop looking" } else { "Find devices" });
        Self::clear(&self.bt_list);
        if !bt.powered {
            self.bt_list.append(&label("Bluetooth is off.", "sub"));
            return;
        }
        let nearby = bt.devices.iter().any(|d| !d.paired);
        if bt.devices.is_empty() || (!nearby && bt.discovering) {
            let l = label(
                if bt.discovering {
                    "Looking… put your device in pairing mode."
                } else {
                    "Nothing paired yet. Find devices to pair one."
                },
                "sub",
            );
            l.set_xalign(0.0);
            self.bt_list.append(&l);
        }
        for d in &bt.devices {
            self.bt_list.append(&self.bt_row(d));
        }
    }

    fn bt_row(self: &Rc<Self>, d: &BluetoothDevice) -> gtk::Box {
        let tag = if d.busy {
            "Working…"
        } else if d.connected {
            "Connected"
        } else if d.paired {
            "Paired"
        } else {
            "Nearby"
        };
        let row = Self::row(&d.name, d.kind, tag);
        let action = |text: &str, act: &'static str| {
            let b = button(text, &["small-button"]);
            b.set_valign(gtk::Align::Center);
            b.set_sensitive(!d.busy);
            let (p, addr) = (self.clone(), d.address.clone());
            b.connect_clicked(move |_| {
                let (w, addr, p2) = (p.wadd.clone(), addr.clone(), p.clone());
                background(move || w.bluetooth_device(&addr, act), move |r| p2.done(r));
            });
            b
        };
        if !d.paired {
            row.append(&action("Pair", "pair"));
        } else {
            row.append(&if d.connected { action("Disconnect", "disconnect") } else { action("Connect", "connect") });
            row.append(&action("Forget", "forget"));
        }
        row
    }

    /// Puts up the next question: pairing's first, then a device on the
    /// tailnet asking, then a new device's.
    fn ask_next(self: &Rc<Self>) {
        if let Some(prompt) = self.bt.borrow().prompt.clone() {
            let tag = format!("bt:{}", prompt.id);
            if self.asking.borrow().as_deref() != Some(&tag) {
                self.asking.replace(Some(tag));
                self.ask_bluetooth(prompt);
            }
            return;
        }
        let ask =
            self.tailnet_asks.borrow().iter().find(|a| !self.asked.borrow().contains(&format!("tn:{}", a.id))).cloned();
        if let Some(a) = ask {
            let tag = format!("tn:{}", a.id);
            if self.asking.borrow().as_deref() != Some(&tag) {
                self.asking.replace(Some(tag));
                self.ask_tailnet(a);
            }
            return;
        }
        let new =
            self.devices.borrow().iter().find(|d| d.new && d.present && !self.asked.borrow().contains(&d.key)).cloned();
        match new {
            Some(d) => {
                let tag = format!("dev:{}", d.key);
                if self.asking.borrow().as_deref() != Some(&tag) {
                    self.asking.replace(Some(tag));
                    self.ask_device(d);
                }
            }
            None => {
                self.asking.replace(None);
                self.ask_win.set_visible(false);
            }
        }
    }

    fn ask_head(&self, icon_name: &str, title: &str, text: &str) {
        Self::clear(&self.ask_card);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        head.append(&icon(icon_name, 22));
        let t = label(title, "title");
        t.set_xalign(0.0);
        t.set_wrap(true);
        head.append(&t);
        self.ask_card.append(&head);
        let s = label(text, "sub");
        s.set_wrap(true);
        s.set_xalign(0.0);
        // Wrapped at a card's width, not the screen's.
        s.set_max_width_chars(56);
        self.ask_card.append(&s);
    }

    /// A device plugged in for the first time: which wadspaces get it.
    fn ask_device(self: &Rc<Self>, d: Device) {
        self.ask_head(
            kind_icon(d.kind),
            &d.name,
            "Plugged in for the first time. Which wadspaces get it? It goes back to them whenever it's plugged in.",
        );
        let picked: Rc<RefCell<Vec<String>>> = Rc::default();
        let flow = gtk::FlowBox::new();
        flow.set_selection_mode(gtk::SelectionMode::None);
        flow.set_max_children_per_line(4);
        for (id, name) in self.spaces.borrow().iter() {
            let chip = gtk::ToggleButton::with_label(name);
            chip.add_css_class("chip");
            if self.running.borrow().contains(id) {
                chip.add_css_class("running");
            }
            let (picked, id) = (picked.clone(), id.clone());
            chip.connect_toggled(move |c| {
                let mut p = picked.borrow_mut();
                p.retain(|x| *x != id);
                if c.is_active() {
                    p.push(id.clone());
                }
            });
            flow.insert(&chip, -1);
        }
        self.ask_card.append(&flow);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_halign(gtk::Align::End);
        let keep = button("Keep it on the machine", &["small-button"]);
        let go = button("Done", &["small-button", "primary"]);
        buttons.append(&keep);
        buttons.append(&go);
        self.ask_card.append(&buttons);
        for (b, with) in [(&keep, false), (&go, true)] {
            let (p, key, picked) = (self.clone(), d.key.clone(), picked.clone());
            b.connect_clicked(move |_| {
                p.asked.borrow_mut().insert(key.clone());
                let list = if with { picked.borrow().clone() } else { vec![] };
                let (w, key, p2) = (p.wadd.clone(), key.clone(), p.clone());
                background(move || w.device_remembered(&key, &list), move |r| p2.done(r));
                p.ask_next();
            });
        }
        self.ask_win.present();
    }

    /// One of the owner's devices on the tailnet asks to manage the machine:
    /// Allow trusts it, Not now puts it off (wadd won't ask again for a while).
    fn ask_tailnet(self: &Rc<Self>, a: TailnetAsk) {
        self.ask_head(
            os_icon(&a.os),
            &format!("Allow {} to manage this machine?", a.name),
            "It's asking from the WadSpaces app, over your tailnet. Allowed, it can build, open, stop and stream \
             wadspaces here. Only allow a device of your own; Machines → Tailnet can take it off again.",
        );
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_halign(gtk::Align::End);
        let later = button("Not now", &["small-button"]);
        let allow = button("Allow", &["small-button", "primary"]);
        buttons.append(&later);
        buttons.append(&allow);
        self.ask_card.append(&buttons);
        for (b, trust) in [(&later, false), (&allow, true)] {
            let (p, id) = (self.clone(), a.id.clone());
            b.connect_clicked(move |_| {
                p.asked.borrow_mut().insert(format!("tn:{id}"));
                let (w, id, p2) = (p.wadd.clone(), id.clone(), p.clone());
                background(
                    move || if trust { w.tailnet_trust(&id) } else { w.tailnet_dismiss(&id) },
                    move |r| p2.done(r),
                );
                p.ask_next();
            });
        }
        self.ask_win.present();
    }

    /// What Bluetooth pairing asks.
    fn ask_bluetooth(self: &Rc<Self>, prompt: wad_proto::v1::BluetoothPrompt) {
        let kind =
            self.bt.borrow().devices.iter().find(|d| d.address == prompt.address).map_or(DeviceKind::Other, |d| d.kind);
        self.ask_head(
            kind_icon(kind),
            &format!("Pairing {}", prompt.name),
            &question(prompt.ask, &prompt.name, prompt.code.as_deref()),
        );
        if let Some(code) = &prompt.code {
            let c = label(code, "code");
            c.set_halign(gtk::Align::Center);
            self.ask_card.append(&c);
        }
        let pin = gtk::Entry::new();
        pin.set_max_length(16);
        pin.set_placeholder_text(Some("PIN"));
        if prompt.ask == BluetoothAsk::Pin {
            self.ask_card.append(&pin);
        }
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_halign(gtk::Align::End);
        let answer = |text: &str, accept: bool, classes: &[&str]| {
            let b = button(text, classes);
            let (p, id, pin) = (self.clone(), prompt.id, pin.clone());
            b.connect_clicked(move |_| {
                let pin = (accept && !pin.text().is_empty()).then(|| pin.text().to_string());
                let (w, p2) = (p.wadd.clone(), p.clone());
                background(move || w.bluetooth_answer(id, accept, pin), move |r| p2.done(r));
                p.ask_win.set_visible(false);
            });
            b
        };
        match prompt.ask {
            BluetoothAsk::Display => buttons.append(&answer("Cancel", false, &["small-button"])),
            BluetoothAsk::Pin => {
                buttons.append(&answer("Cancel", false, &["small-button"]));
                buttons.append(&answer("Pair", true, &["small-button", "primary"]));
            }
            BluetoothAsk::Confirm | BluetoothAsk::Authorize => {
                buttons.append(&answer("No", false, &["small-button"]));
                buttons.append(&answer("Yes", true, &["small-button", "primary"]));
            }
        }
        self.ask_card.append(&buttons);
        self.ask_win.present();
        if prompt.ask == BluetoothAsk::Pin {
            pin.grab_focus();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wad_proto::v1::DeviceBus;

    fn pad(present: bool, attached: &[&str], remembered: &[&str]) -> Device {
        Device {
            key: "usb:1:2:3".into(),
            name: "Pad".into(),
            bus: DeviceBus::Usb,
            kind: DeviceKind::Gamepad,
            attachable: true,
            present,
            attached: attached.iter().map(|s| s.to_string()).collect(),
            remembered: remembered.iter().map(|s| s.to_string()).collect(),
            new: false,
            ejected: false,
        }
    }

    #[test]
    fn says_where_a_device_is() {
        let names = |id: &str| id.to_uppercase();
        assert_eq!(whereabouts(&pad(true, &["a"], &["a", "b"]), &names), "In A");
        assert_eq!(whereabouts(&pad(true, &[], &["b"]), &names), "Goes to B when it starts");
        assert_eq!(whereabouts(&pad(true, &[], &[]), &names), "On the machine");
        assert_eq!(whereabouts(&pad(false, &[], &["a"]), &names), "Not plugged in · goes to A");
        let buds = Device { attachable: false, kind: DeviceKind::Audio, ..pad(true, &[], &[]) };
        assert_eq!(whereabouts(&buds, &names), "Works in every wadspace");
        assert_eq!(question(BluetoothAsk::Confirm, "Pad", Some("123456")), "Does Pad show 123456?");
        assert_eq!((os_icon("android"), os_icon("windows")), ("phone-symbolic", "computer-symbolic"));
    }
}
