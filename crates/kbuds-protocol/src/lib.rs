// SPDX-License-Identifier: GPL-3.0-or-later
//! Google Pixel Buds "Maestro" protocol for the Pigweed models (Pixel Buds Pro, Pro 2, 2a).
//!
//! Layers, bottom up:
//!
//! * [`hdlc`]: pw_hdlc UI frames (flag, escaping, address varint, CRC-32);
//! * [`rpc`]: pw_rpc `RpcPacket`, packet types and status codes; [`ids`]: service/method hashes;
//! * [`address`]: endpoints, this host's id (`MAESTRO_A`/`MAESTRO_B`) and the channel map;
//! * [`engine`]: a sans-IO client that matches requests and responses;
//! * [`maestro`], [`settings`], [`models`]: typed requests, decoded replies, settings, feature gating;
//! * [`client`] (feature `tokio`): an async connection that drives the engine over any byte stream.

pub mod address;
pub mod engine;
pub mod hdlc;
pub mod ids;
pub mod maestro;
pub mod models;
pub mod proto;
pub mod rpc;
pub mod settings;

#[cfg(feature = "tokio")]
pub mod client;

#[cfg(feature = "testing")]
pub mod testing;

pub use address::{Bud, Endpoint, HostId};
pub use rpc::{PacketType, Status};

/// SDP service UUID of the Maestro RFCOMM channel on the Pigweed models.
pub const MAESTRO_UUID: &str = "25e97ff7-24ce-4c4c-8951-f764a708f7b5";
