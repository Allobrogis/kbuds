// SPDX-License-Identifier: GPL-3.0-or-later
//! Persistent settings: `$XDG_CONFIG_HOME/kbuds/kbudsd.toml`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceConfig {
    /// Run FixAudio automatically when a Dual Mono stream is detected.
    pub auto_fix_audio: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Per device, keyed by Bluetooth address.
    pub devices: BTreeMap<String, DeviceConfig>,
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("kbuds").join("kbudsd.toml"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else { return Self::default() };
        match std::fs::read_to_string(&path) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                tracing::warn!("ignoring invalid {}: {e}", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(path.parent().unwrap())?;
            let tmp = path.with_extension("toml.tmp");
            std::fs::write(&tmp, toml::to_string_pretty(self).map_err(std::io::Error::other)?)?;
            std::fs::rename(tmp, &path)
        };
        if let Err(e) = write() {
            tracing::warn!("cannot save {}: {e}", path.display());
        }
    }

    pub fn device(&self, address: &str) -> DeviceConfig {
        self.devices.get(address).cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut c = Config::default();
        c.devices.insert("AA:BB:CC:DD:EE:FF".into(), DeviceConfig { auto_fix_audio: true });
        let s = toml::to_string_pretty(&c).unwrap();
        assert_eq!(toml::from_str::<Config>(&s).unwrap(), c);
        assert!(c.device("AA:BB:CC:DD:EE:FF").auto_fix_audio);
        assert!(!c.device("11:22:33:44:55:66").auto_fix_audio);
    }
}
