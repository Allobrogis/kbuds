// SPDX-License-Identifier: GPL-3.0-or-later
//! Models and feature gating.
//!
//! The app gates features on (device type, firmware number) with a bundled matrix that the server
//! can override. A feature is supported iff it is listed for the type and the firmware is at least
//! the listed minimum; with no entry for the type or no firmware number it is unknown.

use std::fmt;

/// Product, from `HardwareInfo.device_type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Model {
    /// Device type 4 or 5 (allegro).
    Pro,
    /// Device type 6 (largo).
    Pro2,
    /// Device type 9 (ag12/ag22).
    A2,
    /// Pixel Buds 2020 / A-Series: another transport, not supported.
    Legacy,
    Unknown,
}

impl Model {
    pub fn from_device_type(t: i32) -> Self {
        match t {
            4 | 5 => Model::Pro,
            6 => Model::Pro2,
            9 => Model::A2,
            1 | 2 => Model::Legacy,
            _ => Model::Unknown,
        }
    }

    /// Stable identifier used on D-Bus.
    pub fn as_str(self) -> &'static str {
        match self {
            Model::Pro => "pro",
            Model::Pro2 => "pro2",
            Model::A2 => "2a",
            Model::Legacy | Model::Unknown => "unsupported",
        }
    }

    pub fn marketing_name(self) -> &'static str {
        match self {
            Model::Pro => "Pixel Buds Pro",
            Model::Pro2 => "Pixel Buds Pro 2",
            Model::A2 => "Pixel Buds 2a",
            Model::Legacy => "Pixel Buds (legacy)",
            Model::Unknown => "unknown Pixel Buds",
        }
    }
}

impl fmt::Display for Model {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.marketing_name())
    }
}

/// App feature ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Feature {
    TouchControls = 2,
    Anc = 11,
    VolumeEq = 13,
    UserEq = 14,
    Balance = 15,
    Mono = 17,
    HearingWellness = 20,
    ConversationDetection = 21,
    Multipoint = 25,
    AdaptiveAnc = 31,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureState {
    Supported,
    NotSupported,
    /// No matrix entry for the device type, or firmware number unknown.
    Unknown,
}

/// Bundled matrix: (device type, feature id, minimum firmware number; 0 = no gate).
const MATRIX: &[(i32, u32, u64)] = &[
    // Pixel Buds Pro (4)
    (4, 2, 0),
    (4, 5, 0),
    (4, 8, 0),
    (4, 11, 0),
    (4, 12, 0),
    (4, 13, 120),
    (4, 14, 208),
    (4, 15, 208),
    (4, 16, 300),
    (4, 17, 300),
    (4, 18, 300),
    (4, 19, 539),
    (4, 20, 536),
    (4, 21, 536),
    (4, 24, 0),
    // Pixel Buds Pro 2 (6)
    (6, 2, 0),
    (6, 5, 0),
    (6, 8, 0),
    (6, 11, 0),
    (6, 12, 0),
    (6, 13, 0),
    (6, 14, 0),
    (6, 15, 0),
    (6, 16, 0),
    (6, 17, 0),
    (6, 18, 0),
    (6, 19, 0),
    (6, 20, 0),
    (6, 21, 1723068496),
    (6, 22, 0),
    (6, 24, 0),
    (6, 25, 0),
    (6, 28, 0),
    (6, 29, 1728515707),
    (6, 30, 1728514599),
    (6, 31, 1754510785),
    (6, 32, 1754510785),
    (6, 40, 1773940180),
    (6, 41, 1782902755),
    (6, 42, 1782386300),
    (6, 43, 1782386300),
    (6, 47, 1786583974),
    (6, 48, 1782386300),
    // Pixel Buds 2a (9)
    (9, 2, 0),
    (9, 5, 0),
    (9, 11, 0),
    (9, 12, 0),
    (9, 13, 0),
    (9, 14, 0),
    (9, 15, 0),
    (9, 17, 0),
    (9, 18, 0),
    (9, 19, 0),
    (9, 20, 0),
    (9, 22, 0),
    (9, 24, 0),
    (9, 25, 0),
    (9, 41, 1782902755),
    (9, 42, 1782386300),
    (9, 43, 1782386300),
    (9, 48, 1782386300),
];

/// Feature state per the bundled matrix.
pub fn feature_state(device_type: i32, firmware: Option<u64>, feature: Feature) -> FeatureState {
    let id = feature as u32;
    if !MATRIX.iter().any(|(t, _, _)| *t == device_type) {
        return FeatureState::Unknown;
    }
    let Some(fw) = firmware else { return FeatureState::Unknown };
    match MATRIX.iter().find(|(t, f, _)| *t == device_type && *f == id) {
        Some((_, _, min)) if fw >= *min => FeatureState::Supported,
        _ => FeatureState::NotSupported,
    }
}

/// Firmware number used for gating: min(left, right) of the integer versions.
/// On the Pro this is e.g. 575; on the Pro 2 and 2a a Unix build timestamp.
pub fn firmware_number(left: Option<&str>, right: Option<&str>) -> Option<u64> {
    let parse = |v: Option<&str>| v.and_then(|s| s.trim().parse::<u64>().ok());
    match (parse(left), parse(right)) {
        (Some(l), Some(r)) => Some(l.min(r)),
        (Some(v), None) | (None, Some(v)) => Some(v),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gating() {
        assert_eq!(feature_state(4, Some(575), Feature::Anc), FeatureState::Supported);
        assert_eq!(feature_state(4, Some(575), Feature::AdaptiveAnc), FeatureState::NotSupported);
        assert_eq!(feature_state(4, Some(575), Feature::Multipoint), FeatureState::NotSupported);
        assert_eq!(feature_state(4, Some(500), Feature::ConversationDetection), FeatureState::NotSupported);
        assert_eq!(feature_state(4, Some(536), Feature::ConversationDetection), FeatureState::Supported);
        assert_eq!(feature_state(6, Some(1754510785), Feature::AdaptiveAnc), FeatureState::Supported);
        assert_eq!(feature_state(6, Some(1754510784), Feature::AdaptiveAnc), FeatureState::NotSupported);
        assert_eq!(feature_state(5, Some(575), Feature::Anc), FeatureState::Unknown);
        assert_eq!(feature_state(4, None, Feature::Anc), FeatureState::Unknown);
        assert_eq!(feature_state(9, Some(1), Feature::Multipoint), FeatureState::Supported);
    }

    #[test]
    fn firmware() {
        assert_eq!(firmware_number(Some("575"), Some("574")), Some(574));
        assert_eq!(firmware_number(Some("575"), None), Some(575));
        assert_eq!(firmware_number(Some("x"), None), None);
    }

    #[test]
    fn models() {
        assert_eq!(Model::from_device_type(5), Model::Pro);
        assert_eq!(Model::from_device_type(2), Model::Legacy);
        assert_eq!(Model::from_device_type(12), Model::Unknown);
    }
}
