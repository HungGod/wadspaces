//! The machine's Bluetooth for the HUD and the app (wad-bt: BlueZ over
//! D-Bus): looked at every few seconds (every second while looking for
//! devices or pairing), out as a bluetooth event when it changes, with the
//! prompt pairing is waiting on. Paired devices also go to the device
//! server: a Bluetooth gamepad is handed to wadspaces like a USB one, and
//! headphones show as working in every wadspace (the machine's sound).

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use wad_bt::{AgentEvent, Answer, Bluez};
use wad_proto::v1::{BluetoothAnswer, BluetoothPrompt, BluetoothStatus, Event};
use wad_proto::{ApiError, ErrorCode};

use crate::devices::Devices;
use crate::events::Bus;

const EVERY: Duration = Duration::from_secs(4);
const BUSY_EVERY: Duration = Duration::from_secs(1);
/// Looking for devices stops by itself after this.
const SCAN_FOR: Duration = Duration::from_secs(60);

fn api_error(e: wad_bt::Error) -> ApiError {
    let code = match &e {
        wad_bt::Error::Refused(_) => ErrorCode::BadRequest,
        wad_bt::Error::NoAdapter => ErrorCode::Conflict,
        wad_bt::Error::Bus(_) => ErrorCode::Offline,
    };
    ApiError::new(code, e.to_string())
}

type Pending = (BluetoothPrompt, Option<tokio::sync::oneshot::Sender<Answer>>);

pub struct Bluetooth {
    /// None: no BlueZ to talk to (a laptop's tests, a bus that's down).
    bz: Option<Arc<Bluez>>,
    agent: Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<AgentEvent>>>,
    bus: Bus,
    last: Mutex<BluetoothStatus>,
    prompt: Mutex<Option<Pending>>,
    devices: Mutex<Weak<Devices>>,
    poke: tokio::sync::Notify,
    scan_until: Mutex<Option<tokio::time::Instant>>,
}

impl Bluetooth {
    pub fn new(
        bz: Option<Bluez>,
        agent: Option<tokio::sync::mpsc::UnboundedReceiver<AgentEvent>>,
        bus: Bus,
    ) -> Arc<Self> {
        Arc::new(Self {
            bz: bz.map(Arc::new),
            agent: Mutex::new(agent),
            bus,
            last: Mutex::default(),
            prompt: Mutex::default(),
            devices: Mutex::default(),
            poke: tokio::sync::Notify::new(),
            scan_until: Mutex::default(),
        })
    }

    /// Where paired devices go (the device server).
    pub fn set_devices(&self, d: &Arc<Devices>) {
        *self.devices.lock().unwrap() = Arc::downgrade(d);
    }

    fn bz(&self) -> Result<&Arc<Bluez>, ApiError> {
        self.bz.as_ref().ok_or_else(|| ApiError::new(ErrorCode::Offline, "Bluetooth (BlueZ) isn't reachable"))
    }

    /// The status now (with the prompt); published when it changed.
    pub async fn refresh(&self) -> BluetoothStatus {
        let mut s = match &self.bz {
            Some(b) => b.status().await.unwrap_or_default(),
            None => BluetoothStatus::default(),
        };
        s.prompt = self.prompt.lock().unwrap().as_ref().map(|(p, _)| p.clone());
        if let Some(d) = self.devices.lock().unwrap().upgrade() {
            d.set_bluetooth(&s.devices);
        }
        let changed = {
            let mut last = self.last.lock().unwrap();
            let changed = *last != s;
            *last = s.clone();
            changed
        };
        if changed {
            self.bus.publish(Event::Bluetooth(s.clone()));
        }
        s
    }

    pub fn status(&self) -> BluetoothStatus {
        self.last.lock().unwrap().clone()
    }

    /// Follows BlueZ and the agent until it's stopped.
    pub async fn run(self: Arc<Self>) {
        let mut agent = self.agent.lock().unwrap().take();
        loop {
            self.refresh().await;
            if self.scan_until.lock().unwrap().is_some_and(|t| t <= tokio::time::Instant::now()) {
                *self.scan_until.lock().unwrap() = None;
                if let Some(b) = &self.bz {
                    let _ = b.scan(false).await;
                }
                continue;
            }
            let s = self.status();
            let busy = s.discovering || s.prompt.is_some() || s.devices.iter().any(|d| d.busy);
            let wait = tokio::time::sleep(if busy { BUSY_EVERY } else { EVERY });
            tokio::select! {
                _ = wait => {}
                _ = self.poke.notified() => {}
                Some(ev) = async { match agent.as_mut() { Some(a) => a.recv().await, None => std::future::pending().await } } => {
                    self.take(ev);
                }
            }
        }
    }

    /// What the agent asks: the prompt to show (the one before it, if any,
    /// goes unanswered: turned down).
    fn take(&self, ev: AgentEvent) {
        let mut p = self.prompt.lock().unwrap();
        match ev {
            AgentEvent::Ask(a) => *p = Some((a.prompt, a.reply)),
            AgentEvent::Cancel => *p = None,
        }
    }

    /// Answers the prompt shown (by its id).
    pub fn answer(&self, a: BluetoothAnswer) -> Result<(), ApiError> {
        let pending = {
            let mut p = self.prompt.lock().unwrap();
            match p.as_ref() {
                Some((prompt, _)) if prompt.id == a.id => p.take(),
                _ => return Err(ApiError::new(ErrorCode::Conflict, "that question was answered or called off")),
            }
        };
        if let Some((_, Some(reply))) = pending {
            let _ = reply.send(Answer { accept: a.accept, pin: a.pin });
        }
        self.poke.notify_one();
        Ok(())
    }

    pub async fn power(&self, on: bool) -> Result<BluetoothStatus, ApiError> {
        self.bz()?.power(on).await.map_err(api_error)?;
        Ok(self.refresh().await)
    }

    /// Looks for devices to pair for a minute (on), or stops.
    pub async fn scan(&self, on: bool) -> Result<BluetoothStatus, ApiError> {
        self.bz()?.scan(on).await.map_err(api_error)?;
        *self.scan_until.lock().unwrap() = on.then(|| tokio::time::Instant::now() + SCAN_FOR);
        self.poke.notify_one();
        Ok(self.refresh().await)
    }

    /// Pairs, trusts and connects a device in the background (it may ask
    /// things first): how it went comes as events, and a notice if it failed.
    pub fn pair(self: &Arc<Self>, address: &str) -> Result<(), ApiError> {
        self.background(address, "pair", |b, a| Box::pin(async move { b.pair(&a).await }))
    }

    pub fn connect(self: &Arc<Self>, address: &str) -> Result<(), ApiError> {
        self.background(address, "connect", |b, a| Box::pin(async move { b.connect(&a).await }))
    }

    fn background(
        self: &Arc<Self>,
        address: &str,
        what: &'static str,
        f: impl FnOnce(Arc<Bluez>, String) -> futures_util::future::BoxFuture<'static, Result<(), wad_bt::Error>>
        + Send
        + 'static,
    ) -> Result<(), ApiError> {
        if !wad_bt::is_address(address) {
            return Err(ApiError::new(ErrorCode::BadRequest, format!("{address:?} isn't a Bluetooth address")));
        }
        let bz = self.bz()?.clone();
        let (me, address) = (self.clone(), address.to_uppercase());
        self.poke.notify_one();
        tokio::spawn(async move {
            let name =
                me.status().devices.iter().find(|d| d.address == address).map_or(address.clone(), |d| d.name.clone());
            if let Err(e) = f(bz, address).await {
                tracing::info!("bluetooth: {what} {name}: {e}");
                me.bus.publish(Event::Notice { text: format!("Couldn't {what} {name}: {e}") });
            }
            *me.prompt.lock().unwrap() = None;
            me.refresh().await;
        });
        Ok(())
    }

    pub async fn disconnect(&self, address: &str) -> Result<BluetoothStatus, ApiError> {
        self.bz()?.disconnect(address).await.map_err(api_error)?;
        Ok(self.refresh().await)
    }

    pub async fn forget(&self, address: &str) -> Result<BluetoothStatus, ApiError> {
        self.bz()?.forget(address).await.map_err(api_error)?;
        Ok(self.refresh().await)
    }
}
