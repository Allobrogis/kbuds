// SPDX-License-Identifier: GPL-3.0-or-later
//! pw_rpc service and method ids: the 65599 string hash.
//!
//! The service id hashes the fully qualified name (`"maestro_pw.Maestro"`), the method id the bare
//! method name (`"WriteSetting"`).

/// pw_tokenizer-style 65599 hash, as the app computes it: bytes are taken as signed (Java `byte`).
pub const fn pw_hash(name: &str) -> u32 {
    let bytes = name.as_bytes();
    let mut hash = bytes.len() as u32;
    let mut coef: u32 = 65599;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i] as i8 as i32 as u32;
        hash = hash.wrapping_add(b.wrapping_mul(coef));
        coef = coef.wrapping_mul(65599);
        i += 1;
    }
    hash
}

/// A pw_rpc method: service and method id, plus names for logs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Method {
    pub service: &'static str,
    pub name: &'static str,
    pub service_id: u32,
    pub method_id: u32,
}

impl Method {
    pub const fn new(service: &'static str, name: &'static str) -> Self {
        Self { service, name, service_id: pw_hash(service), method_id: pw_hash(name) }
    }
}

pub const MAESTRO: &str = "maestro_pw.Maestro";
pub const MULTIPOINT: &str = "maestro_pw.Multipoint";

pub const GET_SOFTWARE_INFO: Method = Method::new(MAESTRO, "GetSoftwareInfo");
pub const GET_HARDWARE_INFO: Method = Method::new(MAESTRO, "GetHardwareInfo");
pub const SUBSCRIBE_RUNTIME_INFO: Method = Method::new(MAESTRO, "SubscribeRuntimeInfo");
pub const SET_WALLCLOCK: Method = Method::new(MAESTRO, "SetWallclock");
pub const WRITE_SETTING: Method = Method::new(MAESTRO, "WriteSetting");
pub const READ_SETTING: Method = Method::new(MAESTRO, "ReadSetting");
pub const SUBSCRIBE_SETTINGS_CHANGES: Method = Method::new(MAESTRO, "SubscribeToSettingsChanges");
pub const SUBSCRIBE_OOBE_ACTIONS: Method = Method::new(MAESTRO, "SubscribeToOobeActions");
pub const SUBSCRIBE_QUIET_MODE: Method = Method::new(MULTIPOINT, "SubscribeToQuietModeStatus");
pub const FORCE_MULTIPOINT_SWITCH: Method = Method::new(MULTIPOINT, "ForceMultipointSwitch");

/// Every method this crate knows, for log decoding.
pub const ALL: &[Method] = &[
    GET_SOFTWARE_INFO,
    GET_HARDWARE_INFO,
    SUBSCRIBE_RUNTIME_INFO,
    SET_WALLCLOCK,
    WRITE_SETTING,
    READ_SETTING,
    SUBSCRIBE_SETTINGS_CHANGES,
    SUBSCRIBE_OOBE_ACTIONS,
    SUBSCRIBE_QUIET_MODE,
    FORCE_MULTIPOINT_SWITCH,
];

/// Looks up a known method by ids.
pub fn lookup(service_id: u32, method_id: u32) -> Option<&'static Method> {
    ALL.iter().find(|m| m.service_id == service_id && m.method_id == method_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_table() {
        assert_eq!(pw_hash("maestro_pw.Maestro"), 0x7ede71ea);
        assert_eq!(pw_hash("maestro_pw.Multipoint"), 0x4e4abee7);
        assert_eq!(GET_SOFTWARE_INFO.method_id, 0x7199fa44);
        assert_eq!(GET_HARDWARE_INFO.method_id, 0x28eca5e3);
        assert_eq!(SUBSCRIBE_RUNTIME_INFO.method_id, 0xe61e8290);
        assert_eq!(SET_WALLCLOCK.method_id, 0x673bed4e);
        assert_eq!(WRITE_SETTING.method_id, 0x9e8c9a1d);
        assert_eq!(READ_SETTING.method_id, 0xaed0ae51);
        assert_eq!(SUBSCRIBE_SETTINGS_CHANGES.method_id, 0x2821adf5);
        assert_eq!(SUBSCRIBE_OOBE_ACTIONS.method_id, 0x57f3becb);
        assert_eq!(SUBSCRIBE_QUIET_MODE.method_id, 0xd978edbe);
        assert_eq!(FORCE_MULTIPOINT_SWITCH.method_id, 0x473f375f);
    }

    #[test]
    fn signed_bytes() {
        // Non-ASCII bytes count as negative, as in Java.
        // "é" = C3 A9 → -61, -87.
        let expected = 2u32
            .wrapping_add((-61i32 as u32).wrapping_mul(65599))
            .wrapping_add((-87i32 as u32).wrapping_mul(65599u32.wrapping_mul(65599)));
        assert_eq!(pw_hash("é"), expected);
    }
}
