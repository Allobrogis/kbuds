// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed `maestro_pw` requests and replies.

use prost::Message;

use crate::address::HostId;
use crate::proto::maestro_pw as pb;

fn decode<M: Message + Default>(payload: &[u8]) -> Result<M, String> {
    M::decode(payload).map_err(|e| e.to_string())
}

/// Firmware versions of each component (strings as sent, e.g. "575").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Versions {
    pub case: Option<String>,
    pub left: Option<String>,
    pub right: Option<String>,
}

/// `GetSoftwareInfo` reply.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SoftwareInfo {
    pub versions: Versions,
    /// The replying bud is the secondary. Absent counts as primary.
    pub is_secondary: bool,
    pub wallclock_ms: Option<u64>,
}

impl SoftwareInfo {
    pub fn decode(payload: &[u8]) -> Result<Self, String> {
        let m: pb::SoftwareInfo = decode(payload)?;
        let v = match m.versions {
            Some(pb::software_info::Versions::VersionsA(v) | pb::software_info::Versions::VersionsB(v)) => v,
            None => pb::FirmwareVersions::default(),
        };
        let ver = |c: Option<pb::ComponentVersion>| c.and_then(|c| c.version).filter(|s| !s.is_empty());
        Ok(Self {
            versions: Versions { case: ver(v.case), left: ver(v.left), right: ver(v.right) },
            is_secondary: m.is_secondary.unwrap_or(false),
            wallclock_ms: m.wallclock_ms,
        })
    }
}

/// `GetHardwareInfo` reply.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HardwareInfo {
    pub device_type: i32,
    pub sku: u32,
    pub serial_case: Option<String>,
    pub serial_left: Option<String>,
    pub serial_right: Option<String>,
}

impl HardwareInfo {
    pub fn decode(payload: &[u8]) -> Result<Self, String> {
        let m: pb::HardwareInfo = decode(payload)?;
        let s = match m.serials {
            Some(pb::hardware_info::Serials::SerialsA(s) | pb::hardware_info::Serials::SerialsB(s)) => s,
            None => pb::SerialNumbers::default(),
        };
        Ok(Self {
            device_type: m.device_type.unwrap_or(0),
            sku: m.sku.unwrap_or(0),
            serial_case: s.case,
            serial_left: s.left,
            serial_right: s.right,
        })
    }
}

/// Battery level of one component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Battery {
    pub percent: Option<u8>,
    pub charging: bool,
}

/// Battery part of a `RuntimeInfo` push. `None` = not reported (bud not connected; the case is
/// only reported while a bud is in the open case).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BatteryState {
    pub left: Option<Battery>,
    pub right: Option<Battery>,
    pub case: Option<Battery>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    InCase,
    Out,
}

impl Placement {
    pub fn as_str(self) -> &'static str {
        match self {
            Placement::InCase => "in_case",
            Placement::Out => "out",
        }
    }
}

/// Placement part of a `RuntimeInfo` push. `None` = that bud is not connected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlacementState {
    pub left: Option<Placement>,
    pub right: Option<Placement>,
    /// `BudPlacementB.low_latency_mode`: seems to mean "this host streams A2DP" (inferred).
    pub host_streaming: Option<bool>,
}

/// One `RuntimeInfo` push. Each oneof may be absent; the Pro sends both every time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RuntimeInfo {
    pub battery: Option<BatteryState>,
    pub placement: Option<PlacementState>,
}

impl RuntimeInfo {
    pub fn decode(payload: &[u8]) -> Result<Self, String> {
        let m: pb::RuntimeInfo = decode(payload)?;
        let battery = m.battery.map(|b| {
            let (pb::runtime_info::Battery::BatteryA(b) | pb::runtime_info::Battery::BatteryB(b)) = b;
            let level = |l: Option<pb::BatteryLevel>| {
                l.map(|l| Battery {
                    percent: l.level_percent.map(|p| p.min(100) as u8),
                    charging: matches!(
                        l.charging.and_then(|c| pb::ChargingState::try_from(c).ok()),
                        Some(pb::ChargingState::Wired | pb::ChargingState::GenericQi)
                    ),
                })
            };
            BatteryState { left: level(b.left), right: level(b.right), case: level(b.case) }
        });
        let place = |in_case: Option<bool>| in_case.map(|c| if c { Placement::InCase } else { Placement::Out });
        let placement = m.placement.map(|p| match p {
            pb::runtime_info::Placement::PlacementA(p) => PlacementState {
                left: place(p.left_in_case),
                right: place(p.right_in_case),
                host_streaming: None,
            },
            pb::runtime_info::Placement::PlacementB(p) => PlacementState {
                left: place(p.left_in_case),
                right: place(p.right_in_case),
                host_streaming: p.low_latency_mode,
            },
        });
        Ok(Self { battery, placement })
    }
}

/// `QuietModeStatus.quiet_mode`: true = another host holds the audio.
pub fn decode_quiet_mode(payload: &[u8]) -> Result<bool, String> {
    let m: pb::QuietModeStatus = decode(payload)?;
    Ok(m.quiet_mode.unwrap_or(false))
}

/// On-head state from an `OobeAction` item (`SubscribeToOobeActions` snapshot).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnHead {
    Left(bool),
    Right(bool),
    /// A gesture or other action.
    Other(i32),
}

pub fn decode_oobe_action(payload: &[u8]) -> Result<OnHead, String> {
    let m: pb::OobeAction = decode(payload)?;
    Ok(match m.action.unwrap_or(0) {
        10 => OnHead::Left(true),
        11 => OnHead::Left(false),
        12 => OnHead::Right(true),
        13 => OnHead::Right(false),
        a => OnHead::Other(a),
    })
}

/// Empty request (`maestro_pw.Empty`).
pub fn empty() -> Vec<u8> {
    Vec::new()
}

/// `SetWallclock` request.
pub fn wallclock_request(epoch_ms: u64) -> Vec<u8> {
    pb::Wallclock { epoch_ms: Some(epoch_ms) }.encode_to_vec()
}

/// `ForceMultipointSwitch` request: start exclusive use for `host` (`08 HH`), or release it
/// (`08 HH 10 01`). The release only works in the session that started it.
pub fn force_multipoint_switch_request(host: HostId, release: bool) -> Vec<u8> {
    pb::ForceMultipointSwitchRequest {
        source_endpoint: Some(host.endpoint() as i32),
        release: release.then_some(true),
    }
    .encode_to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_switch_shapes() {
        assert_eq!(force_multipoint_switch_request(HostId::A, false), vec![0x08, 0x0a]);
        assert_eq!(force_multipoint_switch_request(HostId::B, true), vec![0x08, 0x0d, 0x10, 0x01]);
    }

    #[test]
    fn runtime_info() {
        // battery_b{left{80, NOT_CHARGING}, right{75, WIRED}}, placement_b{right_in_case: true, left_in_case: false}
        let m = pb::RuntimeInfo {
            wallclock_ms: Some(1),
            battery: Some(pb::runtime_info::Battery::BatteryB(pb::BatteryInfo {
                case: None,
                left: Some(pb::BatteryLevel { level_percent: Some(80), charging: Some(1) }),
                right: Some(pb::BatteryLevel { level_percent: Some(75), charging: Some(2) }),
            })),
            placement: Some(pb::runtime_info::Placement::PlacementB(pb::BudPlacementB {
                right_in_case: Some(true),
                left_in_case: Some(false),
                low_latency_mode: None,
            })),
        };
        let r = RuntimeInfo::decode(&m.encode_to_vec()).unwrap();
        let b = r.battery.unwrap();
        assert_eq!(b.left, Some(Battery { percent: Some(80), charging: false }));
        assert_eq!(b.right, Some(Battery { percent: Some(75), charging: true }));
        assert_eq!(b.case, None);
        let p = r.placement.unwrap();
        assert_eq!((p.left, p.right), (Some(Placement::Out), Some(Placement::InCase)));
    }

    #[test]
    fn software_info() {
        let m = pb::SoftwareInfo {
            wallclock_ms: None,
            versions: Some(pb::software_info::Versions::VersionsB(pb::FirmwareVersions {
                case: Some(pb::ComponentVersion { version: Some("12".into()), display_label: None }),
                left: Some(pb::ComponentVersion { version: Some("575".into()), display_label: None }),
                right: Some(pb::ComponentVersion { version: Some("575".into()), display_label: None }),
            })),
            random_id: None,
            is_secondary: Some(true),
        };
        let s = SoftwareInfo::decode(&m.encode_to_vec()).unwrap();
        assert!(s.is_secondary);
        assert_eq!(s.versions.left.as_deref(), Some("575"));
        assert!(!SoftwareInfo::decode(&[]).unwrap().is_secondary);
    }

    #[test]
    fn quiet_mode_and_oobe() {
        assert!(decode_quiet_mode(&[0x08, 0x01]).unwrap());
        assert!(!decode_quiet_mode(&[]).unwrap());
        assert_eq!(decode_oobe_action(&[0x08, 0x0b]).unwrap(), OnHead::Left(false));
    }
}
