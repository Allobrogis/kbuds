// SPDX-License-Identifier: GPL-3.0-or-later
//! Settings of the Pigweed models (`AllegroSetting`).
//!
//! `WriteSetting` takes a `SettingValue` holding exactly one field; `ReadSetting` takes the field
//! number as `AllegroSettingId`. Only the fields in [`Setting`] can be written: an allow-list of
//! settings that were all written and read back on a Pixel Buds Pro.

use std::fmt;
use std::str::FromStr;

use prost::Message;

use crate::proto::maestro_pw::{
    self as pb, AllegroSetting, ReadSettingRequest, SettingValue, TouchActionTarget, TouchAndHold, TouchSlot,
    UserEq, allegro_setting, read_setting_request, setting_value,
};

/// Noise-control mode (`AncState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AncMode {
    Off,
    Active,
    Aware,
    Adaptive,
}

impl AncMode {
    pub const ALL: [AncMode; 4] = [AncMode::Off, AncMode::Active, AncMode::Aware, AncMode::Adaptive];

    pub fn raw(self) -> i32 {
        match self {
            AncMode::Off => 1,
            AncMode::Active => 2,
            AncMode::Aware => 3,
            AncMode::Adaptive => 4,
        }
    }

    /// `None` for 0 (UNKNOWN, reported while the buds are in the case) and unknown values.
    pub fn from_raw(v: i32) -> Option<Self> {
        match v {
            1 => Some(AncMode::Off),
            2 => Some(AncMode::Active),
            3 => Some(AncMode::Aware),
            4 => Some(AncMode::Adaptive),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AncMode::Off => "off",
            AncMode::Active => "active",
            AncMode::Aware => "aware",
            AncMode::Adaptive => "adaptive",
        }
    }
}

impl fmt::Display for AncMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AncMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        AncMode::ALL
            .into_iter()
            .find(|m| m.as_str() == s)
            .ok_or_else(|| format!("unknown ANC mode {s:?} (off, active, aware, adaptive)"))
    }
}

/// Press-and-hold action (`TouchAction`) as written by the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HoldAction {
    ToggleAnc,
    Assistant,
}

impl HoldAction {
    pub fn raw(self) -> i32 {
        match self {
            HoldAction::ToggleAnc => pb::TouchAction::ToggleAnc as i32,
            HoldAction::Assistant => pb::TouchAction::Assistant as i32,
        }
    }

    pub fn from_raw(v: i32) -> Option<Self> {
        match v {
            5 => Some(HoldAction::ToggleAnc),
            6 => Some(HoldAction::Assistant),
            _ => None,
        }
    }
}

/// `AllegroSettingId` of the supported settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum SettingId {
    OnHeadDetection = 2,
    TouchAndHold = 7,
    Multipoint = 11,
    Anc = 13,
    UserEq = 16,
    ConversationDetection = 22,
}

impl SettingId {
    pub const ALL: [SettingId; 6] = [
        SettingId::OnHeadDetection,
        SettingId::TouchAndHold,
        SettingId::Multipoint,
        SettingId::Anc,
        SettingId::UserEq,
        SettingId::ConversationDetection,
    ];

    pub fn field(self) -> i32 {
        self as i32
    }
}

/// A decoded setting value.
#[derive(Clone, Debug, PartialEq)]
pub enum Setting {
    OnHeadDetection(bool),
    /// Left and right press-and-hold actions.
    TouchAndHold { left: Option<HoldAction>, right: Option<HoldAction> },
    Multipoint(bool),
    /// `None` = 0/UNKNOWN: the buds report this while in the case.
    Anc(Option<AncMode>),
    /// Low bass, bass, mid, treble, upper treble in dB (−6..6, step 0.5).
    UserEq([f32; 5]),
    ConversationDetection(bool),
    /// Any other AllegroSetting field, by number (not interpreted).
    Other(i32),
}

impl Setting {
    pub fn id(&self) -> Option<SettingId> {
        Some(match self {
            Setting::OnHeadDetection(_) => SettingId::OnHeadDetection,
            Setting::TouchAndHold { .. } => SettingId::TouchAndHold,
            Setting::Multipoint(_) => SettingId::Multipoint,
            Setting::Anc(_) => SettingId::Anc,
            Setting::UserEq(_) => SettingId::UserEq,
            Setting::ConversationDetection(_) => SettingId::ConversationDetection,
            Setting::Other(_) => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SettingError {
    #[error("setting cannot be written: {0}")]
    NotWritable(String),
    #[error("value out of range: {0}")]
    OutOfRange(String),
    #[error("undecodable setting: {0}")]
    Decode(String),
}

/// `ReadSetting` request payload for `id`.
pub fn read_request(id: SettingId) -> Vec<u8> {
    ReadSettingRequest { setting_id: Some(read_setting_request::SettingId::Allegro(id.field())) }.encode_to_vec()
}

/// `WriteSetting` request payload. Only allow-listed settings and in-range values are accepted.
pub fn write_request(setting: &Setting) -> Result<Vec<u8>, SettingError> {
    use allegro_setting::Value as V;
    let value = match setting {
        Setting::OnHeadDetection(b) => V::OnHeadDetection(*b),
        Setting::Multipoint(b) => V::Multipoint(*b),
        Setting::ConversationDetection(b) => V::ConversationDetection(*b),
        Setting::Anc(Some(m)) => V::AncState(m.raw()),
        Setting::Anc(None) => return Err(SettingError::OutOfRange("ANC mode UNKNOWN cannot be written".into())),
        Setting::TouchAndHold { left, right } => {
            let slot = |a: &Option<HoldAction>| -> Result<TouchSlot, SettingError> {
                let a = a.ok_or_else(|| SettingError::OutOfRange("both press-and-hold actions are required".into()))?;
                Ok(TouchSlot { hold_action: Some(TouchActionTarget { action: Some(a.raw()), custom: None }) })
            };
            V::TouchAndHold(TouchAndHold { left: Some(slot(left)?), right: Some(slot(right)?) })
        }
        Setting::UserEq(bands) => {
            for b in bands {
                // −6..6 dB in 0.5 dB steps.
                if !(-6.0..=6.0).contains(b) || (b * 2.0).fract() != 0.0 {
                    return Err(SettingError::OutOfRange(format!("EQ band {b} dB (−6..6, step 0.5)")));
                }
            }
            V::UserEq(UserEq {
                low_bass: Some(bands[0]),
                bass: Some(bands[1]),
                mid: Some(bands[2]),
                treble: Some(bands[3]),
                upper_treble: Some(bands[4]),
            })
        }
        Setting::Other(f) => return Err(SettingError::NotWritable(format!("allegro field {f}"))),
    };
    Ok(SettingValue { group: Some(setting_value::Group::Allegro(AllegroSetting { value: Some(value) })) }
        .encode_to_vec())
}

/// Decodes a `SettingValue` (ReadSetting reply or settings push). `Ok(None)` = not an allegro
/// setting or an empty one (the firmware does not know the field).
pub fn decode(payload: &[u8]) -> Result<Option<Setting>, SettingError> {
    use allegro_setting::Value as V;
    let v = SettingValue::decode(payload).map_err(|e| SettingError::Decode(e.to_string()))?;
    let Some(setting_value::Group::Allegro(a)) = v.group else { return Ok(None) };
    let Some(value) = a.value else { return Ok(None) };
    Ok(Some(match value {
        V::OnHeadDetection(b) => Setting::OnHeadDetection(b),
        V::Multipoint(b) => Setting::Multipoint(b),
        V::ConversationDetection(b) => Setting::ConversationDetection(b),
        V::AncState(s) => Setting::Anc(AncMode::from_raw(s)),
        V::TouchAndHold(t) => {
            let action = |s: Option<TouchSlot>| {
                s.and_then(|s| s.hold_action).and_then(|t| t.action).and_then(HoldAction::from_raw)
            };
            Setting::TouchAndHold { left: action(t.left), right: action(t.right) }
        }
        V::UserEq(e) => Setting::UserEq([
            e.low_bass.unwrap_or(0.0),
            e.bass.unwrap_or(0.0),
            e.mid.unwrap_or(0.0),
            e.treble.unwrap_or(0.0),
            e.upper_treble.unwrap_or(0.0),
        ]),
        other => Setting::Other(field_number(&other)),
    }))
}

fn field_number(v: &allegro_setting::Value) -> i32 {
    // Encode and read the tag back: avoids listing all 39 variants.
    let mut buf = Vec::new();
    v.encode(&mut buf);
    prost::encoding::decode_key(&mut &buf[..]).map(|(tag, _)| tag as i32).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> Vec<u8> {
        hex::decode(s.replace(' ', "")).unwrap()
    }

    #[test]
    fn write_encodings() {
        // Byte sequences verified on hardware.
        assert_eq!(write_request(&Setting::Anc(Some(AncMode::Off))).unwrap(), h("22 02 68 01"));
        assert_eq!(write_request(&Setting::Anc(Some(AncMode::Active))).unwrap(), h("22 02 68 02"));
        assert_eq!(write_request(&Setting::Anc(Some(AncMode::Aware))).unwrap(), h("22 02 68 03"));
        assert_eq!(write_request(&Setting::Anc(Some(AncMode::Adaptive))).unwrap(), h("22 02 68 04"));
        assert_eq!(write_request(&Setting::Multipoint(true)).unwrap(), h("22 02 58 01"));
        assert_eq!(write_request(&Setting::Multipoint(false)).unwrap(), h("22 02 58 00"));
        assert_eq!(write_request(&Setting::OnHeadDetection(false)).unwrap(), h("22 02 10 00"));
        assert_eq!(write_request(&Setting::ConversationDetection(false)).unwrap(), h("22 03 b0 01 00"));
        assert_eq!(
            write_request(&Setting::TouchAndHold {
                left: Some(HoldAction::ToggleAnc),
                right: Some(HoldAction::Assistant)
            })
            .unwrap(),
            h("22 0e 3a 0c 0a 04 22 02 08 05 12 04 22 02 08 06")
        );
        let eq = write_request(&Setting::UserEq([0.0, 1.5, -2.0, 6.0, -6.0])).unwrap();
        assert_eq!(decode(&eq).unwrap(), Some(Setting::UserEq([0.0, 1.5, -2.0, 6.0, -6.0])));
    }

    #[test]
    fn refused_writes() {
        assert!(write_request(&Setting::Other(5)).is_err());
        assert!(write_request(&Setting::Anc(None)).is_err());
        assert!(write_request(&Setting::UserEq([6.5, 0.0, 0.0, 0.0, 0.0])).is_err());
        assert!(write_request(&Setting::UserEq([0.25, 0.0, 0.0, 0.0, 0.0])).is_err());
        assert!(write_request(&Setting::TouchAndHold { left: None, right: Some(HoldAction::Assistant) }).is_err());
    }

    #[test]
    fn read_requests() {
        assert_eq!(read_request(SettingId::Anc), h("20 0d"));
        assert_eq!(read_request(SettingId::Multipoint), h("20 0b"));
    }

    #[test]
    fn decodes() {
        assert_eq!(decode(&h("22 02 68 03")).unwrap(), Some(Setting::Anc(Some(AncMode::Aware))));
        // In the case: anc_state 0.
        assert_eq!(decode(&h("22 02 68 00")).unwrap(), Some(Setting::Anc(None)));
        // Empty allegro{}: field unknown to the firmware.
        assert_eq!(decode(&h("22 00")).unwrap(), None);
        // Field 5 (diagnostics).
        assert_eq!(decode(&h("22 02 28 01")).unwrap(), Some(Setting::Other(5)));
        assert!(decode(&h("22 05 ff")).is_err());
    }
}
