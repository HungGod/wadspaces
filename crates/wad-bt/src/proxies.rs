//! The parts of BlueZ's D-Bus API this uses.

use std::collections::HashMap;

use zbus::zvariant::{OwnedObjectPath, OwnedValue};

/// Every object BlueZ has: path → interface → property → value.
pub type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

#[zbus::proxy(interface = "org.freedesktop.DBus.ObjectManager", default_service = "org.bluez", default_path = "/")]
pub trait ObjectManager {
    fn get_managed_objects(&self) -> zbus::Result<Objects>;
}

#[zbus::proxy(interface = "org.bluez.Adapter1", default_service = "org.bluez")]
pub trait Adapter {
    fn start_discovery(&self) -> zbus::Result<()>;
    fn stop_discovery(&self) -> zbus::Result<()>;
    fn remove_device(&self, device: &OwnedObjectPath) -> zbus::Result<()>;
    #[zbus(property)]
    fn powered(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn set_powered(&self, on: bool) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "org.bluez.Device1", default_service = "org.bluez")]
pub trait Device {
    fn pair(&self) -> zbus::Result<()>;
    fn cancel_pairing(&self) -> zbus::Result<()>;
    fn connect(&self) -> zbus::Result<()>;
    fn disconnect(&self) -> zbus::Result<()>;
    #[zbus(property)]
    fn set_trusted(&self, yes: bool) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "org.bluez.AgentManager1", default_service = "org.bluez", default_path = "/org/bluez")]
pub trait AgentManager {
    fn register_agent(&self, agent: &OwnedObjectPath, capability: &str) -> zbus::Result<()>;
    fn request_default_agent(&self, agent: &OwnedObjectPath) -> zbus::Result<()>;
}
