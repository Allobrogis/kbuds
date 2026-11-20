// SPDX-License-Identifier: GPL-3.0-or-later
//! Sans-IO pw_rpc client for one Maestro session.
//!
//! Feed received bytes with [`Engine::feed`], take [`Event`]s with [`Engine::next_event`], and write
//! the frames returned by [`Engine::start_call`] / [`Engine::cancel`] to the socket, one per write.
//!
//! Matching follows the companion app:
//! * calls are keyed by (channel, service, method); a new call on a busy key replaces the old one;
//! * a call sent with a `call_id` owns only packets with that id; a terminal packet with id 0 is
//!   still accepted (firmware that does not echo it). Calls sent without id match by key only;
//! * packets with no pending call are reported as [`Event::Unsolicited`] and never complete a call
//!   (the buds deliver other hosts' responses too);
//! * nothing can be sent before the first frame told us our host id.

use std::collections::{HashMap, VecDeque};

use thiserror::Error;

use crate::address::{Endpoint, HostId};
use crate::hdlc::{self, DecodeError, Decoded, Reassembler};
use crate::ids::Method;
use crate::rpc::{Packet, PacketType, Status};

/// Identifies one call for the lifetime of an [`Engine`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CallHandle(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallKind {
    Unary,
    ServerStream,
}

/// Why a call failed.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum RpcError {
    /// RESPONSE with a non-OK status.
    #[error("buds answered {0}")]
    Status(Status),
    /// SERVER_ERROR (e.g. NOT_FOUND for an unknown service).
    #[error("buds reported server error {0}")]
    ServerError(Status),
    /// A newer call on the same (channel, service, method) replaced this one.
    #[error("replaced by a newer call")]
    Replaced,
    #[error("no answer before the timeout")]
    Timeout,
    #[error("connection closed")]
    ConnectionClosed,
    #[error("host id not known yet (no frame received)")]
    HostIdUnknown,
    #[error("no channel for endpoint {0:?}")]
    NoChannel(Endpoint),
    #[error("undecodable reply: {0}")]
    Decode(String),
}

impl RpcError {
    /// The pw status carried by the error, if the buds sent one.
    pub fn status(&self) -> Option<Status> {
        match self {
            RpcError::Status(s) | RpcError::ServerError(s) => Some(*s),
            _ => None,
        }
    }
}

/// How a server stream ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamEnd {
    /// RESPONSE: the server closed the stream (OK, or e.g. UNAVAILABLE for OobeActions).
    Completed(Status),
    /// SERVER_ERROR.
    ServerError(Status),
    Replaced,
    ConnectionClosed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The first frame arrived; its destination is our host id.
    HostIdLearned(HostId),
    UnaryDone { handle: CallHandle, result: Result<Vec<u8>, RpcError> },
    StreamItem { handle: CallHandle, payload: Vec<u8> },
    StreamEnded { handle: CallHandle, end: StreamEnd },
    /// A server packet that belongs to no pending call: device-initiated pushes (settings on
    /// MAESTRO_A), the hello frame, other hosts' responses. `from` is the sending endpoint.
    Unsolicited { from: Option<Endpoint>, packet: Packet },
    /// Bytes that did not form a valid frame or RpcPacket.
    Invalid(String),
}

#[derive(Debug)]
struct Pending {
    handle: CallHandle,
    kind: CallKind,
    call_id: u32,
    target: Endpoint,
}

type Key = (u32, u32, u32);

/// Sans-IO client state for one RFCOMM session. Create a new one per connection.
#[derive(Debug, Default)]
pub struct Engine {
    reassembler: Reassembler,
    host: Option<HostId>,
    pending: HashMap<Key, Pending>,
    by_handle: HashMap<CallHandle, Key>,
    events: VecDeque<Event>,
    next_handle: u64,
    next_call_id: u32,
}

impl Engine {
    pub fn new() -> Self {
        Self { next_call_id: 1, ..Default::default() }
    }

    pub fn host_id(&self) -> Option<HostId> {
        self.host
    }

    /// Feeds raw received bytes (any split).
    pub fn feed(&mut self, data: &[u8]) {
        for item in self.reassembler.feed(data) {
            match item {
                Decoded::Frame(f) => self.on_frame(f),
                Decoded::Invalid(e, raw) => self.events.push_back(Event::Invalid(format!(
                    "dropped invalid HDLC frame ({}): {}",
                    describe(&e),
                    hex(&raw)
                ))),
            }
        }
    }

    pub fn next_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// Starts a call to `target`. Returns its handle and the frame to send.
    ///
    /// `with_call_id` sends a fresh non-zero `call_id` (used for the settings and quiet-mode
    /// subscriptions, whose echo is confirmed on hardware). Other calls send none, like the app.
    pub fn start_call(
        &mut self,
        target: Endpoint,
        method: Method,
        kind: CallKind,
        payload: Vec<u8>,
        with_call_id: bool,
    ) -> Result<(CallHandle, Vec<u8>), RpcError> {
        let host = self.host.ok_or(RpcError::HostIdUnknown)?;
        let channel = host.channel(target).ok_or(RpcError::NoChannel(target))?;
        let key = (channel, method.service_id, method.method_id);
        if let Some(old) = self.pending.remove(&key) {
            self.by_handle.remove(&old.handle);
            self.events.push_back(match old.kind {
                CallKind::Unary => Event::UnaryDone { handle: old.handle, result: Err(RpcError::Replaced) },
                CallKind::ServerStream => Event::StreamEnded { handle: old.handle, end: StreamEnd::Replaced },
            });
        }
        let call_id = if with_call_id { self.alloc_call_id() } else { 0 };
        self.next_handle += 1;
        let handle = CallHandle(self.next_handle);
        self.pending.insert(key, Pending { handle, kind, call_id, target });
        self.by_handle.insert(handle, key);
        let packet = Packet::request(channel, method.service_id, method.method_id, payload, call_id);
        Ok((handle, self.frame_to(target, &packet)))
    }

    /// Cancels a pending server stream: returns the CLIENT_ERROR CANCELLED frame to send, or `None`
    /// if the call is no longer pending. The server does not answer a cancel.
    pub fn cancel(&mut self, handle: CallHandle) -> Option<Vec<u8>> {
        let key = self.by_handle.remove(&handle)?;
        let p = self.pending.remove(&key)?;
        let packet = Packet::cancel(key.0, key.1, key.2, p.call_id);
        Some(self.frame_to(p.target, &packet))
    }

    /// Forgets a call locally without telling the buds (the app's "abandon", e.g. on timeout).
    pub fn abandon(&mut self, handle: CallHandle) -> bool {
        match self.by_handle.remove(&handle) {
            Some(key) => self.pending.remove(&key).is_some(),
            None => false,
        }
    }

    /// Fails every pending call with `ConnectionClosed`.
    pub fn close(&mut self) {
        let mut pending: Vec<_> = self.pending.drain().map(|(_, p)| p).collect();
        pending.sort_by_key(|p| p.handle);
        self.by_handle.clear();
        for p in pending {
            self.events.push_back(match p.kind {
                CallKind::Unary => Event::UnaryDone { handle: p.handle, result: Err(RpcError::ConnectionClosed) },
                CallKind::ServerStream => {
                    Event::StreamEnded { handle: p.handle, end: StreamEnd::ConnectionClosed }
                }
            });
        }
    }

    /// Number of calls still waiting for an answer.
    pub fn pending_calls(&self) -> usize {
        self.pending.len()
    }

    fn alloc_call_id(&mut self) -> u32 {
        let id = self.next_call_id;
        self.next_call_id = match self.next_call_id.wrapping_add(1) {
            0 | u32::MAX => 1,
            n => n,
        };
        id
    }

    fn frame_to(&self, target: Endpoint, packet: &Packet) -> Vec<u8> {
        let host = self.host.expect("checked by callers");
        hdlc::encode(hdlc::address(host.endpoint(), target), &packet.encode())
    }

    fn on_frame(&mut self, frame: hdlc::Frame) {
        if self.host.is_none() {
            match HostId::from_endpoint(frame.dst()) {
                Some(h) => {
                    self.host = Some(h);
                    self.events.push_back(Event::HostIdLearned(h));
                }
                None => {
                    self.events.push_back(Event::Invalid(format!(
                        "first frame addressed to endpoint {}, not MAESTRO_A/B; still waiting",
                        frame.dst()
                    )));
                    return;
                }
            }
        }
        let packet = match Packet::decode(&frame.payload) {
            Ok(p) => p,
            Err(e) => {
                self.events
                    .push_back(Event::Invalid(format!("undecodable RpcPacket ({e}): {}", hex(&frame.payload))));
                return;
            }
        };
        let from = Endpoint::from_id(frame.src());
        if !packet.kind.from_server() {
            // Client→server types sent by the device: the app ignores them.
            return;
        }
        let key = (packet.channel, packet.service_id, packet.method_id);
        let owns = match self.pending.get(&key) {
            None => false,
            Some(p) if p.call_id == 0 => true,
            Some(p) => {
                packet.call_id == p.call_id
                    || (packet.call_id == 0 && packet.kind != PacketType::ServerStream)
            }
        };
        if !owns {
            self.events.push_back(Event::Unsolicited { from, packet });
            return;
        }
        match packet.kind {
            PacketType::ServerStream => {
                let p = &self.pending[&key];
                if p.kind == CallKind::ServerStream {
                    let handle = p.handle;
                    self.events.push_back(Event::StreamItem { handle, payload: packet.payload });
                } else {
                    self.events.push_back(Event::Unsolicited { from, packet });
                }
            }
            PacketType::Response | PacketType::ServerError => {
                let p = self.pending.remove(&key).expect("owned");
                self.by_handle.remove(&p.handle);
                let server_error = packet.kind == PacketType::ServerError;
                self.events.push_back(match p.kind {
                    CallKind::Unary => Event::UnaryDone {
                        handle: p.handle,
                        result: if server_error {
                            Err(RpcError::ServerError(packet.status))
                        } else if packet.status.is_ok() {
                            Ok(packet.payload)
                        } else {
                            Err(RpcError::Status(packet.status))
                        },
                    },
                    CallKind::ServerStream => Event::StreamEnded {
                        handle: p.handle,
                        end: if server_error {
                            StreamEnd::ServerError(packet.status)
                        } else {
                            StreamEnd::Completed(packet.status)
                        },
                    },
                });
            }
            _ => self.events.push_back(Event::Unsolicited { from, packet }),
        }
    }
}

fn describe(e: &DecodeError) -> &'static str {
    match e {
        DecodeError::TooShort => "too short",
        DecodeError::TooLong => "too long",
        DecodeError::BadCrc => "bad CRC",
        DecodeError::BadAddress => "bad address",
        DecodeError::DanglingEscape => "dangling escape",
    }
}

/// Lower-case hex without separators.
pub fn hex(data: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(data.len() * 2);
    for b in data {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids;

    /// A frame from `src` to `host` carrying `packet`.
    fn from_bud(src: Endpoint, host: HostId, packet: Packet) -> Vec<u8> {
        hdlc::encode(hdlc::address(src, host.endpoint()), &packet.encode())
    }

    fn server(kind: PacketType, channel: u32, m: Method, payload: &[u8], status: Status, call_id: u32) -> Packet {
        Packet {
            kind,
            channel,
            service_id: m.service_id,
            method_id: m.method_id,
            payload: payload.to_vec(),
            status,
            call_id,
        }
    }

    fn hello(host: HostId) -> Vec<u8> {
        let ch = host.channel(Endpoint::LeftBtCore).unwrap();
        from_bud(
            Endpoint::LeftBtCore,
            host,
            server(PacketType::Response, ch, ids::GET_SOFTWARE_INFO, &[0x30, 0x00], Status::Ok, u32::MAX),
        )
    }

    fn events(e: &mut Engine) -> Vec<Event> {
        std::iter::from_fn(|| e.next_event()).collect()
    }

    #[test]
    fn refuses_to_send_before_first_frame() {
        let mut e = Engine::new();
        let r = e.start_call(Endpoint::LeftBtCore, ids::GET_SOFTWARE_INFO, CallKind::Unary, vec![], false);
        assert_eq!(r.unwrap_err(), RpcError::HostIdUnknown);
    }

    #[test]
    fn learns_host_id_and_reports_hello_as_unsolicited() {
        for host in [HostId::A, HostId::B] {
            let mut e = Engine::new();
            e.feed(&hello(host));
            let ev = events(&mut e);
            assert_eq!(ev[0], Event::HostIdLearned(host));
            assert!(matches!(&ev[1], Event::Unsolicited { from: Some(Endpoint::LeftBtCore), packet } if packet.call_id == u32::MAX));
            assert_eq!(e.host_id(), Some(host));
        }
    }

    #[test]
    fn unary_roundtrip_and_status() {
        let mut e = Engine::new();
        e.feed(&hello(HostId::B));
        events(&mut e);
        let (h, frame) = e
            .start_call(Endpoint::RightBtCore, ids::GET_SOFTWARE_INFO, CallKind::Unary, vec![], false)
            .unwrap();
        // MAESTRO_B → RIGHT on channel 26.
        let decoded = Reassembler::new().feed(&frame);
        let [Decoded::Frame(f)] = &decoded[..] else { panic!() };
        assert_eq!(f.src(), Endpoint::MaestroB as u8);
        assert_eq!(f.dst(), Endpoint::RightBtCore as u8);
        assert_eq!(Packet::decode(&f.payload).unwrap().channel, 26);

        e.feed(&from_bud(
            Endpoint::RightBtCore,
            HostId::B,
            server(PacketType::Response, 26, ids::GET_SOFTWARE_INFO, &[0x30, 0x01], Status::Ok, 0),
        ));
        assert_eq!(events(&mut e), vec![Event::UnaryDone { handle: h, result: Ok(vec![0x30, 0x01]) }]);

        // Non-OK RESPONSE and SERVER_ERROR.
        let (h, _) = e.start_call(Endpoint::LeftBtCore, ids::WRITE_SETTING, CallKind::Unary, vec![], false).unwrap();
        e.feed(&from_bud(
            Endpoint::LeftBtCore,
            HostId::B,
            server(PacketType::Response, 24, ids::WRITE_SETTING, &[], Status::FailedPrecondition, 0),
        ));
        assert_eq!(
            events(&mut e),
            vec![Event::UnaryDone { handle: h, result: Err(RpcError::Status(Status::FailedPrecondition)) }]
        );
        let (h, _) = e.start_call(Endpoint::LeftBtCore, ids::READ_SETTING, CallKind::Unary, vec![], false).unwrap();
        e.feed(&from_bud(
            Endpoint::LeftBtCore,
            HostId::B,
            server(PacketType::ServerError, 24, ids::READ_SETTING, &[], Status::NotFound, 0),
        ));
        assert_eq!(
            events(&mut e),
            vec![Event::UnaryDone { handle: h, result: Err(RpcError::ServerError(Status::NotFound)) }]
        );
        assert_eq!(e.pending_calls(), 0);
    }

    #[test]
    fn response_without_pending_call_is_unsolicited() {
        let mut e = Engine::new();
        e.feed(&hello(HostId::A));
        events(&mut e);
        // Seen on hardware: unrequested Dosimeter responses after a reconnect.
        let m = Method::new("maestro_pw.Dosimeter", "FetchDailySummaries");
        e.feed(&from_bud(
            Endpoint::RightBtCore,
            HostId::A,
            server(PacketType::Response, 21, m, &[0x08, 0x01], Status::Ok, 0),
        ));
        assert!(matches!(&events(&mut e)[..], [Event::Unsolicited { .. }]));
    }

    #[test]
    fn stream_with_call_id() {
        let mut e = Engine::new();
        e.feed(&hello(HostId::A));
        events(&mut e);
        let m = ids::SUBSCRIBE_SETTINGS_CHANGES;
        let (h, frame) = e.start_call(Endpoint::LeftBtCore, m, CallKind::ServerStream, vec![], true).unwrap();
        let decoded = Reassembler::new().feed(&frame);
        let [Decoded::Frame(f)] = &decoded[..] else { panic!() };
        let cid = Packet::decode(&f.payload).unwrap().call_id;
        assert_ne!(cid, 0);

        // Echoed id → item; foreign id → unsolicited; id 0 item → unsolicited (stream items must match).
        for (id, ours) in [(cid, true), (cid + 7, false), (0, false)] {
            e.feed(&from_bud(
                Endpoint::LeftBtCore,
                HostId::A,
                server(PacketType::ServerStream, 19, m, &[0x22, 0x02, 0x58, 0x01], Status::Ok, id),
            ));
            let ev = events(&mut e);
            if ours {
                assert_eq!(ev, vec![Event::StreamItem { handle: h, payload: vec![0x22, 0x02, 0x58, 0x01] }]);
            } else {
                assert!(matches!(&ev[..], [Event::Unsolicited { .. }]), "{id}: {ev:?}");
            }
        }
        // A bud ends the stream at once with RESPONSE OK and call_id 0: accepted as terminal.
        e.feed(&from_bud(
            Endpoint::LeftBtCore,
            HostId::A,
            server(PacketType::Response, 19, m, &[], Status::Ok, 0),
        ));
        assert_eq!(events(&mut e), vec![Event::StreamEnded { handle: h, end: StreamEnd::Completed(Status::Ok) }]);
    }

    #[test]
    fn replace_cancel_abandon_close() {
        let mut e = Engine::new();
        e.feed(&hello(HostId::A));
        events(&mut e);
        let m = ids::SUBSCRIBE_RUNTIME_INFO;
        let (h1, _) = e.start_call(Endpoint::LeftBtCore, m, CallKind::ServerStream, vec![], false).unwrap();
        let (h2, _) = e.start_call(Endpoint::LeftBtCore, m, CallKind::ServerStream, vec![], false).unwrap();
        assert_eq!(events(&mut e), vec![Event::StreamEnded { handle: h1, end: StreamEnd::Replaced }]);
        assert!(e.cancel(h1).is_none());
        let cancel = e.cancel(h2).unwrap();
        let decoded = Reassembler::new().feed(&cancel);
        let [Decoded::Frame(f)] = &decoded[..] else { panic!() };
        let p = Packet::decode(&f.payload).unwrap();
        assert_eq!((p.kind, p.status, p.channel), (PacketType::ClientError, Status::Cancelled, 19));
        assert!(e.cancel(h2).is_none());

        let (h3, _) = e.start_call(Endpoint::LeftBtCore, ids::GET_HARDWARE_INFO, CallKind::Unary, vec![], false).unwrap();
        assert!(e.abandon(h3));
        let (h4, _) = e.start_call(Endpoint::RightBtCore, ids::GET_HARDWARE_INFO, CallKind::Unary, vec![], false).unwrap();
        let (h5, _) = e.start_call(Endpoint::RightBtCore, m, CallKind::ServerStream, vec![], false).unwrap();
        e.close();
        assert_eq!(
            events(&mut e),
            vec![
                Event::UnaryDone { handle: h4, result: Err(RpcError::ConnectionClosed) },
                Event::StreamEnded { handle: h5, end: StreamEnd::ConnectionClosed },
            ]
        );
    }
}
