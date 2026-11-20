// SPDX-License-Identifier: GPL-3.0-or-later
//! Session-bus API: `io.github.kbuds.Manager1` and `io.github.kbuds.Device1`
//! (`dbus/*.xml` is the reference; `tests` below check that the two agree).

use std::sync::{Arc, Mutex};

use kbuds_protocol::settings::AncMode;
use tokio::sync::{mpsc, oneshot, watch};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedObjectPath};
use zbus::{fdo, interface};

use crate::state::DeviceState;

pub const BUS_NAME: &str = "io.github.kbuds.Daemon";
pub const MANAGER_PATH: &str = "/io/github/kbuds";
pub const DEVICE_INTERFACE: &str = "io.github.kbuds.Device1";

/// Object path of a device: `/io/github/kbuds/dev_AA_BB_CC_DD_EE_FF`.
pub fn device_path(address: &str) -> OwnedObjectPath {
    let p = format!("{MANAGER_PATH}/dev_{}", address.replace(':', "_").to_ascii_uppercase());
    OwnedObjectPath::try_from(p).expect("valid path")
}

/// Errors returned to D-Bus callers.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "io.github.kbuds.Error")]
pub enum ApiError {
    #[zbus(error)]
    ZBus(zbus::Error),
    NotConnected(String),
    Unavailable(String),
    NotSupported(String),
    InvalidArgs(String),
    Timeout(String),
    Failed(String),
}

pub type Reply = oneshot::Sender<Result<(), ApiError>>;

/// Requests from D-Bus to a device's supervisor.
#[derive(Debug)]
pub enum Command {
    SetAnc(AncMode, Reply),
    SetMultipoint(bool, Reply),
    SetExclusive(bool, Reply),
    FixAudio(Reply),
    SetAutoFixAudio(bool),
}

async fn call(cmd: &mpsc::Sender<Command>, make: impl FnOnce(Reply) -> Command) -> Result<(), ApiError> {
    let (tx, rx) = oneshot::channel();
    cmd.send(make(tx)).await.map_err(|_| ApiError::Failed("device is gone".into()))?;
    rx.await.unwrap_or_else(|_| Err(ApiError::Failed("request dropped".into())))
}

/// `io.github.kbuds.Manager1`.
pub struct Manager {
    pub devices: Arc<Mutex<Vec<OwnedObjectPath>>>,
}

#[interface(name = "io.github.kbuds.Manager1")]
impl Manager {
    #[zbus(property)]
    fn devices(&self) -> Vec<OwnedObjectPath> {
        self.devices.lock().unwrap().clone()
    }

    #[zbus(property)]
    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    #[zbus(signal)]
    pub async fn device_added(emitter: &SignalEmitter<'_>, device: ObjectPath<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn device_removed(emitter: &SignalEmitter<'_>, device: ObjectPath<'_>) -> zbus::Result<()>;
}

/// `io.github.kbuds.Device1`. Reads come from the published state; writes go to the supervisor.
pub struct Device {
    pub state: watch::Receiver<DeviceState>,
    pub cmd: mpsc::Sender<Command>,
}

impl Device {
    fn s(&self) -> watch::Ref<'_, DeviceState> {
        self.state.borrow()
    }
}

#[interface(name = "io.github.kbuds.Device1")]
impl Device {
    #[zbus(property)]
    fn address(&self) -> String {
        self.s().address.clone()
    }
    #[zbus(property)]
    fn name(&self) -> String {
        self.s().name.clone()
    }
    #[zbus(property)]
    fn model(&self) -> String {
        self.s().model.clone()
    }
    #[zbus(property)]
    fn device_type(&self) -> u32 {
        self.s().device_type
    }
    #[zbus(property)]
    fn sku(&self) -> u32 {
        self.s().sku
    }
    #[zbus(property)]
    fn firmware_left(&self) -> String {
        self.s().firmware_left.clone()
    }
    #[zbus(property)]
    fn firmware_right(&self) -> String {
        self.s().firmware_right.clone()
    }
    #[zbus(property)]
    fn firmware_case(&self) -> String {
        self.s().firmware_case.clone()
    }
    #[zbus(property)]
    fn serial_left(&self) -> String {
        self.s().serial_left.clone()
    }
    #[zbus(property)]
    fn serial_right(&self) -> String {
        self.s().serial_right.clone()
    }
    #[zbus(property)]
    fn serial_case(&self) -> String {
        self.s().serial_case.clone()
    }

    #[zbus(property)]
    fn bluetooth_connected(&self) -> bool {
        self.s().bluetooth_connected
    }
    #[zbus(property)]
    fn state(&self) -> String {
        self.s().state.as_str().to_string()
    }
    #[zbus(property)]
    fn host_id(&self) -> String {
        self.s().host_id.clone()
    }
    #[zbus(property)]
    fn primary(&self) -> String {
        self.s().primary.clone()
    }

    #[zbus(property)]
    fn battery_left(&self) -> i32 {
        self.s().battery_left
    }
    #[zbus(property)]
    fn battery_right(&self) -> i32 {
        self.s().battery_right
    }
    #[zbus(property)]
    fn battery_case(&self) -> i32 {
        self.s().battery_case
    }
    #[zbus(property)]
    fn charging_left(&self) -> bool {
        self.s().charging_left
    }
    #[zbus(property)]
    fn charging_right(&self) -> bool {
        self.s().charging_right
    }
    #[zbus(property)]
    fn charging_case(&self) -> bool {
        self.s().charging_case
    }
    #[zbus(property)]
    fn placement_left(&self) -> String {
        self.s().placement_left.clone()
    }
    #[zbus(property)]
    fn placement_right(&self) -> String {
        self.s().placement_right.clone()
    }

    #[zbus(property)]
    fn anc(&self) -> String {
        self.s().anc.clone()
    }
    #[zbus(property)]
    fn anc_modes(&self) -> Vec<String> {
        self.s().anc_modes.clone()
    }
    async fn set_anc(&self, mode: String) -> Result<(), ApiError> {
        let mode: AncMode = mode.parse().map_err(ApiError::InvalidArgs)?;
        call(&self.cmd, |r| Command::SetAnc(mode, r)).await
    }

    #[zbus(property)]
    fn multipoint(&self) -> bool {
        self.s().multipoint
    }
    #[zbus(property)]
    fn multipoint_supported(&self) -> bool {
        self.s().multipoint_supported
    }
    async fn set_multipoint(&self, enabled: bool) -> Result<(), ApiError> {
        call(&self.cmd, |r| Command::SetMultipoint(enabled, r)).await
    }

    #[zbus(property)]
    fn other_host_has_audio(&self) -> bool {
        self.s().other_host_has_audio
    }
    #[zbus(property)]
    fn exclusive(&self) -> bool {
        self.s().exclusive
    }
    async fn set_exclusive(&self, enabled: bool) -> Result<(), ApiError> {
        call(&self.cmd, |r| Command::SetExclusive(enabled, r)).await
    }

    #[zbus(property)]
    fn audio_profile(&self) -> String {
        self.s().audio_profile.clone()
    }
    #[zbus(property)]
    fn a2dp_channel_mode(&self) -> String {
        self.s().a2dp_channel_mode.clone()
    }
    #[zbus(property)]
    fn audio_problem(&self) -> String {
        self.s().audio_problem.clone()
    }
    async fn fix_audio(&self) -> Result<(), ApiError> {
        call(&self.cmd, Command::FixAudio).await
    }
    #[zbus(property)]
    fn auto_fix_audio(&self) -> bool {
        self.s().auto_fix_audio
    }
    #[zbus(property)]
    async fn set_auto_fix_audio(&mut self, enabled: bool) -> fdo::Result<()> {
        self.cmd
            .send(Command::SetAutoFixAudio(enabled))
            .await
            .map_err(|_| fdo::Error::Failed("device is gone".into()))
    }
}

#[cfg(test)]
mod tests {
    //! The served interfaces must match `dbus/*.xml` exactly (names, signatures, access, args).

    use std::collections::BTreeSet;

    use zbus::object_server::Interface;
    use zbus_xml::{ArgDirection, Node, PropertyAccess};

    use super::*;

    fn summary(xml: &str, iface: &str) -> BTreeSet<String> {
        let node = Node::from_reader(xml.as_bytes()).expect("valid XML");
        let i = node.interfaces().iter().find(|i| i.name() == iface).expect("interface present");
        let mut out = BTreeSet::new();
        for m in i.methods() {
            let args: Vec<_> = m
                .args()
                .iter()
                .map(|a| {
                    let dir = match a.direction() {
                        Some(ArgDirection::Out) => "out",
                        _ => "in",
                    };
                    format!("{}:{}:{dir}", a.name().unwrap_or(""), a.ty().inner())
                })
                .collect();
            out.insert(format!("method {}({})", m.name(), args.join(",")));
        }
        for s in i.signals() {
            let args: Vec<_> =
                s.args().iter().map(|a| format!("{}:{}", a.name().unwrap_or(""), a.ty().inner())).collect();
            out.insert(format!("signal {}({})", s.name(), args.join(",")));
        }
        for p in i.properties() {
            let access = match p.access() {
                PropertyAccess::Read => "read",
                PropertyAccess::Write => "write",
                PropertyAccess::ReadWrite => "readwrite",
            };
            out.insert(format!("property {} {} {access}", p.name(), p.ty().inner()));
        }
        out
    }

    fn served<I: Interface>(iface: &I) -> String {
        let mut s = String::from("<node>\n");
        iface.introspect_to_writer(&mut s, 1);
        s.push_str("</node>\n");
        s
    }

    fn reference(file: &str) -> String {
        let path = format!("{}/../../dbus/{file}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn manager_matches_xml() {
        let m = Manager { devices: Default::default() };
        let name = "io.github.kbuds.Manager1";
        assert_eq!(summary(&served(&m), name), summary(&reference("io.github.kbuds.Manager1.xml"), name));
    }

    #[test]
    fn device_matches_xml() {
        let (_tx, rx) = watch::channel(DeviceState::new("AA:BB:CC:DD:EE:FF", ""));
        let (cmd, _crx) = mpsc::channel(1);
        let d = Device { state: rx, cmd };
        assert_eq!(summary(&served(&d), DEVICE_INTERFACE), summary(&reference("io.github.kbuds.Device1.xml"), DEVICE_INTERFACE));
    }

    #[test]
    fn state_properties_match_xml() {
        // DeviceState::properties() drives PropertiesChanged: it must list every property.
        let xml = reference("io.github.kbuds.Device1.xml");
        let expected: BTreeSet<_> =
            summary(&xml, DEVICE_INTERFACE).into_iter().filter(|l| l.starts_with("property ")).collect();
        let st = DeviceState::new("AA:BB:CC:DD:EE:FF", "");
        let got: BTreeSet<_> = st
            .properties()
            .into_iter()
            .map(|(n, v)| {
                let access = if n == "AutoFixAudio" { "readwrite" } else { "read" };
                format!("property {n} {} {access}", v.value_signature())
            })
            .collect();
        assert_eq!(got, expected);
    }

    #[test]
    fn paths() {
        assert_eq!(device_path("aa:bb:cc:dd:ee:ff").as_str(), "/io/github/kbuds/dev_AA_BB_CC_DD_EE_FF");
    }
}
