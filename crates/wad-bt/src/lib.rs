//! Bluetooth through BlueZ over D-Bus: the adapter on and off, looking for
//! devices, pairing (then trusting and connecting), disconnecting and
//! forgetting.
//!
//! Pairing goes through wadd's own agent (org.bluez.Agent1, registered as
//! KeyboardDisplay and the default): what it's asked (does the device show
//! this code? type this code on the keyboard; the PIN an old device wants)
//! goes out as an [`AgentEvent`] for wadd to show and answer, and comes back
//! through the reply it carries. Once paired, a device is trusted, so it
//! connects again by itself.
//!
//! wadd runs as root on a machine, so BlueZ takes everything from it.

use std::collections::HashMap;
use std::sync::Mutex;

use wad_proto::v1::{BluetoothAsk, BluetoothDevice, BluetoothPrompt, BluetoothStatus, DeviceKind};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

mod proxies;
use proxies::*;

const AGENT_PATH: &str = "/io/wadspaces/agent";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Bluetooth: {0}")]
    Bus(#[from] zbus::Error),
    #[error("{0}")]
    Refused(String),
    #[error("this machine has no Bluetooth")]
    NoAdapter,
}

/// What pairing asks; `reply` (None: nothing to answer) takes the answer.
#[derive(Debug)]
pub struct Ask {
    pub prompt: BluetoothPrompt,
    pub reply: Option<tokio::sync::oneshot::Sender<Answer>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub accept: bool,
    pub pin: Option<String>,
}

/// From the agent to wadd.
#[derive(Debug)]
pub enum AgentEvent {
    Ask(Ask),
    /// BlueZ called it off (pairing ended or timed out).
    Cancel,
}

/// What a device is, from BlueZ's icon name (its class of device).
pub fn kind_of(icon: &str) -> DeviceKind {
    match icon {
        "input-gaming" => DeviceKind::Gamepad,
        "input-keyboard" => DeviceKind::Keyboard,
        "input-mouse" | "input-tablet" => DeviceKind::Mouse,
        i if i.starts_with("audio-") || i == "multimedia-player" => DeviceKind::Audio,
        "camera-video" | "camera-web" => DeviceKind::Webcam,
        _ => DeviceKind::Other,
    }
}

pub fn is_address(s: &str) -> bool {
    s.len() == 17
        && s.split(':').count() == 6
        && s.split(':').all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Paired ones first (connected first), then what's in range, strongest
/// first; nameless devices in range (beacons, phones' random addresses)
/// left out.
pub fn order(mut devices: Vec<BluetoothDevice>) -> Vec<BluetoothDevice> {
    devices.retain(|d| d.paired || d.name != d.address);
    devices.sort_by(|a, b| {
        (!a.paired, !a.connected, -(a.rssi.unwrap_or(-127) as i32), a.name.to_lowercase()).cmp(&(
            !b.paired,
            !b.connected,
            -(b.rssi.unwrap_or(-127) as i32),
            b.name.to_lowercase(),
        ))
    });
    devices
}

fn prop<T: TryFrom<OwnedValue>>(props: &HashMap<String, OwnedValue>, k: &str) -> Option<T> {
    props.get(k).and_then(|v| v.try_clone().ok()).and_then(|v| T::try_from(v).ok())
}

pub struct Bluez {
    conn: zbus::Connection,
    /// Addresses being paired or connected.
    busy: Mutex<std::collections::HashSet<String>>,
    agent_on: std::sync::atomic::AtomicBool,
    agent: Agent,
}

impl Bluez {
    /// On the system bus, with wadd's agent (`events` gets what it asks).
    pub async fn system(events: tokio::sync::mpsc::UnboundedSender<AgentEvent>) -> Result<Self, Error> {
        let conn = zbus::Connection::system().await?;
        let agent = Agent { events };
        conn.object_server().at(AGENT_PATH, agent.clone()).await?;
        let me = Self { conn, busy: Mutex::default(), agent_on: std::sync::atomic::AtomicBool::new(false), agent };
        me.ensure_agent().await;
        Ok(me)
    }

    /// Registers the agent with BlueZ (again after bluetoothd restarted).
    async fn ensure_agent(&self) {
        use std::sync::atomic::Ordering;
        if self.agent_on.load(Ordering::Relaxed) {
            return;
        }
        let r = async {
            let m = AgentManagerProxy::new(&self.conn).await?;
            let path = OwnedObjectPath::try_from(AGENT_PATH).expect("path");
            m.register_agent(&path, "KeyboardDisplay").await?;
            m.request_default_agent(&path).await
        }
        .await;
        match r {
            Ok(()) => self.agent_on.store(true, Ordering::Relaxed),
            Err(e) if e.to_string().contains("AlreadyExists") => self.agent_on.store(true, Ordering::Relaxed),
            Err(e) => tracing::debug!("bluetooth agent: {e}"),
        }
    }

    async fn objects(&self) -> Result<Objects, Error> {
        match ObjectManagerProxy::new(&self.conn).await?.get_managed_objects().await {
            Ok(o) => Ok(o),
            Err(e) => {
                // bluetoothd went away: its agent with it.
                self.agent_on.store(false, std::sync::atomic::Ordering::Relaxed);
                Err(e.into())
            }
        }
    }

    async fn adapter(&self, objects: &Objects) -> Result<AdapterProxy<'static>, Error> {
        let mut paths: Vec<&OwnedObjectPath> =
            objects.iter().filter(|(_, i)| i.contains_key(ADAPTER)).map(|(p, _)| p).collect();
        paths.sort_by_key(|p| p.as_str());
        let path = paths.first().ok_or(Error::NoAdapter)?;
        Ok(AdapterProxy::builder(&self.conn)
            .path((*path).clone())?
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await?)
    }

    async fn device(&self, address: &str) -> Result<(OwnedObjectPath, DeviceProxy<'static>), Error> {
        if !is_address(address) {
            return Err(Error::Refused(format!("{address:?} isn't a Bluetooth address")));
        }
        let objects = self.objects().await?;
        let path = objects
            .iter()
            .find(|(_, i)| {
                i.get(DEVICE)
                    .and_then(|p| prop::<String>(p, "Address"))
                    .is_some_and(|a| a.eq_ignore_ascii_case(address))
            })
            .map(|(p, _)| p.clone())
            .ok_or_else(|| Error::Refused("it isn't in range any more".into()))?;
        let proxy = DeviceProxy::builder(&self.conn).path(path.clone())?.build().await?;
        Ok((path, proxy))
    }

    /// The adapter and its devices.
    pub async fn status(&self) -> Result<BluetoothStatus, Error> {
        self.ensure_agent().await;
        let objects = self.objects().await?;
        let Some((_, adapter)) =
            objects.iter().filter(|(_, i)| i.contains_key(ADAPTER)).min_by_key(|(p, _)| p.as_str())
        else {
            return Ok(BluetoothStatus::default());
        };
        let a = &adapter[ADAPTER];
        let busy = self.busy.lock().unwrap().clone();
        let devices = objects
            .values()
            .filter_map(|i| i.get(DEVICE))
            .filter_map(|d| {
                let address: String = prop(d, "Address")?;
                let name: String = prop(d, "Alias").or_else(|| prop(d, "Name")).unwrap_or_else(|| address.clone());
                let icon: String = prop(d, "Icon").unwrap_or_default();
                Some(BluetoothDevice {
                    busy: busy.contains(&address),
                    kind: kind_of(&icon),
                    paired: prop(d, "Paired").unwrap_or(false),
                    connected: prop(d, "Connected").unwrap_or(false),
                    rssi: prop(d, "RSSI"),
                    name,
                    address,
                })
            })
            .collect();
        Ok(BluetoothStatus {
            available: true,
            powered: prop(a, "Powered").unwrap_or(false),
            discovering: prop(a, "Discovering").unwrap_or(false),
            devices: order(devices),
            prompt: None,
        })
    }

    /// Turns the adapter on or off (unblocking it first, when it's
    /// soft-blocked).
    pub async fn power(&self, on: bool) -> Result<(), Error> {
        let adapter = self.adapter(&self.objects().await?).await?;
        if on {
            unblock();
        }
        match adapter.set_powered(on).await {
            Err(e) if on && e.to_string().contains("Blocked") => {
                unblock();
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                adapter.set_powered(true).await?;
            }
            r => r?,
        }
        Ok(())
    }

    /// Looks for devices to pair (on), or stops.
    pub async fn scan(&self, on: bool) -> Result<(), Error> {
        let adapter = self.adapter(&self.objects().await?).await?;
        if on && !adapter.powered().await? {
            self.power(true).await?;
        }
        let r = if on { adapter.start_discovery().await } else { adapter.stop_discovery().await };
        match r {
            // Already as asked.
            Err(e) if e.to_string().contains("InProgress") || e.to_string().contains("NotReady") && !on => Ok(()),
            r => r.map_err(Error::from),
        }
    }

    fn busy(&self, address: &str, yes: bool) {
        let mut b = self.busy.lock().unwrap();
        if yes {
            b.insert(address.to_uppercase());
        } else {
            b.remove(&address.to_uppercase());
        }
    }

    /// Pairs, trusts and connects a device (the agent asks what it must).
    pub async fn pair(&self, address: &str) -> Result<(), Error> {
        let (_, d) = self.device(address).await?;
        self.busy(address, true);
        let r = async {
            match d.pair().await {
                Err(e) if e.to_string().contains("AlreadyExists") => {}
                r => r?,
            }
            d.set_trusted(true).await?;
            d.connect().await?;
            Ok::<_, Error>(())
        }
        .await;
        self.busy(address, false);
        self.agent.done();
        r.map_err(friendly)
    }

    pub async fn connect(&self, address: &str) -> Result<(), Error> {
        let (_, d) = self.device(address).await?;
        self.busy(address, true);
        let r = d.connect().await;
        self.busy(address, false);
        r.map_err(|e| friendly(e.into()))
    }

    pub async fn disconnect(&self, address: &str) -> Result<(), Error> {
        let (_, d) = self.device(address).await?;
        d.disconnect().await?;
        Ok(())
    }

    /// Unpairs and forgets a device.
    pub async fn forget(&self, address: &str) -> Result<(), Error> {
        let objects = self.objects().await?;
        let (path, _) = self.device(address).await?;
        self.adapter(&objects).await?.remove_device(&path).await?;
        Ok(())
    }
}

/// BlueZ's errors, in words.
fn friendly(e: Error) -> Error {
    let text = e.to_string();
    let says = |s: &str| text.contains(s);
    if says("AuthenticationFailed") || says("AuthenticationRejected") || says("Rejected") {
        Error::Refused("pairing was turned down: the codes didn't match, or it was cancelled".into())
    } else if says("AuthenticationTimeout") || says("Timeout") {
        Error::Refused("it didn't answer in time: put it in pairing mode and try again".into())
    } else if says("ConnectionAttemptFailed") || says("page-timeout") || says("Host is down") {
        Error::Refused("it didn't answer: is it on and close by?".into())
    } else {
        e
    }
}

/// Soft-unblocks the machine's Bluetooth radios (rfkill).
fn unblock() {
    for e in std::fs::read_dir("/sys/class/rfkill").into_iter().flatten().flatten() {
        let p = e.path();
        if std::fs::read_to_string(p.join("type")).is_ok_and(|t| t.trim() == "bluetooth")
            && std::fs::read_to_string(p.join("soft")).is_ok_and(|s| s.trim() == "1")
        {
            let _ = std::fs::write(p.join("soft"), "0");
        }
    }
}

// ------------------------------------------------------------------ agent

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Rejected(String),
    Canceled(String),
}

/// wadd's pairing agent: what BlueZ asks goes out as an event, and the
/// answer comes back (or nobody answers in a minute: rejected).
#[derive(Clone)]
struct Agent {
    events: tokio::sync::mpsc::UnboundedSender<AgentEvent>,
}

/// One prompt at a time, numbered.
static PROMPTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Agent {
    async fn who(conn: &zbus::Connection, device: &OwnedObjectPath) -> (String, String) {
        let props = zbus::fdo::PropertiesProxy::builder(conn)
            .destination("org.bluez")
            .and_then(|b| b.path(device.clone()))
            .map(|b| b.build());
        let all = match props {
            Ok(p) => match p.await {
                Ok(p) => p.get_all(DEVICE.try_into().expect("name")).await.unwrap_or_default(),
                Err(_) => HashMap::new(),
            },
            Err(_) => HashMap::new(),
        };
        let address: String = prop(&all, "Address").unwrap_or_default();
        let name: String = prop(&all, "Alias").unwrap_or_else(|| address.clone());
        (address, name)
    }

    async fn ask(
        &self,
        conn: &zbus::Connection,
        device: &OwnedObjectPath,
        ask: BluetoothAsk,
        code: Option<String>,
        answer: bool,
    ) -> Result<Answer, AgentError> {
        let (address, name) = Self::who(conn, device).await;
        let id = PROMPTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let prompt = BluetoothPrompt { id, address, name, ask, code };
        if !answer {
            let _ = self.events.send(AgentEvent::Ask(Ask { prompt, reply: None }));
            return Ok(Answer { accept: true, pin: None });
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.events
            .send(AgentEvent::Ask(Ask { prompt, reply: Some(tx) }))
            .map_err(|_| AgentError::Rejected("nobody to ask".into()))?;
        match tokio::time::timeout(std::time::Duration::from_secs(60), rx).await {
            Ok(Ok(a)) if a.accept => Ok(a),
            Ok(Ok(_)) => Err(AgentError::Rejected("turned down".into())),
            _ => Err(AgentError::Canceled("not answered".into())),
        }
    }

    fn done(&self) {
        let _ = self.events.send(AgentEvent::Cancel);
    }
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    async fn release(&self) {}

    async fn request_pin_code(
        &self,
        #[zbus(connection)] conn: &zbus::Connection,
        device: OwnedObjectPath,
    ) -> Result<String, AgentError> {
        let a = self.ask(conn, &device, BluetoothAsk::Pin, None, true).await?;
        let pin = a.pin.unwrap_or_default();
        if pin.is_empty() || pin.len() > 16 || !pin.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(AgentError::Rejected("no PIN".into()));
        }
        Ok(pin)
    }

    async fn display_pin_code(
        &self,
        #[zbus(connection)] conn: &zbus::Connection,
        device: OwnedObjectPath,
        pincode: String,
    ) -> Result<(), AgentError> {
        self.ask(conn, &device, BluetoothAsk::Display, Some(pincode), false).await.map(drop)
    }

    async fn request_passkey(
        &self,
        #[zbus(connection)] conn: &zbus::Connection,
        device: OwnedObjectPath,
    ) -> Result<u32, AgentError> {
        let a = self.ask(conn, &device, BluetoothAsk::Pin, None, true).await?;
        a.pin
            .as_deref()
            .and_then(|p| p.parse::<u32>().ok())
            .filter(|p| *p <= 999_999)
            .ok_or(AgentError::Rejected("no passkey".into()))
    }

    async fn display_passkey(
        &self,
        #[zbus(connection)] conn: &zbus::Connection,
        device: OwnedObjectPath,
        passkey: u32,
        _entered: u16,
    ) -> Result<(), AgentError> {
        self.ask(conn, &device, BluetoothAsk::Display, Some(format!("{passkey:06}")), false).await.map(drop)
    }

    async fn request_confirmation(
        &self,
        #[zbus(connection)] conn: &zbus::Connection,
        device: OwnedObjectPath,
        passkey: u32,
    ) -> Result<(), AgentError> {
        self.ask(conn, &device, BluetoothAsk::Confirm, Some(format!("{passkey:06}")), true).await.map(drop)
    }

    async fn request_authorization(
        &self,
        #[zbus(connection)] conn: &zbus::Connection,
        device: OwnedObjectPath,
    ) -> Result<(), AgentError> {
        self.ask(conn, &device, BluetoothAsk::Authorize, None, true).await.map(drop)
    }

    /// A paired device using a service: fine (it's trusted once paired).
    async fn authorize_service(
        &self,
        #[zbus(connection)] conn: &zbus::Connection,
        device: OwnedObjectPath,
        _uuid: String,
    ) -> Result<(), AgentError> {
        let paired = DeviceInfoProxy::builder(conn)
            .path(device)
            .map_err(AgentError::ZBus)?
            .build()
            .await
            .map_err(AgentError::ZBus)?
            .paired()
            .await
            .unwrap_or(false);
        if paired { Ok(()) } else { Err(AgentError::Rejected("not paired".into())) }
    }

    async fn cancel(&self) {
        self.done();
    }
}

#[zbus::proxy(interface = "org.bluez.Device1", default_service = "org.bluez")]
trait DeviceInfo {
    #[zbus(property)]
    fn paired(&self) -> zbus::Result<bool>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str, paired: bool, connected: bool, rssi: Option<i16>) -> BluetoothDevice {
        BluetoothDevice {
            address: "AA:BB:CC:DD:EE:FF".into(),
            name: name.into(),
            kind: DeviceKind::Other,
            paired,
            connected,
            rssi,
            busy: false,
        }
    }

    #[test]
    fn paired_first_then_the_strongest_in_range() {
        let list = order(vec![
            dev("Far", false, false, Some(-90)),
            dev("Pad", true, false, None),
            dev("AA:BB:CC:DD:EE:FF", false, false, Some(-40)),
            dev("Near", false, false, Some(-50)),
            dev("Buds", true, true, None),
        ]);
        let names: Vec<&str> = list.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Buds", "Pad", "Near", "Far"]);
    }

    #[test]
    fn kinds_come_from_the_class_icon() {
        assert_eq!(kind_of("input-gaming"), DeviceKind::Gamepad);
        assert_eq!(kind_of("audio-headphones"), DeviceKind::Audio);
        assert_eq!(kind_of("audio-headset"), DeviceKind::Audio);
        assert_eq!(kind_of("input-keyboard"), DeviceKind::Keyboard);
        assert_eq!(kind_of("phone"), DeviceKind::Other);
        assert!(is_address("A0:AB:51:12:34:56") && !is_address("A0:AB:51:12:34") && !is_address("../x"));
    }
}

/// `cargo test -p wad-bt this_machines_bluetooth -- --ignored --nocapture`.
#[cfg(test)]
#[tokio::test]
#[ignore = "reads this machine's BlueZ"]
async fn this_machines_bluetooth() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let b = Bluez::system(tx).await.unwrap();
    let s = b.status().await.unwrap();
    println!("available {} powered {} discovering {}", s.available, s.powered, s.discovering);
    for d in s.devices {
        println!(
            "  {} {} {:?} paired={} connected={} rssi={:?}",
            d.address, d.name, d.kind, d.paired, d.connected, d.rssi
        );
    }
}
