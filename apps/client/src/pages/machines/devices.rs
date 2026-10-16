//! Devices (this machine): what's plugged in and which wadspaces each one
//! goes to (a chip per wadspace: on, it goes there whenever both are about;
//! a running one gets it now), renamed or forgotten; what works in every
//! wadspace already; and Bluetooth: on or off, find devices, pair, connect,
//! disconnect, forget. Pairing's questions come up in the HUD, on screen
//! whatever's showing. wadd's devices and bluetooth events keep it current.

use gtk::{glib, prelude::*};
use serde_json::json;
use wad_gtk::Icon;
use wad_proto::v1::{BluetoothDevice, BluetoothStatus, Device, DeviceKind, Display, Kind};
use wad_wadd_client::Method;

use super::{act, get, send};
use crate::modal::Modal;
use crate::store::{AppState, Topic, store};
use crate::ui::{self, Size, Variant};

/// A lucide icon for a kind of device.
pub fn kind_icon(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Gamepad => "gamepad-2",
        DeviceKind::Webcam => "webcam",
        DeviceKind::SecurityKey => "key",
        DeviceKind::Serial => "cable",
        DeviceKind::Keyboard => "keyboard",
        DeviceKind::Mouse => "mouse",
        DeviceKind::Audio => "headphones",
        DeviceKind::Storage => "hard-drive",
        DeviceKind::Other => "usb",
    }
}

/// The wadspaces here that can have devices (native ones): (id, name, running).
fn spaces(s: &AppState) -> Vec<(String, String, bool)> {
    s.workspaces
        .iter()
        .filter(|w| w.kind == Kind::Container && w.display == Display::Host && w.enabled)
        .map(|w| {
            let running = s.states.get(&w.id).is_some_and(|st| st.container == "running");
            (w.id.clone(), w.name.clone(), running)
        })
        .collect()
}

/// Where a device is, in words.
pub fn whereabouts(d: &Device, name: &dyn Fn(&str) -> String) -> String {
    if d.ejected {
        return "Ejected · safe to unplug".into();
    }
    if !d.attachable {
        return if d.present { "Works in every wadspace, through this machine".into() } else { "Not connected".into() };
    }
    let list = |ids: &[String]| ids.iter().map(|i| name(i)).collect::<Vec<_>>().join(", ");
    match (d.present, d.attached.is_empty(), d.remembered.is_empty()) {
        (false, _, true) => "Not plugged in".into(),
        (false, _, false) => format!("Not plugged in · goes to {}", list(&d.remembered)),
        (true, false, _) if d.kind == DeviceKind::Storage => format!("In {} · ~/Drives", list(&d.attached)),
        (true, false, _) => format!("In {}", list(&d.attached)),
        (true, true, false) => format!("Goes to {} when it starts", list(&d.remembered)),
        (true, true, true) => "On this machine only".into(),
    }
}

fn report(r: Result<(), String>) {
    if let Err(e) = r {
        crate::toast::error("Couldn't do that", &e);
    }
}

/// Reads both again (the events keep them current after that).
fn reload() {
    glib::spawn_future_local(async {
        let d = get::<Vec<Device>>("/v1/devices".into()).await;
        let b = get::<BluetoothStatus>("/v1/bluetooth".into()).await;
        store().update(|s| {
            if let Ok(d) = d {
                s.devices = d;
            }
            if let Ok(b) = b {
                s.bluetooth = Some(b);
            }
            Topic::DEVICES
        });
    });
}

pub fn view() -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 20);
    let devices = ui::panel(
        "Devices",
        Some("usb"),
        Some(
            "Drives, gamepads, webcams, security keys and other USB devices go to the wadspaces you pick, whenever both are about. A drive's files are in ~/Drives there.",
        ),
    );
    let shared = ui::panel("Works in every wadspace", Some("monitor-smartphone"), None);
    let bluetooth = ui::panel("Bluetooth", Some("bluetooth"), Some("Pairing's questions come up on the screen."));
    let power = gtk::Switch::new();
    power.set_valign(gtk::Align::Center);
    power.set_tooltip_text(Some("Bluetooth on or off"));
    let find = ui::button("Find devices", Some("bluetooth-searching"), Variant::Ghost, Size::Sm);
    bluetooth.actions.append(&find);
    bluetooth.actions.append(&power);
    for b in [&devices.body, &shared.body, &bluetooth.body] {
        b.add_css_class("device-list");
    }
    root.append(&devices.root);
    root.append(&shared.root);
    root.append(&bluetooth.root);
    root.connect_map(|_| reload());

    let syncing = std::rc::Rc::new(std::cell::Cell::new(false));
    let sy = syncing.clone();
    power.connect_active_notify(move |p| {
        if sy.get() {
            return;
        }
        let on = p.is_active();
        glib::spawn_future_local(async move {
            report(
                send::<serde_json::Value>(Method::POST, "/v1/bluetooth/power".into(), Some(json!({ "on": on })))
                    .await
                    .map(drop),
            );
        });
    });
    find.connect_clicked(|_| {
        let on = !store().read_now(|s| s.bluetooth.as_ref().is_some_and(|b| b.discovering));
        glib::spawn_future_local(async move {
            report(
                send::<serde_json::Value>(Method::POST, "/v1/bluetooth/scan".into(), Some(json!({ "on": on })))
                    .await
                    .map(drop),
            );
        });
    });

    let (dev_body, shared_root, shared_body, bt_body, bt_root) = (
        devices.body.clone(),
        shared.root.clone(),
        shared.body.clone(),
        bluetooth.body.clone(),
        bluetooth.root.clone(),
    );
    store().watch(&root, Topic::DEVICES | Topic::WORKSPACES | Topic::STATES, move |s| {
        let sp = spaces(s);
        let name = |id: &str| sp.iter().find(|(i, _, _)| i == id).map_or(id.to_string(), |(_, n, _)| n.clone());
        ui::clear(&dev_body);
        let handed: Vec<&Device> = s.devices.iter().filter(|d| d.attachable).collect();
        if handed.is_empty() {
            dev_body.append(&ui::empty_state(
                "usb",
                "Nothing to hand out",
                Some("Plug in a drive, gamepad, webcam or security key."),
            ));
        }
        for d in handed {
            dev_body.append(&device_row(d, &sp, &name));
        }
        ui::clear(&shared_body);
        let everywhere: Vec<&Device> = s.devices.iter().filter(|d| !d.attachable && d.present).collect();
        shared_root.set_visible(!everywhere.is_empty());
        for d in everywhere {
            shared_body.append(&row_head(d.kind, &d.name, &whereabouts(d, &name)));
        }
        ui::clear(&bt_body);
        let bt = s.bluetooth.clone().unwrap_or_default();
        bt_root.set_visible(bt.available);
        syncing.set(true);
        power.set_active(bt.powered);
        syncing.set(false);
        find.set_sensitive(bt.powered);
        ui::set_text(&find, if bt.discovering { "Stop looking" } else { "Find devices" });
        if !bt.powered {
            bt_body.append(&ui::notice("Bluetooth is off.", false));
        } else if bt.devices.is_empty() {
            let text =
                if bt.discovering { "Looking… put your device in pairing mode." } else { "Nothing paired yet." };
            bt_body.append(&ui::notice(text, false));
        }
        for d in &bt.devices {
            bt_body.append(&bt_row(d));
        }
    });
    root.upcast()
}

fn row_head(kind: DeviceKind, name: &str, sub: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("device-row");
    let i = Icon::new(kind_icon(kind), 18);
    i.add_css_class("muted");
    i.set_valign(gtk::Align::Center);
    row.append(&i);
    let texts = gtk::Box::new(gtk::Orientation::Vertical, 2);
    texts.set_hexpand(true);
    let n = gtk::Label::new(Some(name));
    n.add_css_class("row-name");
    n.set_xalign(0.0);
    n.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let w = gtk::Label::new(Some(sub));
    w.add_css_class("small-muted");
    w.set_xalign(0.0);
    w.set_ellipsize(gtk::pango::EllipsizeMode::End);
    texts.append(&n);
    texts.append(&w);
    row.append(&texts);
    row
}

fn device_row(d: &Device, spaces: &[(String, String, bool)], name: &dyn Fn(&str) -> String) -> gtk::Box {
    let block = gtk::Box::new(gtk::Orientation::Vertical, 8);
    block.add_css_class("device-block");
    let head = row_head(d.kind, &d.name, &whereabouts(d, name));
    if !d.present || d.ejected {
        head.add_css_class("dim");
    }
    let key = d.key.clone();
    // A drive: Eject, before it's pulled out.
    if d.kind == DeviceKind::Storage && d.present && !d.ejected {
        let eject = ui::button("Eject", Some("unplug"), Variant::Ghost, Size::Sm);
        eject.set_valign(gtk::Align::Center);
        eject.set_tooltip_text(Some("Take it out of its wadspaces and turn it off, to unplug it safely"));
        let key = key.clone();
        eject.connect_clicked(move |b| {
            b.set_sensitive(false);
            let (path, b) = (format!("/v1/devices/{key}/eject"), b.clone());
            glib::spawn_future_local(async move {
                match send::<serde_json::Value>(Method::POST, path, Some(json!({}))).await {
                    Ok(_) => crate::toast::show("Safe to unplug", None, crate::toast::Tone::Info),
                    Err(e) => {
                        crate::toast::error("Couldn't eject it", &e);
                        b.set_sensitive(true);
                    }
                }
            });
        });
        head.append(&eject);
    }
    let mut items: Vec<ui::MenuItem> = vec![(
        "Rename",
        "pencil",
        false,
        Box::new({
            let (key, now) = (key.clone(), d.name.clone());
            move || rename(&key, &now)
        }),
    )];
    if !d.present {
        items.push((
            "Forget",
            "trash",
            true,
            Box::new({
                let key = key.clone();
                move || {
                    let key = key.clone();
                    glib::spawn_future_local(async move {
                        report(act(Method::DELETE, format!("/v1/devices/{key}"), None).await);
                        reload();
                    });
                }
            }),
        ));
    }
    head.append(&ui::menu("ellipsis", "More", items));
    block.append(&head);
    let chips = gtk::FlowBox::new();
    chips.add_css_class("device-chips");
    chips.set_selection_mode(gtk::SelectionMode::None);
    chips.set_max_children_per_line(8);
    chips.set_column_spacing(6);
    chips.set_row_spacing(6);
    if spaces.is_empty() {
        chips.insert(&ui::notice("No wadspaces on this machine yet.", false), -1);
    }
    for (id, n, running) in spaces {
        let chip = gtk::ToggleButton::with_label(n);
        chip.add_css_class("device-chip");
        if *running {
            chip.add_css_class("running");
            chip.set_tooltip_text(Some("Running: it gets it now"));
        }
        chip.set_active(d.remembered.contains(id));
        let (key, id) = (key.clone(), id.clone());
        chip.connect_toggled(move |c| {
            let how = if c.is_active() { "attach" } else { "detach" };
            let (path, id) = (format!("/v1/devices/{key}/{how}"), id.clone());
            glib::spawn_future_local(async move {
                report(send::<serde_json::Value>(Method::POST, path, Some(json!({ "wadspace": id }))).await.map(drop));
            });
        });
        chips.insert(&chip, -1);
    }
    if !d.ejected {
        block.append(&chips);
    }
    block
}

/// Renames a device: a small dialog (empty: its own name again).
fn rename(key: &str, now: &str) {
    let m = Modal::open("Rename device", Some("Leave it empty for the device's own name."), 420);
    let col = gtk::Box::new(gtk::Orientation::Vertical, 16);
    let entry = ui::entry("Its name");
    entry.set_text(now);
    col.append(&entry);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_halign(gtk::Align::End);
    let cancel = ui::button("Cancel", None, Variant::Ghost, Size::Md);
    let save = ui::button("Save", None, Variant::Primary, Size::Md);
    let mm = m.clone();
    cancel.connect_clicked(move |_| mm.close());
    let go = {
        let (mm, key, entry) = (m.clone(), key.to_string(), entry.clone());
        move || {
            let (mm, path, name) = (mm.clone(), format!("/v1/devices/{key}"), entry.text().to_string());
            glib::spawn_future_local(async move {
                let r = send::<serde_json::Value>(Method::PUT, path, Some(json!({ "name": name }))).await;
                report(r.map(drop));
                mm.close();
            });
        }
    };
    let g = go.clone();
    save.connect_clicked(move |_| g());
    entry.connect_activate(move |_| go());
    row.append(&cancel);
    row.append(&save);
    col.append(&row);
    m.body().append(&col);
    entry.grab_focus();
}

fn bt_row(d: &BluetoothDevice) -> gtk::Box {
    let tag = if d.busy {
        "Working…"
    } else if d.connected {
        "Connected"
    } else if d.paired {
        "Paired"
    } else {
        "Nearby"
    };
    let row = row_head(d.kind, &d.name, tag);
    let action = |text: &str, icon: &'static str, act_: &'static str| {
        let b = ui::button(text, Some(icon), Variant::Ghost, Size::Sm);
        b.set_valign(gtk::Align::Center);
        b.set_sensitive(!d.busy);
        let addr = d.address.clone();
        b.connect_clicked(move |_| {
            let path = format!("/v1/bluetooth/{addr}/{act_}");
            glib::spawn_future_local(async move { report(act(Method::POST, path, Some(json!({}))).await) });
        });
        b
    };
    if !d.paired {
        row.append(&action("Pair", "link-2", "pair"));
    } else {
        row.append(&if d.connected {
            action("Disconnect", "unplug", "disconnect")
        } else {
            action("Connect", "plug", "connect")
        });
        row.append(&action("Forget", "trash", "forget"));
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;
    use wad_proto::v1::DeviceBus;

    #[test]
    fn says_where_a_device_is() {
        let d = Device {
            key: "usb:1:2:3".into(),
            name: "Pad".into(),
            bus: DeviceBus::Usb,
            kind: DeviceKind::Gamepad,
            attachable: true,
            present: true,
            attached: vec!["a".into()],
            remembered: vec!["a".into(), "b".into()],
            new: false,
            ejected: false,
        };
        let name = |id: &str| id.to_uppercase();
        assert_eq!(whereabouts(&d, &name), "In A");
        let off = Device { present: false, ..d.clone() };
        assert_eq!(whereabouts(&off, &name), "Not plugged in · goes to A, B");
        assert_eq!(kind_icon(DeviceKind::SecurityKey), "key");
    }
}
