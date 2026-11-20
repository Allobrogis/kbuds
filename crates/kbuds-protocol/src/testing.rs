// SPDX-License-Identifier: GPL-3.0-or-later
//! A scripted fake Pixel Buds Pro for tests (feature `testing`). It serves the device side of an
//! in-memory stream.
//!
//! Behaviour modelled on the hardware findings:
//! * sends the unsolicited `GetSoftwareInfo` hello (call_id 0xFFFFFFFF) from the primary first;
//! * answers on the bud the request was addressed to; only the primary has runtime/settings state;
//! * `SubscribeToSettingsChanges` on the secondary ends at once with RESPONSE OK;
//! * ANC reads 0 and ANC writes fail with FAILED_PRECONDITION while both buds are in the case;
//! * no settings push for the host's own write;
//! * unknown methods get SERVER_ERROR NOT_FOUND.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use prost::Message;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::mpsc;

use crate::address::{Bud, Endpoint, HostId};
use crate::hdlc::{self, Decoded, Reassembler};
use crate::ids::{self, Method};
use crate::proto::maestro_pw as pb;
use crate::rpc::{Packet, PacketType, Status};
use crate::settings::{self, Setting, SettingId};

/// Device state; tests may change it through [`FakeHandle::state`].
#[derive(Clone, Debug)]
pub struct FakeState {
    pub host: HostId,
    pub primary: Bud,
    pub device_type: i32,
    pub firmware: String,
    pub battery: [Option<(u32, bool)>; 3], // left, right, case
    pub in_case: [bool; 2],                // left, right
    pub anc: i32,
    pub multipoint: bool,
    pub quiet_mode: bool,
    pub exclusive: bool,
    /// Status returned to ReadSetting/WriteSetting instead of the normal answer, per field.
    pub setting_status: HashMap<i32, Status>,
    /// Do not answer these methods at all (timeouts).
    pub silent: Vec<&'static str>,
}

impl Default for FakeState {
    fn default() -> Self {
        Self {
            host: HostId::A,
            primary: Bud::Left,
            device_type: 4,
            firmware: "575".into(),
            battery: [Some((80, false)), Some((75, false)), None],
            in_case: [false, false],
            anc: 2,
            multipoint: false,
            quiet_mode: false,
            exclusive: false,
            setting_status: HashMap::new(),
            silent: Vec::new(),
        }
    }
}

/// A request the fake received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Received {
    pub to: Endpoint,
    pub method: &'static str,
    pub kind: PacketType,
    pub payload: Vec<u8>,
    pub call_id: u32,
}

enum Control {
    PushRuntime,
    PushSetting(Vec<u8>),
    PushQuietMode,
    Close,
}

/// Test-side handle.
#[derive(Clone)]
pub struct FakeHandle {
    pub state: Arc<Mutex<FakeState>>,
    pub received: Arc<Mutex<Vec<Received>>>,
    ctl: mpsc::UnboundedSender<Control>,
}

impl FakeHandle {
    /// Sends a RuntimeInfo push with the current state on open runtime streams.
    pub fn push_runtime(&self) {
        let _ = self.ctl.send(Control::PushRuntime);
    }
    /// A settings change made by another host: pushed on every open settings stream, and without
    /// subscription when this host is MAESTRO_A.
    pub fn push_setting(&self, s: &Setting) {
        let v = settings::write_request(s).unwrap();
        let _ = self.ctl.send(Control::PushSetting(v));
    }
    pub fn push_quiet_mode(&self) {
        let _ = self.ctl.send(Control::PushQuietMode);
    }
    /// Closes the link like a primary handover (ECONNRESET on Linux is EOF here).
    pub fn close(&self) {
        let _ = self.ctl.send(Control::Close);
    }
    /// Names of the methods received, in order.
    pub fn methods(&self) -> Vec<&'static str> {
        self.received.lock().unwrap().iter().map(|r| r.method).collect()
    }
}

struct Stream {
    from: Endpoint,
    channel: u32,
    method: Method,
    call_id: u32,
}

/// Starts a fake on the device side `io`. Returns the handle; the task ends when the stream does.
pub fn spawn(io: DuplexStream, state: FakeState) -> FakeHandle {
    let (ctl, ctl_rx) = mpsc::unbounded_channel();
    let h = FakeHandle {
        state: Arc::new(Mutex::new(state)),
        received: Arc::new(Mutex::new(Vec::new())),
        ctl,
    };
    tokio::spawn(run(io, h.clone(), ctl_rx));
    h
}

fn frame(from: Endpoint, host: HostId, p: &Packet) -> Vec<u8> {
    hdlc::encode(hdlc::address(from, host.endpoint()), &p.encode())
}

fn reply(kind: PacketType, req: &Packet, payload: Vec<u8>, status: Status, call_id: u32) -> Packet {
    Packet {
        kind,
        channel: req.channel,
        service_id: req.service_id,
        method_id: req.method_id,
        payload,
        status,
        call_id,
    }
}

fn software_info(st: &FakeState, bud: Bud) -> Vec<u8> {
    let v = |s: &str| Some(pb::ComponentVersion { version: Some(s.into()), display_label: None });
    pb::SoftwareInfo {
        wallclock_ms: None,
        versions: Some(pb::software_info::Versions::VersionsB(pb::FirmwareVersions {
            case: v("12"),
            left: v(&st.firmware),
            right: v(&st.firmware),
        })),
        random_id: None,
        is_secondary: Some(bud != st.primary),
    }
    .encode_to_vec()
}

fn runtime_info(st: &FakeState) -> Vec<u8> {
    let lvl = |b: Option<(u32, bool)>| {
        b.map(|(p, c)| pb::BatteryLevel { level_percent: Some(p), charging: Some(if c { 2 } else { 1 }) })
    };
    pb::RuntimeInfo {
        wallclock_ms: Some(1),
        battery: Some(pb::runtime_info::Battery::BatteryB(pb::BatteryInfo {
            left: lvl(st.battery[0]),
            right: lvl(st.battery[1]),
            case: lvl(st.battery[2]),
        })),
        placement: Some(pb::runtime_info::Placement::PlacementB(pb::BudPlacementB {
            left_in_case: Some(st.in_case[0]),
            right_in_case: Some(st.in_case[1]),
            low_latency_mode: None,
        })),
    }
    .encode_to_vec()
}

fn quiet(st: &FakeState) -> Vec<u8> {
    pb::QuietModeStatus { quiet_mode: Some(st.quiet_mode) }.encode_to_vec()
}

fn all_in_case(st: &FakeState) -> bool {
    st.in_case[0] && st.in_case[1]
}

async fn run(mut io: DuplexStream, h: FakeHandle, mut ctl: mpsc::UnboundedReceiver<Control>) {
    let host = h.state.lock().unwrap().host;
    let mut streams: Vec<Stream> = Vec::new();
    // Hello: unsolicited GetSoftwareInfo RESPONSE from the primary, call_id 0xFFFFFFFF.
    {
        let st = h.state.lock().unwrap().clone();
        let m = ids::GET_SOFTWARE_INFO;
        let p = Packet {
            kind: PacketType::Response,
            channel: host.channel(st.primary.endpoint()).unwrap(),
            service_id: m.service_id,
            method_id: m.method_id,
            payload: software_info(&st, st.primary),
            status: Status::Ok,
            call_id: u32::MAX,
        };
        if io.write_all(&frame(st.primary.endpoint(), host, &p)).await.is_err() {
            return;
        }
    }
    let mut reasm = Reassembler::new();
    let mut buf = vec![0u8; 4096];
    loop {
        let mut out: Vec<Vec<u8>> = Vec::new();
        tokio::select! {
            r = io.read(&mut buf) => {
                let n = match r { Ok(0) | Err(_) => return, Ok(n) => n };
                for item in reasm.feed(&buf[..n]) {
                    let Decoded::Frame(f) = item else { continue };
                    let Some(to) = Endpoint::from_id(f.dst()) else { continue };
                    let Ok(req) = Packet::decode(&f.payload) else { continue };
                    handle_request(&h, host, to, req, &mut streams, &mut out);
                }
            }
            c = ctl.recv() => {
                let st = h.state.lock().unwrap().clone();
                match c {
                    None | Some(Control::Close) => return,
                    Some(Control::PushRuntime) => {
                        for s in streams.iter().filter(|s| s.method == ids::SUBSCRIBE_RUNTIME_INFO) {
                            let p = Packet { kind: PacketType::ServerStream, channel: s.channel, service_id: s.method.service_id,
                                method_id: s.method.method_id, payload: runtime_info(&st), status: Status::Ok, call_id: s.call_id };
                            out.push(frame(s.from, host, &p));
                        }
                    }
                    Some(Control::PushQuietMode) => {
                        for s in streams.iter().filter(|s| s.method == ids::SUBSCRIBE_QUIET_MODE) {
                            let p = Packet { kind: PacketType::ServerStream, channel: s.channel, service_id: s.method.service_id,
                                method_id: s.method.method_id, payload: quiet(&st), status: Status::Ok, call_id: s.call_id };
                            out.push(frame(s.from, host, &p));
                        }
                    }
                    Some(Control::PushSetting(v)) => {
                        let subs: Vec<_> = streams.iter().filter(|s| s.method == ids::SUBSCRIBE_SETTINGS_CHANGES).collect();
                        let m = ids::SUBSCRIBE_SETTINGS_CHANGES;
                        if subs.is_empty() && host == HostId::A {
                            // MAESTRO_A gets pushes without a subscription, without call_id.
                            let from = st.primary.endpoint();
                            let p = Packet { kind: PacketType::ServerStream, channel: host.channel(from).unwrap(),
                                service_id: m.service_id, method_id: m.method_id, payload: v.clone(), status: Status::Ok, call_id: 0 };
                            out.push(frame(from, host, &p));
                        }
                        for s in subs {
                            let p = Packet { kind: PacketType::ServerStream, channel: s.channel, service_id: m.service_id,
                                method_id: m.method_id, payload: v.clone(), status: Status::Ok, call_id: s.call_id };
                            out.push(frame(s.from, host, &p));
                        }
                    }
                }
            }
        }
        for f in out {
            if io.write_all(&f).await.is_err() {
                return;
            }
        }
    }
}

fn handle_request(
    h: &FakeHandle,
    host: HostId,
    to: Endpoint,
    req: Packet,
    streams: &mut Vec<Stream>,
    out: &mut Vec<Vec<u8>>,
) {
    let method = ids::lookup(req.service_id, req.method_id).copied();
    h.received.lock().unwrap().push(Received {
        to,
        method: method.map(|m| m.name).unwrap_or("?"),
        kind: req.kind,
        payload: req.payload.clone(),
        call_id: req.call_id,
    });
    if req.kind == PacketType::ClientError {
        streams.retain(|s| !(s.channel == req.channel && s.method.method_id == req.method_id));
        return;
    }
    if req.kind != PacketType::Request {
        return;
    }
    let Some(bud) = Bud::from_endpoint(to) else { return }; // case and sensor hubs never answer
    let Some(method) = method else {
        out.push(frame(to, host, &reply(PacketType::ServerError, &req, vec![], Status::NotFound, req.call_id)));
        return;
    };
    let mut st = h.state.lock().unwrap();
    if st.silent.contains(&method.name) {
        return;
    }
    let primary = bud == st.primary;
    let ok = |payload: Vec<u8>| reply(PacketType::Response, &req, payload, Status::Ok, req.call_id);
    let status = |s: Status| reply(PacketType::Response, &req, vec![], s, req.call_id);
    let item = |payload: Vec<u8>| reply(PacketType::ServerStream, &req, payload, Status::Ok, req.call_id);
    let add_stream = |streams: &mut Vec<Stream>| {
        streams.retain(|s| !(s.channel == req.channel && s.method == method));
        streams.push(Stream { from: to, channel: req.channel, method, call_id: req.call_id });
    };
    let p = match method.name {
        "GetSoftwareInfo" => ok(software_info(&st, bud)),
        "GetHardwareInfo" => ok(pb::HardwareInfo {
            device_type: Some(st.device_type),
            sku: Some(1),
            serials: Some(pb::hardware_info::Serials::SerialsB(pb::SerialNumbers {
                case: Some("CASE0".into()),
                right: Some("RIGHT0".into()),
                left: Some("LEFT0".into()),
            })),
            ..Default::default()
        }
        .encode_to_vec()),
        "SetWallclock" => ok(vec![]),
        "SubscribeRuntimeInfo" if primary => {
            add_stream(streams);
            item(runtime_info(&st))
        }
        "SubscribeToSettingsChanges" if !primary => {
            // The secondary ends the stream at once.
            ok(vec![])
        }
        "SubscribeToSettingsChanges" => {
            add_stream(streams);
            return;
        }
        "SubscribeToQuietModeStatus" if primary => {
            add_stream(streams);
            item(quiet(&st))
        }
        "ReadSetting" => {
            let req_id = pb::ReadSettingRequest::decode(&req.payload[..]).ok().and_then(|r| r.setting_id);
            let field = match req_id {
                Some(pb::read_setting_request::SettingId::Allegro(f)) => f,
                _ => 0,
            };
            if let Some(s) = st.setting_status.get(&field) {
                status(*s)
            } else {
                let v = match field {
                    f if f == SettingId::Anc.field() => {
                        Some(Setting::Anc(if all_in_case(&st) { None } else { crate::settings::AncMode::from_raw(st.anc) }))
                    }
                    f if f == SettingId::Multipoint.field() => Some(Setting::Multipoint(st.multipoint)),
                    _ => None,
                };
                match v {
                    // Anc(None) cannot be written by write_request: encode anc_state 0 by hand.
                    Some(Setting::Anc(None)) => ok(vec![0x22, 0x02, 0x68, 0x00]),
                    Some(s) => ok(settings::write_request(&s).unwrap()),
                    None => status(Status::Unknown),
                }
            }
        }
        "WriteSetting" => match settings::decode(&req.payload) {
            Ok(Some(s)) => {
                let field = s.id().map(|i| i.field()).unwrap_or(0);
                if let Some(code) = st.setting_status.get(&field) {
                    status(*code)
                } else {
                    match s {
                        Setting::Anc(_) if all_in_case(&st) => status(Status::FailedPrecondition),
                        Setting::Anc(Some(m)) => {
                            st.anc = m.raw();
                            ok(vec![])
                        }
                        Setting::Multipoint(b) => {
                            st.multipoint = b;
                            ok(vec![])
                        }
                        _ => ok(vec![]),
                    }
                }
            }
            _ => status(Status::InvalidArgument),
        },
        "ForceMultipointSwitch" => {
            let r = pb::ForceMultipointSwitchRequest::decode(&req.payload[..]).unwrap_or_default();
            if r.source_endpoint != Some(host.endpoint() as i32) {
                status(Status::InvalidArgument)
            } else {
                st.exclusive = !r.release.unwrap_or(false);
                if st.exclusive {
                    st.quiet_mode = false;
                }
                ok(vec![])
            }
        }
        "SubscribeToOobeActions" => {
            let a = |on: bool, l: i32| pb::OobeAction { action: Some(if on { l } else { l + 1 }) }.encode_to_vec();
            out.push(frame(to, host, &item(a(!st.in_case[0], 10))));
            out.push(frame(to, host, &item(a(!st.in_case[1], 12))));
            status(Status::Unavailable)
        }
        _ => reply(PacketType::ServerError, &req, vec![], Status::Unavailable, req.call_id),
    };
    out.push(frame(to, host, &p));
}
