// SPDX-License-Identifier: GPL-3.0-or-later
//! Async Maestro connection over any byte stream (an RFCOMM socket in the daemon, an in-memory
//! duplex in tests).
//!
//! [`Connection::open`] spawns a driver task that owns the stream and an [`Engine`], waits for the
//! first frame (host id) and returns a cheap, clonable handle. The connection lives until every
//! handle is dropped or the stream closes; [`Notification::Closed`] reports the latter.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use crate::address::{Bud, Endpoint, HostId};
use crate::engine::{CallHandle, CallKind, Engine, Event, RpcError, StreamEnd, hex};
use crate::ids::{self, Method};
use crate::maestro::{self, HardwareInfo, SoftwareInfo};
use crate::rpc::Packet;
use crate::settings::{self, Setting, SettingId};

/// Tunables from the app.
pub mod timing {
    use std::time::Duration;
    /// Wait for the buds' first frame after connecting.
    pub const FIRST_FRAME: Duration = Duration::from_secs(5);
    /// Default unary call timeout.
    pub const RPC: Duration = Duration::from_secs(3);
    /// ReadSetting: 1 s per attempt, 3 attempts.
    pub const READ_SETTING: Duration = Duration::from_secs(1);
    pub const READ_SETTING_ATTEMPTS: u32 = 3;
    /// WriteSetting: 5 s per attempt, 5 attempts, 2 s apart.
    pub const WRITE_SETTING: Duration = Duration::from_secs(5);
    pub const WRITE_SETTING_ATTEMPTS: u32 = 5;
    pub const WRITE_SETTING_RETRY_DELAY: Duration = Duration::from_secs(2);
    /// Delay before reconnecting after the buds closed the channel.
    pub const RECONNECT_DELAY: Duration = Duration::from_secs(3);
    /// RFCOMM connect timeout.
    pub const RFCOMM_CONNECT: Duration = Duration::from_secs(12);
}

/// Direction of a wire chunk, for [`WireTap`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Host → buds (one frame per write).
    Tx,
    /// Buds → host (one raw read; may hold partial or several frames).
    Rx,
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Direction::Tx => "TX",
            Direction::Rx => "RX",
        })
    }
}

/// Observer of raw wire traffic (for frame logs).
pub type WireTap = Arc<dyn Fn(Direction, &[u8]) + Send + Sync>;

/// Something the buds sent that is not the answer to one of our calls, or the end of the link.
#[derive(Debug)]
pub enum Notification {
    Unsolicited { from: Option<Endpoint>, packet: Packet },
    Invalid(String),
    /// The stream closed (EOF, ECONNRESET, …). `None` = clean EOF.
    Closed(Option<io::Error>),
}

/// Item of a server stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamMsg {
    Item(Vec<u8>),
    End(StreamEnd),
}

enum Command {
    Unary {
        target: Endpoint,
        method: Method,
        payload: Vec<u8>,
        timeout: Duration,
        reply: oneshot::Sender<Result<Vec<u8>, RpcError>>,
    },
    Stream {
        target: Endpoint,
        method: Method,
        payload: Vec<u8>,
        with_call_id: bool,
        items: mpsc::UnboundedSender<StreamMsg>,
        reply: oneshot::Sender<Result<CallHandle, RpcError>>,
    },
    Cancel(CallHandle),
}

/// Handle to an open Maestro session. Clone freely.
#[derive(Clone)]
pub struct Connection {
    cmd: mpsc::UnboundedSender<Command>,
    host: HostId,
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection").field("host", &self.host).finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("no frame from the buds within {0:?}")]
    NoFirstFrame(Duration),
    #[error("connection closed before the first frame: {0}")]
    Closed(String),
}

impl Connection {
    /// Starts the driver on `io` and waits up to `first_frame` for the buds' first frame.
    ///
    /// Returns the connection and the receiver of [`Notification`]s (unsolicited packets, close).
    pub async fn open<T>(
        io: T,
        first_frame: Duration,
        tap: Option<WireTap>,
    ) -> Result<(Connection, mpsc::UnboundedReceiver<Notification>), OpenError>
    where
        T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (note_tx, mut note_rx) = mpsc::unbounded_channel();
        let (host_tx, host_rx) = oneshot::channel();
        tokio::spawn(drive(io, cmd_rx, note_tx, host_tx, tap));
        match tokio::time::timeout(first_frame, host_rx).await {
            Ok(Ok(host)) => Ok((Connection { cmd: cmd_tx, host }, note_rx)),
            Ok(Err(_)) => {
                let why = match note_rx.try_recv() {
                    Ok(Notification::Closed(Some(e))) => e.to_string(),
                    _ => "EOF".to_string(),
                };
                Err(OpenError::Closed(why))
            }
            Err(_) => Err(OpenError::NoFirstFrame(first_frame)),
        }
    }

    /// This host's id for the session.
    pub fn host_id(&self) -> HostId {
        self.host
    }

    /// True once the driver stopped (stream closed).
    pub fn is_closed(&self) -> bool {
        self.cmd.is_closed()
    }

    /// One unary call.
    pub async fn unary(
        &self,
        target: Endpoint,
        method: Method,
        payload: Vec<u8>,
        timeout: Duration,
    ) -> Result<Vec<u8>, RpcError> {
        let (reply, rx) = oneshot::channel();
        self.cmd
            .send(Command::Unary { target, method, payload, timeout, reply })
            .map_err(|_| RpcError::ConnectionClosed)?;
        rx.await.unwrap_or(Err(RpcError::ConnectionClosed))
    }

    /// Opens a server stream.
    pub async fn subscribe(
        &self,
        target: Endpoint,
        method: Method,
        payload: Vec<u8>,
        with_call_id: bool,
    ) -> Result<Subscription, RpcError> {
        let (items, rx_items) = mpsc::unbounded_channel();
        let (reply, rx) = oneshot::channel();
        self.cmd
            .send(Command::Stream { target, method, payload, with_call_id, items, reply })
            .map_err(|_| RpcError::ConnectionClosed)?;
        let handle = rx.await.unwrap_or(Err(RpcError::ConnectionClosed))?;
        Ok(Subscription { handle, rx: rx_items, cmd: self.cmd.clone(), ended: false })
    }

    // --- typed helpers (single attempt; callers own the retry policy)

    pub async fn software_info(&self, bud: Bud) -> Result<SoftwareInfo, RpcError> {
        let r = self.unary(bud.endpoint(), ids::GET_SOFTWARE_INFO, maestro::empty(), timing::RPC).await?;
        SoftwareInfo::decode(&r).map_err(RpcError::Decode)
    }

    pub async fn hardware_info(&self, bud: Bud) -> Result<HardwareInfo, RpcError> {
        let r = self.unary(bud.endpoint(), ids::GET_HARDWARE_INFO, maestro::empty(), timing::RPC).await?;
        HardwareInfo::decode(&r).map_err(RpcError::Decode)
    }

    pub async fn set_wallclock(&self, bud: Bud, epoch_ms: u64) -> Result<(), RpcError> {
        self.unary(bud.endpoint(), ids::SET_WALLCLOCK, maestro::wallclock_request(epoch_ms), timing::RPC)
            .await
            .map(|_| ())
    }

    /// `ReadSetting`. `Ok(None)` = the firmware does not know the field. A non-OK status (e.g.
    /// UNKNOWN(2), whose payload is only a placeholder) is an error.
    pub async fn read_setting(&self, bud: Bud, id: SettingId, timeout: Duration) -> Result<Option<Setting>, RpcError> {
        let r = self.unary(bud.endpoint(), ids::READ_SETTING, settings::read_request(id), timeout).await?;
        settings::decode(&r).map_err(|e| RpcError::Decode(e.to_string()))
    }

    /// `WriteSetting`. The buds send no push for our own write: confirm with [`Self::read_setting`].
    pub async fn write_setting(&self, bud: Bud, setting: &Setting, timeout: Duration) -> Result<(), RpcError> {
        let payload = settings::write_request(setting).map_err(|e| RpcError::Decode(e.to_string()))?;
        self.unary(bud.endpoint(), ids::WRITE_SETTING, payload, timeout).await.map(|_| ())
    }

    /// `Multipoint/ForceMultipointSwitch` with this session's own host id.
    pub async fn force_multipoint_switch(&self, bud: Bud, release: bool) -> Result<(), RpcError> {
        let payload = maestro::force_multipoint_switch_request(self.host, release);
        self.unary(bud.endpoint(), ids::FORCE_MULTIPOINT_SWITCH, payload, timing::RPC).await.map(|_| ())
    }
}

/// An open server stream. Dropping it cancels the stream (CLIENT_ERROR CANCELLED) if it is still open.
pub struct Subscription {
    handle: CallHandle,
    rx: mpsc::UnboundedReceiver<StreamMsg>,
    cmd: mpsc::UnboundedSender<Command>,
    ended: bool,
}

impl Subscription {
    /// Next item; `StreamMsg::End` is the last message. `None` after the end.
    pub async fn next(&mut self) -> Option<StreamMsg> {
        if self.ended {
            return None;
        }
        let msg = self.rx.recv().await.unwrap_or(StreamMsg::End(StreamEnd::ConnectionClosed));
        if matches!(msg, StreamMsg::End(_)) {
            self.ended = true;
        }
        Some(msg)
    }

    pub fn handle(&self) -> CallHandle {
        self.handle
    }

    /// Cancels the stream now.
    pub fn cancel(mut self) {
        self.send_cancel();
    }

    fn send_cancel(&mut self) {
        if !self.ended {
            self.ended = true;
            let _ = self.cmd.send(Command::Cancel(self.handle));
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.send_cancel();
    }
}

enum Sink {
    Unary(oneshot::Sender<Result<Vec<u8>, RpcError>>),
    Stream(mpsc::UnboundedSender<StreamMsg>),
}

enum Wake {
    Read(io::Result<usize>),
    Cmd(Option<Command>),
    Timer,
}

async fn drive<T>(
    mut io: T,
    mut cmd_rx: mpsc::UnboundedReceiver<Command>,
    notes: mpsc::UnboundedSender<Notification>,
    host_tx: oneshot::Sender<HostId>,
    tap: Option<WireTap>,
) where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut engine = Engine::new();
    let mut sinks: HashMap<CallHandle, Sink> = HashMap::new();
    let mut deadlines: HashMap<CallHandle, Instant> = HashMap::new();
    let mut host_tx = Some(host_tx);
    let mut buf = vec![0u8; 4096];
    let tap_fn = |d: Direction, data: &[u8]| {
        tracing::trace!(target: "kbuds_protocol::wire", "{d} {}", hex(data));
        if let Some(t) = &tap {
            t(d, data);
        }
    };

    let close_reason: Option<io::Error> = loop {
        let next_deadline = deadlines.values().min().copied();
        let wake = tokio::select! {
            r = io.read(&mut buf) => Wake::Read(r),
            // No handle exists before the host id is known, so commands can only be the
            // "all handles dropped" None (e.g. `open` timed out).
            c = cmd_rx.recv() => Wake::Cmd(c),
            _ = tokio::time::sleep_until(next_deadline.unwrap_or_else(Instant::now)), if next_deadline.is_some() => Wake::Timer,
        };
        let mut outgoing: Vec<Vec<u8>> = Vec::new();
        match wake {
            Wake::Read(Ok(0)) => break None,
            Wake::Read(Ok(n)) => {
                tap_fn(Direction::Rx, &buf[..n]);
                engine.feed(&buf[..n]);
            }
            Wake::Read(Err(e)) => break Some(e),
            Wake::Cmd(None) => {
                // Every handle dropped: stop. Pending calls die with the engine.
                return;
            }
            Wake::Cmd(Some(cmd)) => match cmd {
                Command::Unary { target, method, payload, timeout, reply } => {
                    match engine.start_call(target, method, CallKind::Unary, payload, false) {
                        Ok((h, frame)) => {
                            sinks.insert(h, Sink::Unary(reply));
                            deadlines.insert(h, Instant::now() + timeout);
                            outgoing.push(frame);
                        }
                        Err(e) => {
                            let _ = reply.send(Err(e));
                        }
                    }
                }
                Command::Stream { target, method, payload, with_call_id, items, reply } => {
                    match engine.start_call(target, method, CallKind::ServerStream, payload, with_call_id) {
                        Ok((h, frame)) => {
                            sinks.insert(h, Sink::Stream(items));
                            outgoing.push(frame);
                            let _ = reply.send(Ok(h));
                        }
                        Err(e) => {
                            let _ = reply.send(Err(e));
                        }
                    }
                }
                Command::Cancel(h) => {
                    sinks.remove(&h);
                    if let Some(frame) = engine.cancel(h) {
                        outgoing.push(frame);
                    }
                }
            },
            Wake::Timer => {
                let now = Instant::now();
                let expired: Vec<_> = deadlines.iter().filter(|(_, d)| **d <= now).map(|(h, _)| *h).collect();
                for h in expired {
                    deadlines.remove(&h);
                    engine.abandon(h);
                    if let Some(Sink::Unary(reply)) = sinks.remove(&h) {
                        let _ = reply.send(Err(RpcError::Timeout));
                    }
                }
            }
        }
        dispatch(&mut engine, &mut sinks, &mut deadlines, &notes, &mut host_tx);
        let mut write_err = None;
        for frame in outgoing {
            tap_fn(Direction::Tx, &frame);
            // One frame per write, as the app does.
            if let Err(e) = io.write_all(&frame).await {
                write_err = Some(e);
                break;
            }
        }
        if let Some(e) = write_err {
            break Some(e);
        }
    };

    engine.close();
    dispatch(&mut engine, &mut sinks, &mut deadlines, &notes, &mut host_tx);
    let _ = notes.send(Notification::Closed(close_reason));
}

fn dispatch(
    engine: &mut Engine,
    sinks: &mut HashMap<CallHandle, Sink>,
    deadlines: &mut HashMap<CallHandle, Instant>,
    notes: &mpsc::UnboundedSender<Notification>,
    host_tx: &mut Option<oneshot::Sender<HostId>>,
) {
    while let Some(ev) = engine.next_event() {
        match ev {
            Event::HostIdLearned(h) => {
                if let Some(tx) = host_tx.take() {
                    let _ = tx.send(h);
                }
            }
            Event::UnaryDone { handle, result } => {
                deadlines.remove(&handle);
                if let Some(Sink::Unary(reply)) = sinks.remove(&handle) {
                    let _ = reply.send(result);
                }
            }
            Event::StreamItem { handle, payload } => {
                if let Some(Sink::Stream(tx)) = sinks.get(&handle) {
                    let _ = tx.send(StreamMsg::Item(payload));
                }
            }
            Event::StreamEnded { handle, end } => {
                if let Some(Sink::Stream(tx)) = sinks.remove(&handle) {
                    let _ = tx.send(StreamMsg::End(end));
                }
            }
            Event::Unsolicited { from, packet } => {
                let _ = notes.send(Notification::Unsolicited { from, packet });
            }
            Event::Invalid(why) => {
                let _ = notes.send(Notification::Invalid(why));
            }
        }
    }
}
