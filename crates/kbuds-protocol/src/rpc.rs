// SPDX-License-Identifier: GPL-3.0-or-later
//! pw_rpc `RpcPacket` (`proto/pigweed/rpc_packet.proto`).

use std::fmt;

use prost::Message;

use crate::proto::pw_rpc::RpcPacket;

/// `RpcPacket.type`. Client → server types are even, server → client odd.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PacketType {
    Request,
    Response,
    ClientStream,
    ClientError,
    ServerError,
    ServerStream,
    ClientRequestCompletion,
    Other(i32),
}

impl PacketType {
    pub fn from_raw(v: i32) -> Self {
        match v {
            0 => Self::Request,
            1 => Self::Response,
            2 => Self::ClientStream,
            4 => Self::ClientError,
            5 => Self::ServerError,
            7 => Self::ServerStream,
            8 => Self::ClientRequestCompletion,
            o => Self::Other(o),
        }
    }

    pub fn raw(self) -> i32 {
        match self {
            Self::Request => 0,
            Self::Response => 1,
            Self::ClientStream => 2,
            Self::ClientError => 4,
            Self::ServerError => 5,
            Self::ServerStream => 7,
            Self::ClientRequestCompletion => 8,
            Self::Other(o) => o,
        }
    }

    /// Server → client packets are the odd ones; the app ignores even types from the device.
    pub fn from_server(self) -> bool {
        self.raw() % 2 != 0
    }
}

/// pw::Status codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    Ok,
    Cancelled,
    Unknown,
    InvalidArgument,
    DeadlineExceeded,
    NotFound,
    AlreadyExists,
    PermissionDenied,
    ResourceExhausted,
    FailedPrecondition,
    Aborted,
    OutOfRange,
    Unimplemented,
    Internal,
    Unavailable,
    DataLoss,
    Unauthenticated,
    Other(u32),
}

impl Status {
    const NAMES: [Status; 17] = [
        Status::Ok,
        Status::Cancelled,
        Status::Unknown,
        Status::InvalidArgument,
        Status::DeadlineExceeded,
        Status::NotFound,
        Status::AlreadyExists,
        Status::PermissionDenied,
        Status::ResourceExhausted,
        Status::FailedPrecondition,
        Status::Aborted,
        Status::OutOfRange,
        Status::Unimplemented,
        Status::Internal,
        Status::Unavailable,
        Status::DataLoss,
        Status::Unauthenticated,
    ];

    pub fn from_raw(v: u32) -> Self {
        Self::NAMES.get(v as usize).copied().unwrap_or(Status::Other(v))
    }

    pub fn raw(self) -> u32 {
        match self {
            Status::Other(v) => v,
            s => Self::NAMES.iter().position(|n| *n == s).unwrap() as u32,
        }
    }

    pub fn is_ok(self) -> bool {
        self == Status::Ok
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Status::Other(v) => write!(f, "STATUS_{v}"),
            s => {
                let name = format!("{s:?}");
                // CamelCase → SCREAMING_SNAKE, as the spec writes them.
                let mut out = String::new();
                for (i, c) in name.chars().enumerate() {
                    if c.is_uppercase() && i > 0 {
                        out.push('_');
                    }
                    out.push(c.to_ascii_uppercase());
                }
                write!(f, "{out}({})", s.raw())
            }
        }
    }
}

/// A decoded `RpcPacket`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub kind: PacketType,
    pub channel: u32,
    pub service_id: u32,
    pub method_id: u32,
    pub payload: Vec<u8>,
    pub status: Status,
    pub call_id: u32,
}

impl Packet {
    pub fn decode(buf: &[u8]) -> Result<Self, prost::DecodeError> {
        let p = RpcPacket::decode(buf)?;
        Ok(Self {
            kind: PacketType::from_raw(p.r#type),
            channel: p.channel_id,
            service_id: p.service_id,
            method_id: p.method_id,
            payload: p.payload,
            status: Status::from_raw(p.status),
            call_id: p.call_id,
        })
    }

    /// proto3 encoding: zero fields are omitted, as protobuf-lite does.
    pub fn encode(&self) -> Vec<u8> {
        RpcPacket {
            r#type: self.kind.raw(),
            channel_id: self.channel,
            service_id: self.service_id,
            method_id: self.method_id,
            payload: self.payload.clone(),
            status: self.status.raw(),
            call_id: self.call_id,
        }
        .encode_to_vec()
    }

    pub fn request(channel: u32, service_id: u32, method_id: u32, payload: Vec<u8>, call_id: u32) -> Self {
        Self {
            kind: PacketType::Request,
            channel,
            service_id,
            method_id,
            payload,
            status: Status::Ok,
            call_id,
        }
    }

    /// Stream cancel: CLIENT_ERROR with CANCELLED and no payload. The server does not answer.
    pub fn cancel(channel: u32, service_id: u32, method_id: u32, call_id: u32) -> Self {
        Self {
            kind: PacketType::ClientError,
            channel,
            service_id,
            method_id,
            payload: Vec::new(),
            status: Status::Cancelled,
            call_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids;

    #[test]
    fn request_encoding_matches_spec() {
        let p = Packet::request(
            19,
            ids::WRITE_SETTING.service_id,
            ids::WRITE_SETTING.method_id,
            vec![0x22, 0x02, 0x68, 0x02],
            0,
        );
        assert_eq!(
            p.encode(),
            hex::decode("10131dea71de7e251d9a8c9e2a0422026802").unwrap()
        );
        assert_eq!(Packet::decode(&p.encode()).unwrap(), p);
    }

    #[test]
    fn cancel_encoding() {
        // 08 04 10 15 1d … 25 … 30 01
        let m = ids::SUBSCRIBE_RUNTIME_INFO;
        let enc = Packet::cancel(21, m.service_id, m.method_id, 0).encode();
        assert_eq!(&enc[..4], &[0x08, 0x04, 0x10, 0x15]);
        assert_eq!(&enc[enc.len() - 2..], &[0x30, 0x01]);
    }

    #[test]
    fn payload_first_order_decodes() {
        // The buds put payload before the other fields.
        let mut buf = vec![0x2a, 0x02, 0x08, 0x01];
        buf.extend_from_slice(&[0x08, 0x01, 0x10, 0x13]);
        let p = Packet::decode(&buf).unwrap();
        assert_eq!(p.kind, PacketType::Response);
        assert_eq!(p.channel, 19);
        assert_eq!(p.payload, vec![0x08, 0x01]);
    }

    #[test]
    fn status_names() {
        assert_eq!(Status::FailedPrecondition.to_string(), "FAILED_PRECONDITION(9)");
        assert_eq!(Status::from_raw(14), Status::Unavailable);
        assert_eq!(Status::from_raw(99).raw(), 99);
    }
}
