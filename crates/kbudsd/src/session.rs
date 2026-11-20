// SPDX-License-Identifier: GPL-3.0-or-later
//! One Maestro session on an open RFCOMM channel.
//!
//! Runs until the buds close the channel (every primary handover does) or the supervisor drops
//! it. The supervisor reconnects and starts a fresh session: host id, primary and subscriptions are
//! never carried over.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use kbuds_protocol::client::{Connection, Notification, StreamMsg, Subscription, timing};
use kbuds_protocol::engine::{RpcError, StreamEnd};
use kbuds_protocol::maestro::{self, RuntimeInfo, SoftwareInfo};
use kbuds_protocol::models::{self, Feature, FeatureState, Model};
use kbuds_protocol::rpc::{PacketType, Status};
use kbuds_protocol::settings::{self, AncMode, Setting, SettingId};
use kbuds_protocol::{Bud, ids};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::api::{ApiError, Reply};
use crate::state::{DeviceState, SessionState};

pub type StateTx = Arc<watch::Sender<DeviceState>>;

/// Commands a running session handles.
#[derive(Debug)]
pub enum SessionCmd {
    Anc(AncMode, Reply),
    Multipoint(bool, Reply),
    Exclusive(bool, Reply),
}

#[derive(Debug)]
pub enum SessionEnd {
    /// The buds closed the channel (EOF / ECONNRESET), or the stream failed.
    Closed(String),
    /// The session could not be set up.
    SetupFailed(String),
    /// The supervisor dropped the command channel.
    Stopped,
}

impl std::fmt::Display for SessionEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionEnd::Closed(why) => write!(f, "channel closed ({why})"),
            SessionEnd::SetupFailed(why) => write!(f, "setup failed: {why}"),
            SessionEnd::Stopped => f.write_str("stopped"),
        }
    }
}

/// Delay before re-reading ANC after a RuntimeInfo push (the app re-reads it on every placement
/// update; the buds switch to AWARE on their own with one bud out).
const ANC_REREAD_DELAY: Duration = Duration::from_millis(400);
/// Retry delay for a stream that the buds ended.
const RESUBSCRIBE_DELAY: Duration = Duration::from_secs(5);

fn rpc_to_api(e: RpcError, what: &str) -> ApiError {
    match e {
        RpcError::Status(Status::FailedPrecondition) => {
            ApiError::Unavailable(format!("{what}: buds in the case or not worn"))
        }
        RpcError::Timeout => ApiError::Timeout(format!("{what}: no answer from the buds")),
        RpcError::ConnectionClosed | RpcError::HostIdUnknown => {
            ApiError::NotConnected(format!("{what}: connection to the buds lost"))
        }
        e => ApiError::Failed(format!("{what}: {e}")),
    }
}

/// `ReadSetting` with the app's policy: 1 s per attempt, 3 attempts; non-OK status is final.
pub async fn read_setting(conn: &Connection, bud: Bud, id: SettingId) -> Result<Option<Setting>, RpcError> {
    let mut last = RpcError::Timeout;
    for _ in 0..timing::READ_SETTING_ATTEMPTS {
        match conn.read_setting(bud, id, timing::READ_SETTING).await {
            Err(RpcError::Timeout) => last = RpcError::Timeout,
            r => return r,
        }
    }
    Err(last)
}

/// `WriteSetting` with the app's policy (5 s, 5 attempts, 2 s apart), then read back.
pub async fn write_confirmed(conn: &Connection, bud: Bud, setting: &Setting) -> Result<(), ApiError> {
    let id = setting.id().ok_or_else(|| ApiError::NotSupported("setting is not writable".into()))?;
    let what = format!("{id:?}");
    let mut attempt = 0;
    loop {
        attempt += 1;
        match conn.write_setting(bud, setting, timing::WRITE_SETTING).await {
            Ok(()) => break,
            Err(RpcError::Timeout) if attempt < timing::WRITE_SETTING_ATTEMPTS => {
                tokio::time::sleep(timing::WRITE_SETTING_RETRY_DELAY).await;
            }
            Err(e) => return Err(rpc_to_api(e, &what)),
        }
    }
    // The buds send no push for our own write: the read-back is the confirmation.
    match read_setting(conn, bud, id).await {
        Ok(Some(v)) if &v == setting => Ok(()),
        Ok(v) => Err(ApiError::Failed(format!("{what}: buds kept {v:?} after the write"))),
        Err(e) => Err(rpc_to_api(e, &format!("{what} read-back"))),
    }
}

fn apply_setting(state: &StateTx, s: &Setting) {
    state.send_if_modified(|st| {
        let old = st.clone();
        match s {
            Setting::Anc(Some(m)) => st.anc = m.as_str().into(),
            Setting::Anc(None) => st.anc = "unavailable".into(),
            Setting::Multipoint(b) => st.multipoint = *b,
            _ => {}
        }
        *st != old
    });
}

fn apply_runtime(state: &StateTx, r: &RuntimeInfo) {
    state.send_if_modified(|st| {
        let old = st.clone();
        if let Some(b) = r.battery {
            let level = |b: Option<maestro::Battery>| b.and_then(|b| b.percent).map(i32::from).unwrap_or(-1);
            st.battery_left = level(b.left);
            st.battery_right = level(b.right);
            st.battery_case = level(b.case);
            st.charging_left = b.left.is_some_and(|b| b.charging);
            st.charging_right = b.right.is_some_and(|b| b.charging);
            st.charging_case = b.case.is_some_and(|b| b.charging);
        }
        if let Some(p) = r.placement {
            st.placement_left = p.left.map(|p| p.as_str().to_string()).unwrap_or_default();
            st.placement_right = p.right.map(|p| p.as_str().to_string()).unwrap_or_default();
        }
        *st != old
    });
}

/// Which ANC modes the gate allows (empty = no ANC).
fn anc_modes(device_type: i32, fw: Option<u64>, readable: bool) -> Vec<String> {
    let anc = models::feature_state(device_type, fw, Feature::Anc);
    let ok = anc == FeatureState::Supported || (anc == FeatureState::Unknown && readable);
    if !ok {
        return Vec::new();
    }
    let mut modes = vec![AncMode::Off, AncMode::Active, AncMode::Aware];
    if models::feature_state(device_type, fw, Feature::AdaptiveAnc) == FeatureState::Supported {
        modes.push(AncMode::Adaptive);
    }
    modes.into_iter().map(|m| m.as_str().to_string()).collect()
}

/// Primary detection.
async fn find_primary(conn: &Connection, hello_from: Option<Bud>) -> Result<(Bud, SoftwareInfo), String> {
    let (l, r) = tokio::join!(conn.software_info(Bud::Left), conn.software_info(Bud::Right));
    for (bud, res) in [(Bud::Left, &l), (Bud::Right, &r)] {
        if let Ok(info) = res
            && !info.is_secondary
        {
            return Ok((bud, info.clone()));
        }
    }
    // Neither claims to be primary: trust the bud that sent the hello frame.
    if let Some(bud) = hello_from {
        let info = if bud == Bud::Left { l.clone() } else { r.clone() };
        if let Ok(info) = info {
            return Ok((bud, info));
        }
    }
    Err(format!("no primary bud (left: {l:?}, right: {r:?})"))
}

struct Streams {
    runtime: Option<Subscription>,
    quiet: Option<Subscription>,
    settings: HashMap<Bud, Subscription>,
}

async fn next_or_pending(sub: &mut Option<Subscription>) -> StreamMsg {
    match sub {
        Some(s) => s.next().await.unwrap_or(StreamMsg::End(StreamEnd::ConnectionClosed)),
        None => std::future::pending().await,
    }
}

async fn next_settings(subs: &mut HashMap<Bud, Subscription>) -> (Bud, StreamMsg) {
    if subs.is_empty() {
        return std::future::pending().await;
    }
    let futs = subs.iter_mut().map(|(bud, s)| {
        let bud = *bud;
        Box::pin(async move { (bud, s.next().await.unwrap_or(StreamMsg::End(StreamEnd::ConnectionClosed))) })
    });
    futures_util::future::select_all(futs).await.0
}

async fn sleep_until_opt(t: Option<Instant>) {
    match t {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending().await,
    }
}

/// Runs one session. `notes` must be the receiver returned with `conn`.
pub async fn run(
    conn: Connection,
    mut notes: mpsc::UnboundedReceiver<Notification>,
    state: StateTx,
    mut cmds: mpsc::Receiver<SessionCmd>,
) -> SessionEnd {
    let host = conn.host_id();
    state.send_modify(|s| s.host_id = host.to_string());
    tracing::info!("Maestro session up, host id {host}");

    // The hello frame (unsolicited GetSoftwareInfo RESPONSE) comes from the primary.
    let mut hello_from = None;
    while let Ok(n) = notes.try_recv() {
        if let Notification::Unsolicited { from: Some(e), packet } = n
            && packet.method_id == ids::GET_SOFTWARE_INFO.method_id
        {
            hello_from = Bud::from_endpoint(e);
        }
    }

    let (primary, sw) = match find_primary(&conn, hello_from).await {
        Ok(p) => p,
        Err(e) => return SessionEnd::SetupFailed(e),
    };
    tracing::info!("primary bud: {primary}");
    let hw = match conn.hardware_info(primary).await {
        Ok(hw) => Some(hw),
        Err(e) => {
            tracing::warn!("GetHardwareInfo failed: {e}");
            None
        }
    };
    let device_type = hw.as_ref().map(|h| h.device_type).unwrap_or(0);
    let fw = models::firmware_number(sw.versions.left.as_deref(), sw.versions.right.as_deref());
    let model = Model::from_device_type(device_type);
    state.send_modify(|s| {
        s.primary = primary.to_string();
        s.firmware_left = sw.versions.left.clone().unwrap_or_default();
        s.firmware_right = sw.versions.right.clone().unwrap_or_default();
        s.firmware_case = sw.versions.case.clone().unwrap_or_default();
        if let Some(hw) = &hw {
            s.device_type = hw.device_type.max(0) as u32;
            s.sku = hw.sku;
            s.model = model.as_str().into();
            s.serial_left = hw.serial_left.clone().unwrap_or_default();
            s.serial_right = hw.serial_right.clone().unwrap_or_default();
            s.serial_case = hw.serial_case.clone().unwrap_or_default();
        }
    });
    if matches!(model, Model::Legacy) {
        return SessionEnd::SetupFailed("legacy model on a Pigweed channel".into());
    }

    tracing::debug!("hardware: type {device_type} ({}), firmware {fw:?}", model.as_str());
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    if let Err(e) = conn.set_wallclock(primary, now_ms).await {
        tracing::debug!("SetWallclock: {e}");
    }

    // Subscriptions.
    let mut streams = Streams { runtime: None, quiet: None, settings: HashMap::new() };
    let subscribe = async |target: Bud, m: ids::Method, call_id: bool| -> Option<Subscription> {
        match conn.subscribe(target.endpoint(), m, maestro::empty(), call_id).await {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!("{} on {target}: {e}", m.name);
                None
            }
        }
    };
    streams.runtime = subscribe(primary, ids::SUBSCRIBE_RUNTIME_INFO, false).await;
    streams.quiet = subscribe(primary, ids::SUBSCRIBE_QUIET_MODE, true).await;
    for bud in [Bud::Left, Bud::Right] {
        // Required on MAESTRO_B, harmless on MAESTRO_A.
        if let Some(s) = subscribe(bud, ids::SUBSCRIBE_SETTINGS_CHANGES, true).await {
            streams.settings.insert(bud, s);
        }
    }
    if conn.is_closed() {
        return SessionEnd::Closed("closed during setup".into());
    }

    // Initial reads, one at a time, status checked.
    tracing::debug!("subscriptions open; reading settings");
    let anc_read = read_setting(&conn, primary, SettingId::Anc).await;
    let mp_read = read_setting(&conn, primary, SettingId::Multipoint).await;
    let modes = anc_modes(device_type, fw, matches!(anc_read, Ok(Some(_))));
    state.send_modify(|s| {
        s.anc_modes = modes.clone();
        // Multipoint: gated by feature 25 on Pro 2/2a; on the Pro the write is confirmed on
        // hardware although the app hides it, so a readable setting is enough.
        s.multipoint_supported = matches!(mp_read, Ok(Some(Setting::Multipoint(_))));
    });
    for r in [&anc_read, &mp_read] {
        match r {
            Ok(Some(s)) => apply_setting(&state, s),
            Ok(None) => {}
            Err(e) => tracing::debug!("initial read: {e}"),
        }
    }
    state.send_modify(|s| s.state = SessionState::Connected);
    {
        let s = state.borrow();
        tracing::info!(
            "session ready: {} fw {}/{}, battery L {} R {} case {}, ANC {:?} {:?}, multipoint {}",
            model.marketing_name(),
            s.firmware_left,
            s.firmware_right,
            s.battery_left,
            s.battery_right,
            s.battery_case,
            s.anc,
            s.anc_modes,
            if s.multipoint_supported { if s.multipoint { "on" } else { "off" } } else { "n/a" },
        );
        if let Err(e) = &anc_read {
            tracing::info!("initial ANC read failed: {e}");
        }
        if let Err(e) = &mp_read {
            tracing::info!("initial multipoint read failed: {e}");
        }
    }

    let mut anc_reread: Option<Instant> = None;
    let mut resubscribe: Option<Instant> = None;
    let mut writes: HashMap<SettingId, (oneshot::Sender<()>, JoinHandle<()>)> = HashMap::new();

    let end = loop {
        tokio::select! {
            msg = next_or_pending(&mut streams.runtime) => match msg {
                StreamMsg::Item(p) => match RuntimeInfo::decode(&p) {
                    Ok(r) => {
                        apply_runtime(&state, &r);
                        if r.placement.is_some() {
                            anc_reread = Some(Instant::now() + ANC_REREAD_DELAY);
                        }
                    }
                    Err(e) => tracing::warn!("bad RuntimeInfo: {e}"),
                },
                StreamMsg::End(end) => {
                    tracing::info!("RuntimeInfo stream ended: {end:?}");
                    streams.runtime = None;
                    resubscribe = Some(Instant::now() + RESUBSCRIBE_DELAY);
                }
            },
            msg = next_or_pending(&mut streams.quiet) => match msg {
                StreamMsg::Item(p) => match maestro::decode_quiet_mode(&p) {
                    Ok(q) => {
                        state.send_if_modified(|s| std::mem::replace(&mut s.other_host_has_audio, q) != q);
                    }
                    Err(e) => tracing::warn!("bad QuietModeStatus: {e}"),
                },
                StreamMsg::End(end) => {
                    tracing::info!("quiet-mode stream ended: {end:?}");
                    streams.quiet = None;
                    resubscribe = Some(Instant::now() + RESUBSCRIBE_DELAY);
                }
            },
            (bud, msg) = next_settings(&mut streams.settings) => match msg {
                StreamMsg::Item(p) => match settings::decode(&p) {
                    Ok(Some(s)) => apply_setting(&state, &s),
                    Ok(None) => {}
                    Err(e) => tracing::warn!("bad settings push: {e}"),
                },
                StreamMsg::End(end) => {
                    // Either bud may end it at once with OK: not an error. We fall back to
                    // re-reading on RuntimeInfo pushes.
                    tracing::debug!("settings stream on {bud} ended: {end:?}");
                    streams.settings.remove(&bud);
                }
            },
            note = notes.recv() => match note {
                None => break SessionEnd::Closed("driver stopped".into()),
                Some(Notification::Closed(e)) => {
                    break SessionEnd::Closed(e.map(|e| e.to_string()).unwrap_or_else(|| "EOF".into()));
                }
                Some(Notification::Unsolicited { packet, from }) => {
                    // MAESTRO_A gets settings pushes without a subscription; any bud may push.
                    if packet.kind == PacketType::ServerStream
                        && packet.method_id == ids::SUBSCRIBE_SETTINGS_CHANGES.method_id
                        && packet.service_id == ids::SUBSCRIBE_SETTINGS_CHANGES.service_id
                    {
                        if let Ok(Some(s)) = settings::decode(&packet.payload) {
                            apply_setting(&state, &s);
                        }
                    } else {
                        tracing::trace!("ignoring unsolicited {:?} from {from:?}", packet.kind);
                    }
                }
                Some(Notification::Invalid(why)) => tracing::debug!("{why}"),
            },
            cmd = cmds.recv() => match cmd {
                None => break SessionEnd::Stopped,
                Some(SessionCmd::Anc(mode, reply)) => {
                    let st = state.borrow().clone();
                    if !st.anc_modes.iter().any(|m| m == mode.as_str()) {
                        let _ = reply.send(Err(ApiError::NotSupported(format!("ANC mode {mode} not supported by this model/firmware"))));
                    } else if st.anc == "unavailable" {
                        let _ = reply.send(Err(ApiError::Unavailable("ANC: buds in the case".into())));
                    } else {
                        spawn_write(&mut writes, &conn, primary, &state, Setting::Anc(Some(mode)), reply);
                    }
                }
                Some(SessionCmd::Multipoint(on, reply)) => {
                    if !state.borrow().multipoint_supported {
                        let _ = reply.send(Err(ApiError::NotSupported("multipoint not available".into())));
                    } else {
                        spawn_write(&mut writes, &conn, primary, &state, Setting::Multipoint(on), reply);
                    }
                }
                Some(SessionCmd::Exclusive(on, reply)) => {
                    // Start and release must happen in this session (confirmed on hardware).
                    let r = conn.force_multipoint_switch(primary, !on).await;
                    let r = r.map(|()| {
                        state.send_modify(|s| {
                            s.exclusive = on;
                            if on {
                                s.other_host_has_audio = false;
                            }
                        });
                    }).map_err(|e| rpc_to_api(e, "ForceMultipointSwitch"));
                    let _ = reply.send(r);
                }
            },
            _ = sleep_until_opt(anc_reread) => {
                anc_reread = None;
                writes.retain(|_, (_, t)| !t.is_finished());
                if !writes.contains_key(&SettingId::Anc) {
                    match read_setting(&conn, primary, SettingId::Anc).await {
                        Ok(Some(s)) => apply_setting(&state, &s),
                        Ok(None) => {}
                        Err(e) => tracing::debug!("ANC re-read: {e}"),
                    }
                }
            },
            _ = sleep_until_opt(resubscribe) => {
                resubscribe = None;
                if streams.runtime.is_none() {
                    streams.runtime = subscribe(primary, ids::SUBSCRIBE_RUNTIME_INFO, false).await;
                }
                if streams.quiet.is_none() {
                    streams.quiet = subscribe(primary, ids::SUBSCRIBE_QUIET_MODE, true).await;
                }
            },
        }
    };
    for (_, (_, task)) in writes.drain() {
        task.abort();
    }
    end
}

/// Runs a confirmed write in the background; a newer write to the same setting cancels it.
fn spawn_write(
    writes: &mut HashMap<SettingId, (oneshot::Sender<()>, JoinHandle<()>)>,
    conn: &Connection,
    bud: Bud,
    state: &StateTx,
    setting: Setting,
    reply: Reply,
) {
    let id = setting.id().expect("writable");
    writes.retain(|_, (_, t)| !t.is_finished());
    if let Some((cancel, _)) = writes.remove(&id) {
        let _ = cancel.send(());
    }
    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    let conn = conn.clone();
    let state = state.clone();
    let task = tokio::spawn(async move {
        tokio::select! {
            r = write_confirmed(&conn, bud, &setting) => {
                if r.is_ok() {
                    apply_setting(&state, &setting);
                }
                let _ = reply.send(r);
            }
            _ = cancel_rx => {
                let _ = reply.send(Err(ApiError::Failed("superseded by a newer write".into())));
            }
        }
    });
    writes.insert(id, (cancel_tx, task));
}

#[cfg(test)]
mod tests {
    use super::*;
    use kbuds_protocol::{Endpoint, HostId};
    use kbuds_protocol::testing::{self, FakeHandle, FakeState};

    struct Rig {
        state: StateTx,
        rx: watch::Receiver<DeviceState>,
        cmds: mpsc::Sender<SessionCmd>,
        fake: FakeHandle,
        task: JoinHandle<SessionEnd>,
    }

    async fn rig(fake_state: FakeState) -> Rig {
        let (host_io, dev_io) = tokio::io::duplex(8192);
        let fake = testing::spawn(dev_io, fake_state);
        let (conn, notes) = Connection::open(host_io, timing::FIRST_FRAME, None).await.unwrap();
        let (tx, rx) = watch::channel(DeviceState::new("AA:BB:CC:DD:EE:FF", "Pixel Buds Pro"));
        let state = Arc::new(tx);
        let (cmds, cmd_rx) = mpsc::channel(8);
        let task = tokio::spawn(run(conn, notes, state.clone(), cmd_rx));
        Rig { state, rx, cmds, fake, task }
    }

    async fn until(rx: &mut watch::Receiver<DeviceState>, f: impl Fn(&DeviceState) -> bool) -> DeviceState {
        tokio::time::timeout(Duration::from_secs(5), rx.wait_for(|s| f(s))).await.expect("state reached").unwrap().clone()
    }

    async fn cmd(r: &Rig, f: impl FnOnce(Reply) -> SessionCmd) -> Result<(), ApiError> {
        let (tx, rx) = oneshot::channel();
        r.cmds.send(f(tx)).await.unwrap();
        rx.await.unwrap()
    }

    #[tokio::test]
    async fn setup_publishes_everything() {
        for host in [HostId::A, HostId::B] {
            let mut r = rig(FakeState { host, primary: Bud::Right, ..Default::default() }).await;
            let s = until(&mut r.rx, |s| s.state == SessionState::Connected).await;
            assert_eq!(s.host_id, host.to_string());
            assert_eq!(s.primary, "right");
            assert_eq!(s.model, "pro");
            assert_eq!((s.battery_left, s.battery_right, s.battery_case), (80, 75, -1));
            assert_eq!(s.anc, "active");
            assert_eq!(s.anc_modes, ["off", "active", "aware"]);
            assert!(s.multipoint_supported && !s.multipoint);
            assert_eq!(s.firmware_left, "575");
            assert_eq!(s.serial_left, "LEFT0");
            // Calls went to the primary; settings subscriptions to both buds with call ids.
            let rx = r.fake.received.lock().unwrap().clone();
            let subs: Vec<_> = rx.iter().filter(|x| x.method == "SubscribeToSettingsChanges").collect();
            assert_eq!(subs.len(), 2);
            assert!(subs.iter().all(|x| x.call_id != 0));
            assert!(rx.iter().filter(|x| x.method == "ReadSetting").all(|x| x.to == Endpoint::RightBtCore));
            let _ = r.state;
        }
    }

    #[tokio::test]
    async fn runtime_and_quiet_pushes() {
        let mut r = rig(FakeState::default()).await;
        until(&mut r.rx, |s| s.state == SessionState::Connected).await;
        {
            let mut st = r.fake.state.lock().unwrap();
            st.battery[0] = Some((42, true));
            st.battery[2] = Some((90, false));
            st.in_case[1] = true;
            st.quiet_mode = true;
        }
        r.fake.push_runtime();
        r.fake.push_quiet_mode();
        let s = until(&mut r.rx, |s| s.battery_left == 42 && s.other_host_has_audio).await;
        assert!(s.charging_left);
        assert_eq!(s.battery_case, 90);
        assert_eq!((s.placement_left.as_str(), s.placement_right.as_str()), ("out", "in_case"));
    }

    #[tokio::test]
    async fn settings_push_from_other_host() {
        for host in [HostId::A, HostId::B] {
            let mut r = rig(FakeState { host, ..Default::default() }).await;
            until(&mut r.rx, |s| s.state == SessionState::Connected).await;
            r.fake.push_setting(&Setting::Anc(Some(AncMode::Aware)));
            until(&mut r.rx, |s| s.anc == "aware").await;
        }
    }

    #[tokio::test]
    async fn set_anc_confirms_and_maps_errors() {
        let mut r = rig(FakeState::default()).await;
        until(&mut r.rx, |s| s.state == SessionState::Connected).await;
        cmd(&r, |t| SessionCmd::Anc(AncMode::Aware, t)).await.unwrap();
        assert_eq!(r.rx.borrow().anc, "aware");
        assert_eq!(r.fake.state.lock().unwrap().anc, 3);
        assert!(matches!(cmd(&r, |t| SessionCmd::Anc(AncMode::Adaptive, t)).await, Err(ApiError::NotSupported(_))));
        // Buds go into the case: re-read after the RuntimeInfo push shows "unavailable".
        r.fake.state.lock().unwrap().in_case = [true, true];
        r.fake.push_runtime();
        until(&mut r.rx, |s| s.anc == "unavailable").await;
        assert!(matches!(cmd(&r, |t| SessionCmd::Anc(AncMode::Off, t)).await, Err(ApiError::Unavailable(_))));
    }

    #[tokio::test]
    async fn write_failed_precondition_is_unavailable() {
        let mut r = rig(FakeState::default()).await;
        until(&mut r.rx, |s| s.state == SessionState::Connected).await;
        r.fake.state.lock().unwrap().setting_status.insert(13, Status::FailedPrecondition);
        assert!(matches!(cmd(&r, |t| SessionCmd::Anc(AncMode::Off, t)).await, Err(ApiError::Unavailable(_))));
    }

    #[tokio::test]
    async fn multipoint_and_exclusive() {
        let mut r = rig(FakeState { quiet_mode: true, ..Default::default() }).await;
        until(&mut r.rx, |s| s.state == SessionState::Connected && s.other_host_has_audio).await;
        cmd(&r, |t| SessionCmd::Multipoint(true, t)).await.unwrap();
        assert!(r.rx.borrow().multipoint);
        cmd(&r, |t| SessionCmd::Exclusive(true, t)).await.unwrap();
        let s = r.rx.borrow().clone();
        assert!(s.exclusive && !s.other_host_has_audio);
        assert!(r.fake.state.lock().unwrap().exclusive);
        cmd(&r, |t| SessionCmd::Exclusive(false, t)).await.unwrap();
        assert!(!r.fake.state.lock().unwrap().exclusive);
    }

    #[tokio::test]
    async fn ends_when_buds_close() {
        let mut r = rig(FakeState::default()).await;
        until(&mut r.rx, |s| s.state == SessionState::Connected).await;
        r.fake.close();
        let end = tokio::time::timeout(Duration::from_secs(5), r.task).await.unwrap().unwrap();
        assert!(matches!(end, SessionEnd::Closed(_)), "{end:?}");
    }

    #[tokio::test]
    async fn no_primary_fails_setup() {
        let r = rig(FakeState { silent: vec!["GetSoftwareInfo"], ..Default::default() }).await;
        // The hello names the primary, but its GetSoftwareInfo times out: no primary info.
        let end = tokio::time::timeout(Duration::from_secs(10), r.task).await.unwrap().unwrap();
        assert!(matches!(end, SessionEnd::SetupFailed(_)), "{end:?}");
    }
}
