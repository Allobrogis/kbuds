// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-device supervisor: connects the Maestro channel while the buds are connected over
//! Bluetooth, runs sessions, reconnects after the buds close the channel, watches the audio stack,
//! and publishes the state on D-Bus.

use std::collections::HashMap;
use std::fs::File;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kbuds_protocol::MAESTRO_UUID;
use kbuds_protocol::client::{Connection, timing};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use zbus::zvariant::{OwnedObjectPath, Value};

use crate::api::{self, ApiError, Command, Reply};
use crate::audio;
use crate::battery::Batteries;
use crate::bluez::{Device1Proxy, DeviceInfo, TransportInfo};
use crate::config::Config;
use crate::fdstream::FdStream;
use crate::framelog;
use crate::profile::{ProfileEvent, Router};
use crate::session::{self, SessionCmd, SessionEnd, StateTx};
use crate::state::{DeviceState, SessionState};

/// Shared daemon context.
pub struct Ctx {
    pub system: zbus::Connection,
    pub session_bus: zbus::Connection,
    pub router: Router,
    pub batteries: Batteries,
    pub frame_log: Option<Arc<Mutex<File>>>,
    pub config: Arc<Mutex<Config>>,
}

/// Updates from the BlueZ monitor.
#[derive(Debug)]
pub enum Input {
    Info(DeviceInfo),
    Transport(TransportInfo),
    TransportRemoved(OwnedObjectPath),
}

pub struct DeviceHandle {
    pub input: mpsc::UnboundedSender<Input>,
    pub dbus_path: OwnedObjectPath,
    pub bluez_path: OwnedObjectPath,
    supervisor: JoinHandle<()>,
    publisher: JoinHandle<()>,
}

impl DeviceHandle {
    /// Stops the device: drops the Maestro session and removes its D-Bus objects.
    pub async fn stop(self, ctx: &Ctx) {
        self.supervisor.abort();
        self.publisher.abort();
        let _ = self.supervisor.await;
        ctx.router.unregister(&self.bluez_path);
        let _ = ctx.session_bus.object_server().remove::<api::Device, _>(&self.dbus_path).await;
        ctx.batteries.update(&self.bluez_path, None).await;
    }
}

/// Creates the D-Bus object and starts the tasks for a newly seen device.
pub async fn spawn(ctx: Arc<Ctx>, info: DeviceInfo) -> zbus::Result<DeviceHandle> {
    let mut st = DeviceState::new(&info.address, &info.alias);
    st.bluetooth_connected = info.connected;
    st.auto_fix_audio = ctx.config.lock().unwrap().device(&info.address).auto_fix_audio;
    let (state_tx, state_rx) = watch::channel(st);
    let state_tx = Arc::new(state_tx);
    let (cmd_tx, cmd_rx) = mpsc::channel(16);
    let dbus_path = api::device_path(&info.address);
    ctx.session_bus
        .object_server()
        .at(&dbus_path, api::Device { state: state_rx.clone(), cmd: cmd_tx })
        .await?;
    let publisher = tokio::spawn(publish(ctx.clone(), dbus_path.clone(), info.path.clone(), state_rx));
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let profile_rx = ctx.router.register(info.path.clone());
    let bluez_path = info.path.clone();
    let supervisor = tokio::spawn(Supervisor::new(ctx, info, state_tx).run(input_rx, cmd_rx, profile_rx));
    Ok(DeviceHandle { input: input_tx, dbus_path, bluez_path, supervisor, publisher })
}

/// Emits PropertiesChanged for every state change, and feeds the battery provider.
async fn publish(
    ctx: Arc<Ctx>,
    path: OwnedObjectPath,
    bluez_path: OwnedObjectPath,
    mut rx: watch::Receiver<DeviceState>,
) {
    let mut last = rx.borrow_and_update().clone();
    let iface = zbus::names::InterfaceName::from_static_str_unchecked(api::DEVICE_INTERFACE);
    let Ok(emitter) = zbus::object_server::SignalEmitter::new(&ctx.session_bus, path.clone()) else { return };
    while rx.changed().await.is_ok() {
        let now = rx.borrow_and_update().clone();
        let changed = now.changed_since(&last);
        if !changed.is_empty() {
            let map: HashMap<&str, Value<'_>> = changed.iter().map(|(k, v)| (*k, Value::from(v.clone()))).collect();
            if let Err(e) = zbus::fdo::Properties::properties_changed(&emitter, iface.clone(), map, (&[][..]).into()).await {
                tracing::debug!("PropertiesChanged: {e}");
            }
            let level = (now.state == SessionState::Connected).then(|| now.lowest_bud()).flatten();
            let old_level = (last.state == SessionState::Connected).then(|| last.lowest_bud()).flatten();
            if level != old_level {
                ctx.batteries.update(&bluez_path, level).await;
            }
        }
        last = now;
    }
}

struct Running {
    cmd: mpsc::Sender<SessionCmd>,
    task: JoinHandle<SessionEnd>,
    started: Instant,
}

/// A session that lasted this long counts as successful (resets the back-off).
const STABLE_SESSION: Duration = Duration::from_secs(10);
/// No `NewConnection` within this after `ConnectProfile` → give up this attempt.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Delay after the Bluetooth link comes up before opening the Maestro channel.
const SETTLE_DELAY: Duration = Duration::from_secs(1);
/// Poll of the PipeWire card profile while no A2DP transport exists.
const CARD_POLL: Duration = Duration::from_secs(10);
/// Minimum time between two automatic audio fixes.
const AUTOFIX_INTERVAL: Duration = Duration::from_secs(60);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

struct Supervisor {
    ctx: Arc<Ctx>,
    info: DeviceInfo,
    state: StateTx,
    session: Option<Running>,
    connecting: Option<JoinHandle<zbus::Result<()>>>,
    connect_at: Option<Instant>,
    connect_deadline: Option<Instant>,
    failures: u32,
    transports: HashMap<OwnedObjectPath, TransportInfo>,
    card_profile: Option<String>,
    card_check_at: Option<Instant>,
    fixing: Option<JoinHandle<Result<(), String>>>,
    fix_waiters: Vec<Reply>,
    last_autofix: Option<Instant>,
}

async fn join_opt<T>(h: &mut Option<JoinHandle<T>>) -> Result<T, tokio::task::JoinError> {
    match h {
        Some(h) => h.await,
        None => std::future::pending().await,
    }
}

async fn join_session(r: &mut Option<Running>) -> Result<SessionEnd, tokio::task::JoinError> {
    match r {
        Some(r) => (&mut r.task).await,
        None => std::future::pending().await,
    }
}

async fn sleep_opt(t: Option<Instant>) {
    match t {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending().await,
    }
}

impl Supervisor {
    fn new(ctx: Arc<Ctx>, info: DeviceInfo, state: StateTx) -> Self {
        let now = Instant::now();
        let connected = info.connected;
        Self {
            ctx,
            info,
            state,
            session: None,
            connecting: None,
            connect_at: connected.then_some(now),
            connect_deadline: None,
            failures: 0,
            transports: HashMap::new(),
            card_profile: None,
            card_check_at: connected.then_some(now),
            fixing: None,
            fix_waiters: Vec::new(),
            last_autofix: None,
        }
    }

    fn backoff(&self) -> Duration {
        let base = timing::RECONNECT_DELAY;
        if self.failures <= 1 {
            base
        } else {
            (base * 2u32.saturating_pow(self.failures.min(8) - 1)).min(MAX_BACKOFF)
        }
    }

    fn schedule_connect(&mut self, after: Duration) {
        if self.info.connected && self.session.is_none() {
            self.connect_at = Some(Instant::now() + after);
            self.state.send_modify(|s| {
                if s.state != SessionState::Disconnected {
                    s.state = SessionState::Reconnecting;
                }
            });
        }
    }

    fn drop_session(&mut self) {
        if let Some(r) = self.session.take() {
            r.task.abort();
        }
        if let Some(c) = self.connecting.take() {
            c.abort();
        }
        self.connect_deadline = None;
        self.state.send_modify(|s| s.reset_session());
    }

    async fn run(
        mut self,
        mut input: mpsc::UnboundedReceiver<Input>,
        mut cmds: mpsc::Receiver<Command>,
        mut profile: mpsc::UnboundedReceiver<ProfileEvent>,
    ) {
        loop {
            tokio::select! {
                Some(inp) = input.recv() => self.on_input(inp),
                Some(cmd) = cmds.recv() => self.on_command(cmd).await,
                Some(ev) = profile.recv() => self.on_profile(ev),
                end = join_session(&mut self.session) => self.on_session_end(end),
                r = join_opt(&mut self.connecting) => {
                    self.connecting = None;
                    self.on_connect_result(r);
                }
                _ = sleep_opt(self.connect_at) => {
                    self.connect_at = None;
                    self.start_connect().await;
                }
                _ = sleep_opt(self.connect_deadline) => {
                    self.connect_deadline = None;
                    tracing::warn!("{}: no Maestro channel within {CONNECT_TIMEOUT:?}", self.info.address);
                    if let Some(c) = self.connecting.take() {
                        c.abort();
                    }
                    self.failures += 1;
                    self.schedule_connect(self.backoff());
                }
                _ = sleep_opt(self.card_check_at) => {
                    self.card_check_at = None;
                    self.card_profile = audio::card_info(&self.info.address).await.map(|c| c.profile);
                    self.reevaluate_audio();
                }
                r = join_opt(&mut self.fixing) => {
                    self.fixing = None;
                    let r = r.unwrap_or_else(|e| Err(e.to_string()));
                    if let Err(e) = &r {
                        tracing::warn!("{}: audio fix failed: {e}", self.info.address);
                    }
                    for w in self.fix_waiters.drain(..) {
                        let _ = w.send(r.clone().map_err(ApiError::Failed));
                    }
                    self.card_check_at = Some(Instant::now());
                }
                else => return,
            }
        }
    }

    fn on_input(&mut self, inp: Input) {
        match inp {
            Input::Info(new) => {
                let was = self.info.connected;
                self.info = new;
                let (connected, alias) = (self.info.connected, self.info.alias.clone());
                self.state.send_modify(|s| {
                    s.name = alias;
                    s.bluetooth_connected = connected;
                });
                if connected && !was {
                    tracing::info!("{}: Bluetooth connected", self.info.address);
                    self.failures = 0;
                    self.connect_at = Some(Instant::now() + SETTLE_DELAY);
                    self.card_check_at = Some(Instant::now() + SETTLE_DELAY);
                } else if !connected && was {
                    tracing::info!("{}: Bluetooth disconnected", self.info.address);
                    self.connect_at = None;
                    self.card_check_at = None;
                    self.card_profile = None;
                    self.drop_session();
                    self.state.send_modify(|s| s.state = SessionState::Disconnected);
                    self.reevaluate_audio();
                }
            }
            Input::Transport(t) => {
                self.transports.insert(t.path.clone(), t);
                self.reevaluate_audio();
            }
            Input::TransportRemoved(p) => {
                self.transports.remove(&p);
                // The card may have fallen back to HFP: look soon.
                self.card_check_at = Some(Instant::now() + Duration::from_secs(2));
                self.reevaluate_audio();
            }
        }
    }

    fn reevaluate_audio(&mut self) {
        let status = audio::evaluate(&self.transports, self.card_profile.as_deref());
        let auto = self.state.borrow().auto_fix_audio;
        self.state.send_if_modified(|s| {
            let old = (s.audio_profile.clone(), s.a2dp_channel_mode.clone(), s.audio_problem.clone());
            s.audio_profile = status.profile.clone();
            s.a2dp_channel_mode = status.channel_mode.clone();
            s.audio_problem = status.problem.clone();
            old != (s.audio_profile.clone(), s.a2dp_channel_mode.clone(), s.audio_problem.clone())
        });
        if self.info.connected && status.profile != "a2dp" && self.card_check_at.is_none() {
            self.card_check_at = Some(Instant::now() + CARD_POLL);
        }
        // Only the Dual Mono case is fixed automatically: HFP may be a real call.
        if auto
            && status.problem == "dual_mono"
            && self.fixing.is_none()
            && self.last_autofix.is_none_or(|t| t.elapsed() >= AUTOFIX_INTERVAL)
        {
            tracing::info!("{}: Dual Mono stream detected, fixing automatically", self.info.address);
            self.last_autofix = Some(Instant::now());
            self.start_fix();
        }
    }

    fn start_fix(&mut self) {
        let system = self.ctx.system.clone();
        let (path, addr) = (self.info.path.clone(), self.info.address.clone());
        self.fixing = Some(tokio::spawn(async move { audio::fix(&system, &path, &addr).await }));
    }

    async fn on_command(&mut self, cmd: Command) {
        let session_cmd = match cmd {
            Command::SetAnc(m, r) => SessionCmd::Anc(m, r),
            Command::SetMultipoint(b, r) => SessionCmd::Multipoint(b, r),
            Command::SetExclusive(b, r) => SessionCmd::Exclusive(b, r),
            Command::FixAudio(r) => {
                if !self.info.connected {
                    let _ = r.send(Err(ApiError::NotConnected("buds not connected".into())));
                } else {
                    self.fix_waiters.push(r);
                    if self.fixing.is_none() {
                        self.start_fix();
                    }
                }
                return;
            }
            Command::SetAutoFixAudio(b) => {
                {
                    let mut cfg = self.ctx.config.lock().unwrap();
                    cfg.devices.entry(self.info.address.clone()).or_default().auto_fix_audio = b;
                    cfg.save();
                }
                self.state.send_modify(|s| s.auto_fix_audio = b);
                self.reevaluate_audio();
                return;
            }
        };
        match &self.session {
            Some(r) if !r.task.is_finished() => {
                if let Err(mpsc::error::SendError(c)) = r.cmd.send(session_cmd).await {
                    reply_not_connected(c);
                }
            }
            _ => reply_not_connected(session_cmd),
        }
    }

    fn on_profile(&mut self, ev: ProfileEvent) {
        match ev {
            ProfileEvent::Connected(fd) => {
                self.connect_deadline = None;
                if self.session.is_some() {
                    tracing::warn!("{}: new Maestro channel replaces the current one", self.info.address);
                    if let Some(r) = self.session.take() {
                        r.task.abort();
                    }
                    self.state.send_modify(|s| s.reset_session());
                }
                let stream = match FdStream::new(fd) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("{}: bad RFCOMM fd: {e}", self.info.address);
                        return;
                    }
                };
                let tap = self.ctx.frame_log.clone().map(|f| framelog::tap(f, &self.info.address));
                let (cmd_tx, cmd_rx) = mpsc::channel(16);
                let state = self.state.clone();
                let task = tokio::spawn(async move {
                    match Connection::open(stream, timing::FIRST_FRAME, tap).await {
                        Ok((conn, notes)) => session::run(conn, notes, state, cmd_rx).await,
                        Err(e) => SessionEnd::SetupFailed(e.to_string()),
                    }
                });
                self.session = Some(Running { cmd: cmd_tx, task, started: Instant::now() });
            }
            ProfileEvent::DisconnectRequested => {
                tracing::info!("{}: BlueZ requested disconnection of the Maestro channel", self.info.address);
                self.drop_session();
                self.schedule_connect(timing::RECONNECT_DELAY);
            }
        }
    }

    fn on_session_end(&mut self, end: Result<SessionEnd, tokio::task::JoinError>) {
        let started = self.session.take().map(|r| r.started);
        let end = end.map(|e| e.to_string()).unwrap_or_else(|e| format!("task: {e}"));
        tracing::info!("{}: Maestro session ended: {end}", self.info.address);
        self.state.send_modify(|s| s.reset_session());
        if started.is_some_and(|t| t.elapsed() >= STABLE_SESSION) {
            self.failures = 0;
        } else {
            self.failures += 1;
        }
        // The buds close the channel on every primary handover: reconnect after ~3 s like the app.
        self.schedule_connect(self.backoff());
        if !self.info.connected {
            self.state.send_modify(|s| s.state = SessionState::Disconnected);
        }
    }

    async fn start_connect(&mut self) {
        if !self.info.connected || self.session.is_some() || self.connecting.is_some() {
            return;
        }
        self.state.send_modify(|s| {
            s.state = if s.state == SessionState::Disconnected {
                SessionState::Connecting
            } else {
                SessionState::Reconnecting
            }
        });
        let system = self.ctx.system.clone();
        let path = self.info.path.clone();
        self.connecting = Some(tokio::spawn(async move {
            let dev = Device1Proxy::builder(&system).path(path)?.build().await?;
            match dev.connect_profile(MAESTRO_UUID).await {
                Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "org.bluez.Error.AlreadyConnected" => {
                    // Stale profile connection (e.g. after a daemon restart): drop and retry once.
                    let _ = dev.disconnect_profile(MAESTRO_UUID).await;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    dev.connect_profile(MAESTRO_UUID).await
                }
                r => r,
            }
        }));
        self.connect_deadline = Some(Instant::now() + CONNECT_TIMEOUT);
    }

    fn on_connect_result(&mut self, r: Result<zbus::Result<()>, tokio::task::JoinError>) {
        match r {
            Ok(Ok(())) => {} // The fd arrives through Profile1.NewConnection.
            Ok(Err(e)) => {
                // "Host is down", "InProgress", "busy" are normal while the buds reconnect.
                tracing::info!("{}: ConnectProfile failed: {e}", self.info.address);
                self.connect_deadline = None;
                if self.session.is_none() {
                    self.failures += 1;
                    self.schedule_connect(self.backoff());
                }
            }
            Err(e) if e.is_cancelled() => {}
            Err(e) => tracing::warn!("connect task: {e}"),
        }
    }
}

fn reply_not_connected(c: SessionCmd) {
    let e = || Err(ApiError::NotConnected("no Maestro session with the buds".into()));
    match c {
        SessionCmd::Anc(_, r) | SessionCmd::Multipoint(_, r) | SessionCmd::Exclusive(_, r) => {
            let _ = r.send(e());
        }
    }
}
