// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated protobuf types (prost) from `proto/`.

#![allow(clippy::all, missing_docs)]

/// `maestro_pw` services' messages and settings.
pub mod maestro_pw {
    include!(concat!(env!("OUT_DIR"), "/maestro_pw.rs"));
}

/// Pigweed RPC wire packet.
pub mod pw_rpc {
    include!(concat!(env!("OUT_DIR"), "/pw.rpc.internal.rs"));
}
