// SPDX-License-Identifier: GPL-3.0-or-later
//! Audio-stack checks. The daemon never touches A2DP/HFP
//! itself: it watches BlueZ transports and the PipeWire card profile, and can ask BlueZ to
//! reconnect A2DP so that the PC configures the stream (Opus-G Stereo instead of Dual Mono).

use std::collections::HashMap;
use std::time::Duration;

use serde_json::Value as Json;
use tokio::process::Command;
use zbus::zvariant::OwnedObjectPath;

use crate::bluez::{A2DP_SINK_UUID, A2DP_SOURCE_UUID, Device1Proxy, TransportInfo};

/// A2DP vendor codec id in `MediaTransport1.Codec`.
const VENDOR_CODEC: u8 = 0xff;

/// Opus-G channel mode of a transport configuration, if it is Google Opus.
///
/// Layout: vendor id (u32 LE, 0x000000e0 Google), codec id (u16 LE, 0x0001 Opus), then one byte:
/// frequency (0x80), frame duration (0x08/0x10), channels 0x01 mono, 0x02 stereo, 0x04 dual mono.
pub fn opus_g_channel_mode(codec: u8, config: &[u8]) -> Option<&'static str> {
    if codec != VENDOR_CODEC || config.len() < 7 {
        return None;
    }
    let vendor = u32::from_le_bytes([config[0], config[1], config[2], config[3]]);
    let codec_id = u16::from_le_bytes([config[4], config[5]]);
    if vendor != 0xe0 || codec_id != 0x0001 {
        return None;
    }
    Some(match config[6] & 0x07 {
        0x01 => "mono",
        0x02 => "stereo",
        0x04 => "dual_mono",
        _ => "unknown",
    })
}

/// What the published audio properties should be.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioStatus {
    pub profile: String,
    pub channel_mode: String,
    pub problem: String,
}

/// Combines the device's BlueZ transports with the PipeWire card profile name.
pub fn evaluate(transports: &HashMap<OwnedObjectPath, TransportInfo>, card_profile: Option<&str>) -> AudioStatus {
    let a2dp = transports.values().find(|t| t.uuid == A2DP_SOURCE_UUID);
    let hfp = card_profile.is_some_and(|p| p.starts_with("headset-head-unit"));
    let profile = if a2dp.is_some() {
        "a2dp"
    } else if hfp {
        "hfp"
    } else {
        "none"
    };
    let channel_mode = a2dp.and_then(|t| opus_g_channel_mode(t.codec, &t.configuration)).unwrap_or("");
    let problem = if channel_mode == "dual_mono" {
        "dual_mono"
    } else if profile == "hfp" {
        // A call also uses HFP; the GUI words it as a question and AutoFixAudio never acts on it.
        "hfp_fallback"
    } else {
        ""
    };
    AudioStatus { profile: profile.into(), channel_mode: channel_mode.into(), problem: problem.into() }
}

/// The PipeWire device of a Bluetooth address: id, active profile name and the A2DP profile index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardInfo {
    pub id: u64,
    pub profile: String,
    pub a2dp_index: Option<i64>,
}

pub fn parse_pw_dump(json: &str, address: &str) -> Option<CardInfo> {
    let objects: Vec<Json> = serde_json::from_str(json).ok()?;
    objects.iter().find_map(|o| {
        if o.get("type")?.as_str()? != "PipeWire:Interface:Device" {
            return None;
        }
        let info = o.get("info")?;
        let addr = info.get("props")?.get("api.bluez5.address")?.as_str()?;
        if !addr.eq_ignore_ascii_case(address) {
            return None;
        }
        let params = info.get("params")?;
        let profile = params
            .get("Profile")
            .and_then(|p| p.get(0))
            .and_then(|p| p.get("name"))
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        let a2dp_index = params.get("EnumProfile").and_then(Json::as_array).and_then(|list| {
            list.iter()
                .filter(|p| p.get("name").and_then(Json::as_str).is_some_and(|n| n.starts_with("a2dp-sink")))
                .filter(|p| p.get("available").and_then(Json::as_str) != Some("no"))
                .find_map(|p| p.get("index").and_then(Json::as_i64))
        });
        Some(CardInfo { id: o.get("id")?.as_u64()?, profile, a2dp_index })
    })
}

/// Reads the card from `pw-dump` (pipewire-bin). `None` if PipeWire or the card is not there.
pub async fn card_info(address: &str) -> Option<CardInfo> {
    let out = tokio::time::timeout(Duration::from_secs(5), Command::new("pw-dump").kill_on_drop(true).output())
        .await
        .ok()?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_pw_dump(&String::from_utf8_lossy(&out.stdout), address)
}

/// Reconnects A2DP from this host so PipeWire configures the stream (Stereo). If the card is
/// still on HFP afterwards, switches it to the A2DP profile with `wpctl`.
pub async fn fix(system: &zbus::Connection, device: &OwnedObjectPath, address: &str) -> Result<(), String> {
    let dev = Device1Proxy::builder(system)
        .path(device.clone())
        .map_err(|e| e.to_string())?
        .build()
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!("{address}: reconnecting A2DP from this host");
    if let Err(e) = dev.disconnect_profile(A2DP_SINK_UUID).await {
        tracing::debug!("DisconnectProfile(A2DP): {e}");
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    dev.connect_profile(A2DP_SINK_UUID).await.map_err(|e| format!("A2DP connect failed: {e}"))?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    if let Some(card) = card_info(address).await
        && card.profile.starts_with("headset-head-unit")
        && let Some(index) = card.a2dp_index
    {
        tracing::info!("{address}: switching card {} to A2DP profile {index}", card.id);
        let st = Command::new("wpctl")
            .args(["set-profile", &card.id.to_string(), &index.to_string()])
            .status()
            .await
            .map_err(|e| format!("wpctl: {e}"))?;
        if !st.success() {
            return Err(format!("wpctl set-profile failed ({st})"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transport(config: Vec<u8>) -> TransportInfo {
        TransportInfo {
            path: OwnedObjectPath::try_from("/org/bluez/hci0/dev_AA/sep1/fd0").unwrap(),
            device: OwnedObjectPath::try_from("/org/bluez/hci0/dev_AA").unwrap(),
            uuid: A2DP_SOURCE_UUID.into(),
            codec: 0xff,
            configuration: config,
            state: "idle".into(),
        }
    }

    #[test]
    fn channel_modes() {
        // Seen on hardware: buds-configured 0x94 = 48 kHz + 20 ms + Dual Mono.
        assert_eq!(opus_g_channel_mode(0xff, &[0xe0, 0, 0, 0, 1, 0, 0x94]), Some("dual_mono"));
        assert_eq!(opus_g_channel_mode(0xff, &[0xe0, 0, 0, 0, 1, 0, 0x92]), Some("stereo"));
        assert_eq!(opus_g_channel_mode(0x00, &[0x21, 0x15, 2, 53]), None); // SBC
        assert_eq!(opus_g_channel_mode(0xff, &[0x4f, 0, 0, 0, 1, 0, 0x94]), None); // other vendor
    }

    #[test]
    fn evaluation() {
        let mut t = HashMap::new();
        assert_eq!(evaluate(&t, None).profile, "none");
        assert_eq!(evaluate(&t, Some("headset-head-unit-msbc")).problem, "hfp_fallback");
        t.insert(transport(vec![]).path, transport(vec![0xe0, 0, 0, 0, 1, 0, 0x94]));
        assert_eq!(
            evaluate(&t, None),
            AudioStatus { profile: "a2dp".into(), channel_mode: "dual_mono".into(), problem: "dual_mono".into() }
        );
        t.insert(transport(vec![]).path, transport(vec![0xe0, 0, 0, 0, 1, 0, 0x92]));
        assert_eq!(evaluate(&t, None).problem, "");
    }

    #[test]
    fn pw_dump() {
        let json = r#"[
          {"id": 30, "type": "PipeWire:Interface:Node", "info": {"props": {}}},
          {"id": 77, "type": "PipeWire:Interface:Device", "info": {
            "props": {"api.bluez5.address": "AA:BB:CC:DD:EE:FF"},
            "params": {
              "EnumProfile": [
                {"index": 0, "name": "off"},
                {"index": 1, "name": "a2dp-sink", "available": "yes"},
                {"index": 3, "name": "headset-head-unit", "available": "yes"}
              ],
              "Profile": [{"index": 3, "name": "headset-head-unit"}]
            }}}
        ]"#;
        assert_eq!(
            parse_pw_dump(json, "aa:bb:cc:dd:ee:ff"),
            Some(CardInfo { id: 77, profile: "headset-head-unit".into(), a2dp_index: Some(1) })
        );
        assert_eq!(parse_pw_dump(json, "11:22:33:44:55:66"), None);
    }
}
