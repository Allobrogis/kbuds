// SPDX-License-Identifier: GPL-3.0-or-later
//! Endpoints, host ids and RPC channels.

use std::fmt;

/// HDLC endpoint ids (4 bits in the address).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Endpoint {
    Host = 1,
    Case = 2,
    LeftBtCore = 3,
    RightBtCore = 4,
    LeftSensorHub = 5,
    RightSensorHub = 6,
    LeftSpiBridge = 7,
    RightSpiBridge = 8,
    DebugApp = 9,
    MaestroA = 10,
    LeftTahiti = 11,
    RightTahiti = 12,
    MaestroB = 13,
}

impl Endpoint {
    pub fn from_id(id: u8) -> Option<Self> {
        use Endpoint::*;
        Some(match id {
            1 => Host,
            2 => Case,
            3 => LeftBtCore,
            4 => RightBtCore,
            5 => LeftSensorHub,
            6 => RightSensorHub,
            7 => LeftSpiBridge,
            8 => RightSpiBridge,
            9 => DebugApp,
            10 => MaestroA,
            11 => LeftTahiti,
            12 => RightTahiti,
            13 => MaestroB,
            _ => return None,
        })
    }
}

/// This host's id in the current Maestro session. Learned from the first frame, never cached across
/// sessions: it changes between connections and on a primary handover.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HostId {
    A,
    B,
}

impl HostId {
    /// Host id from the destination of a received frame.
    pub fn from_endpoint(id: u8) -> Option<Self> {
        match id {
            10 => Some(HostId::A),
            13 => Some(HostId::B),
            _ => None,
        }
    }

    pub fn endpoint(self) -> Endpoint {
        match self {
            HostId::A => Endpoint::MaestroA,
            HostId::B => Endpoint::MaestroB,
        }
    }

    /// RPC channel id used to reach `target` from this host. `None` for endpoints without a channel.
    pub fn channel(self, target: Endpoint) -> Option<u32> {
        let base = match self {
            HostId::A => 18,
            HostId::B => 23,
        };
        let offset = match target {
            Endpoint::Case => 0,
            Endpoint::LeftBtCore => 1,
            Endpoint::LeftSensorHub => 2,
            Endpoint::RightBtCore => 3,
            Endpoint::RightSensorHub => 4,
            _ => return None,
        };
        Some(base + offset)
    }
}

impl fmt::Display for HostId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            HostId::A => "A",
            HostId::B => "B",
        })
    }
}

/// A bud (or the case) as an RPC target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bud {
    Left,
    Right,
}

impl Bud {
    pub fn endpoint(self) -> Endpoint {
        match self {
            Bud::Left => Endpoint::LeftBtCore,
            Bud::Right => Endpoint::RightBtCore,
        }
    }

    pub fn other(self) -> Bud {
        match self {
            Bud::Left => Bud::Right,
            Bud::Right => Bud::Left,
        }
    }

    pub fn from_endpoint(e: Endpoint) -> Option<Bud> {
        match e {
            Endpoint::LeftBtCore => Some(Bud::Left),
            Endpoint::RightBtCore => Some(Bud::Right),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Bud::Left => "left",
            Bud::Right => "right",
        }
    }
}

impl fmt::Display for Bud {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels() {
        use Endpoint::*;
        let a: Vec<_> = [Case, LeftBtCore, LeftSensorHub, RightBtCore, RightSensorHub]
            .iter()
            .map(|e| HostId::A.channel(*e).unwrap())
            .collect();
        assert_eq!(a, [18, 19, 20, 21, 22]);
        let b: Vec<_> = [Case, LeftBtCore, LeftSensorHub, RightBtCore, RightSensorHub]
            .iter()
            .map(|e| HostId::B.channel(*e).unwrap())
            .collect();
        assert_eq!(b, [23, 24, 25, 26, 27]);
        assert_eq!(HostId::A.channel(DebugApp), None);
    }
}
