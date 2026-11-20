// SPDX-License-Identifier: GPL-3.0-or-later
//! Everything the daemon publishes about one device. Mirrors `dbus/io.github.kbuds.Device1.xml`.

use zbus::zvariant::{OwnedValue, Value};

/// Maestro session state (`Device1.State`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

impl SessionState {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionState::Disconnected => "disconnected",
            SessionState::Connecting => "connecting",
            SessionState::Connected => "connected",
            SessionState::Reconnecting => "reconnecting",
        }
    }
}

/// Published device state. "Unknown" is "" for strings and -1 for levels.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceState {
    pub address: String,
    pub name: String,
    pub model: String,
    pub device_type: u32,
    pub sku: u32,
    pub firmware_left: String,
    pub firmware_right: String,
    pub firmware_case: String,
    pub serial_left: String,
    pub serial_right: String,
    pub serial_case: String,

    pub bluetooth_connected: bool,
    pub state: SessionState,
    pub host_id: String,
    pub primary: String,

    pub battery_left: i32,
    pub battery_right: i32,
    pub battery_case: i32,
    pub charging_left: bool,
    pub charging_right: bool,
    pub charging_case: bool,
    pub placement_left: String,
    pub placement_right: String,

    pub anc: String,
    pub anc_modes: Vec<String>,
    pub multipoint: bool,
    pub multipoint_supported: bool,

    pub other_host_has_audio: bool,
    pub exclusive: bool,

    pub audio_profile: String,
    pub a2dp_channel_mode: String,
    pub audio_problem: String,
    pub auto_fix_audio: bool,
}

impl DeviceState {
    pub fn new(address: &str, name: &str) -> Self {
        Self {
            address: address.to_string(),
            name: name.to_string(),
            model: String::new(),
            device_type: 0,
            sku: 0,
            firmware_left: String::new(),
            firmware_right: String::new(),
            firmware_case: String::new(),
            serial_left: String::new(),
            serial_right: String::new(),
            serial_case: String::new(),
            bluetooth_connected: false,
            state: SessionState::Disconnected,
            host_id: String::new(),
            primary: String::new(),
            battery_left: -1,
            battery_right: -1,
            battery_case: -1,
            charging_left: false,
            charging_right: false,
            charging_case: false,
            placement_left: String::new(),
            placement_right: String::new(),
            anc: String::new(),
            anc_modes: Vec::new(),
            multipoint: false,
            multipoint_supported: false,
            other_host_has_audio: false,
            exclusive: false,
            audio_profile: "none".into(),
            a2dp_channel_mode: String::new(),
            audio_problem: String::new(),
            auto_fix_audio: false,
        }
    }

    /// Clears everything learned in a Maestro session (kept: identity from BlueZ, audio state).
    pub fn reset_session(&mut self) {
        let fresh = DeviceState::new(&self.address, &self.name);
        self.host_id = fresh.host_id;
        self.primary = fresh.primary;
        self.battery_left = -1;
        self.battery_right = -1;
        self.battery_case = -1;
        self.charging_left = false;
        self.charging_right = false;
        self.charging_case = false;
        self.placement_left.clear();
        self.placement_right.clear();
        self.anc.clear();
        self.other_host_has_audio = false;
        // The buds end exclusive use when the session closes.
        self.exclusive = false;
    }

    /// Lowest known bud level, for BlueZ's battery provider.
    pub fn lowest_bud(&self) -> Option<u8> {
        [self.battery_left, self.battery_right].into_iter().filter(|l| *l >= 0).min().map(|l| l as u8)
    }

    /// All D-Bus properties, by name.
    pub fn properties(&self) -> Vec<(&'static str, OwnedValue)> {
        fn v<'a>(x: impl Into<Value<'a>>) -> OwnedValue {
            x.into().try_to_owned().expect("plain values have no fds")
        }
        vec![
            ("Address", v(self.address.as_str())),
            ("Name", v(self.name.as_str())),
            ("Model", v(self.model.as_str())),
            ("DeviceType", v(self.device_type)),
            ("Sku", v(self.sku)),
            ("FirmwareLeft", v(self.firmware_left.as_str())),
            ("FirmwareRight", v(self.firmware_right.as_str())),
            ("FirmwareCase", v(self.firmware_case.as_str())),
            ("SerialLeft", v(self.serial_left.as_str())),
            ("SerialRight", v(self.serial_right.as_str())),
            ("SerialCase", v(self.serial_case.as_str())),
            ("BluetoothConnected", v(self.bluetooth_connected)),
            ("State", v(self.state.as_str())),
            ("HostId", v(self.host_id.as_str())),
            ("Primary", v(self.primary.as_str())),
            ("BatteryLeft", v(self.battery_left)),
            ("BatteryRight", v(self.battery_right)),
            ("BatteryCase", v(self.battery_case)),
            ("ChargingLeft", v(self.charging_left)),
            ("ChargingRight", v(self.charging_right)),
            ("ChargingCase", v(self.charging_case)),
            ("PlacementLeft", v(self.placement_left.as_str())),
            ("PlacementRight", v(self.placement_right.as_str())),
            ("Anc", v(self.anc.as_str())),
            ("AncModes", v(self.anc_modes.clone())),
            ("Multipoint", v(self.multipoint)),
            ("MultipointSupported", v(self.multipoint_supported)),
            ("OtherHostHasAudio", v(self.other_host_has_audio)),
            ("Exclusive", v(self.exclusive)),
            ("AudioProfile", v(self.audio_profile.as_str())),
            ("A2dpChannelMode", v(self.a2dp_channel_mode.as_str())),
            ("AudioProblem", v(self.audio_problem.as_str())),
            ("AutoFixAudio", v(self.auto_fix_audio)),
        ]
    }

    /// Properties whose value differs from `old`.
    pub fn changed_since(&self, old: &DeviceState) -> Vec<(&'static str, OwnedValue)> {
        let before = old.properties();
        self.properties()
            .into_iter()
            .zip(before)
            .filter(|((_, a), (_, b))| a != b)
            .map(|(new, _)| new)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff() {
        let a = DeviceState::new("AA:BB:CC:DD:EE:FF", "Pixel Buds Pro");
        let mut b = a.clone();
        b.battery_left = 50;
        b.anc_modes = vec!["off".into()];
        let names: Vec<_> = b.changed_since(&a).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["BatteryLeft", "AncModes"]);
        assert_eq!(b.lowest_bud(), Some(50));
        b.battery_right = 20;
        assert_eq!(b.lowest_bud(), Some(20));
    }
}
