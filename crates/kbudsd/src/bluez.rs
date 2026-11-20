// SPDX-License-Identifier: GPL-3.0-or-later
//! BlueZ on the system bus: proxies and an object monitor.
//!
//! The monitor mirrors `org.bluez`'s object tree and reports the objects we care about: devices
//! that expose the Maestro service, and A2DP media transports.

use std::collections::HashMap;

use futures_util::StreamExt;
use kbuds_protocol::MAESTRO_UUID;
use tokio::sync::mpsc;
use zbus::fdo::{DBusProxy, ObjectManagerProxy};
use zbus::zvariant::{Array, ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::{MatchRule, MessageStream, proxy};

pub const SERVICE: &str = "org.bluez";
pub const DEVICE_IFACE: &str = "org.bluez.Device1";
pub const TRANSPORT_IFACE: &str = "org.bluez.MediaTransport1";
/// Remote A2DP sink (the buds), used for profile (dis)connects.
pub const A2DP_SINK_UUID: &str = "0000110b-0000-1000-8000-00805f9b34fb";
/// Local A2DP source endpoint UUID, as found in `MediaTransport1.UUID` on the PC.
pub const A2DP_SOURCE_UUID: &str = "0000110a-0000-1000-8000-00805f9b34fb";

#[proxy(interface = "org.bluez.Device1", default_service = "org.bluez")]
pub trait Device1 {
    fn connect_profile(&self, uuid: &str) -> zbus::Result<()>;
    fn disconnect_profile(&self, uuid: &str) -> zbus::Result<()>;
}

#[proxy(interface = "org.bluez.ProfileManager1", default_service = "org.bluez", default_path = "/org/bluez")]
pub trait ProfileManager1 {
    fn register_profile(
        &self,
        profile: &ObjectPath<'_>,
        uuid: &str,
        options: HashMap<&str, Value<'_>>,
    ) -> zbus::Result<()>;
    fn unregister_profile(&self, profile: &ObjectPath<'_>) -> zbus::Result<()>;
}

#[proxy(interface = "org.bluez.BatteryProviderManager1", default_service = "org.bluez")]
pub trait BatteryProviderManager1 {
    fn register_battery_provider(&self, provider: &ObjectPath<'_>) -> zbus::Result<()>;
    fn unregister_battery_provider(&self, provider: &ObjectPath<'_>) -> zbus::Result<()>;
}

/// A paired device that exposes the Maestro service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub path: OwnedObjectPath,
    pub adapter: OwnedObjectPath,
    pub address: String,
    pub alias: String,
    pub connected: bool,
}

/// An `org.bluez.MediaTransport1`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportInfo {
    pub path: OwnedObjectPath,
    pub device: OwnedObjectPath,
    pub uuid: String,
    pub codec: u8,
    pub configuration: Vec<u8>,
    pub state: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Device(DeviceInfo),
    DeviceRemoved(OwnedObjectPath),
    Transport(TransportInfo),
    TransportRemoved(OwnedObjectPath),
    /// `org.bluez` (re)appeared: re-register profile and battery provider.
    ServiceUp,
    ServiceDown,
}

type Props = HashMap<String, OwnedValue>;
type Ifaces = HashMap<String, Props>;

fn get_str(p: &Props, k: &str) -> Option<String> {
    p.get(k).and_then(|v| <&str>::try_from(&**v).ok()).map(str::to_string)
}

fn get_path(p: &Props, k: &str) -> Option<OwnedObjectPath> {
    p.get(k).and_then(|v| <ObjectPath<'_>>::try_from(&**v).ok()).map(|p| OwnedObjectPath::from(p.into_owned()))
}

fn get_bool(p: &Props, k: &str) -> Option<bool> {
    p.get(k).and_then(|v| bool::try_from(&**v).ok())
}

fn get_u8(p: &Props, k: &str) -> Option<u8> {
    p.get(k).and_then(|v| u8::try_from(&**v).ok())
}

fn get_array<'a, T: TryFrom<&'a Value<'a>>>(p: &'a Props, k: &str) -> Vec<T> {
    let Some(v) = p.get(k) else { return Vec::new() };
    let Ok(a) = <&Array<'_>>::try_from(&**v) else { return Vec::new() };
    a.iter().filter_map(|x| T::try_from(x).ok()).collect()
}

fn device_info(path: &OwnedObjectPath, p: &Props) -> Option<DeviceInfo> {
    let uuids: Vec<&str> = get_array(p, "UUIDs");
    if !uuids.iter().any(|u| u.eq_ignore_ascii_case(MAESTRO_UUID)) {
        return None;
    }
    let address = get_str(p, "Address")?;
    Some(DeviceInfo {
        path: path.clone(),
        adapter: get_path(p, "Adapter").unwrap_or_else(|| {
            // /org/bluez/hci0/dev_… → /org/bluez/hci0
            let s = path.as_str();
            OwnedObjectPath::try_from(&s[..s.rfind('/').unwrap_or(s.len())]).unwrap_or(path.clone())
        }),
        alias: get_str(p, "Alias").or_else(|| get_str(p, "Name")).unwrap_or_else(|| address.clone()),
        address,
        connected: get_bool(p, "Connected").unwrap_or(false),
    })
}

fn transport_info(path: &OwnedObjectPath, p: &Props) -> Option<TransportInfo> {
    Some(TransportInfo {
        path: path.clone(),
        device: get_path(p, "Device")?,
        uuid: get_str(p, "UUID").unwrap_or_default().to_ascii_lowercase(),
        codec: get_u8(p, "Codec").unwrap_or(0),
        configuration: get_array(p, "Configuration"),
        state: get_str(p, "State").unwrap_or_default(),
    })
}

/// Mirrors BlueZ's objects and sends [`Event`]s. Runs until the system bus connection fails.
pub async fn monitor(conn: zbus::Connection, tx: mpsc::UnboundedSender<Event>) -> zbus::Result<()> {
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(SERVICE)?
        .interface("org.freedesktop.DBus.Properties")?
        .member("PropertiesChanged")?
        .build();
    let mut prop_changes = MessageStream::for_match_rule(rule, &conn, None).await?;
    let om = ObjectManagerProxy::builder(&conn).destination(SERVICE)?.path("/")?.build().await?;
    let mut added = om.receive_interfaces_added().await?;
    let mut removed = om.receive_interfaces_removed().await?;
    let dbus = DBusProxy::new(&conn).await?;
    let mut owner = dbus.receive_name_owner_changed_with_args(&[(0, SERVICE)]).await?;

    let mut mirror: HashMap<OwnedObjectPath, Ifaces> = HashMap::new();
    let emit = |mirror: &HashMap<OwnedObjectPath, Ifaces>, path: &OwnedObjectPath| {
        let Some(ifaces) = mirror.get(path) else { return };
        if let Some(info) = ifaces.get(DEVICE_IFACE).and_then(|p| device_info(path, p)) {
            let _ = tx.send(Event::Device(info));
        }
        if let Some(info) = ifaces.get(TRANSPORT_IFACE).and_then(|p| transport_info(path, p)) {
            let _ = tx.send(Event::Transport(info));
        }
    };

    let load = async |mirror: &mut HashMap<OwnedObjectPath, Ifaces>| {
        mirror.clear();
        match om.get_managed_objects().await {
            Ok(objects) => {
                for (path, ifaces) in objects {
                    let ifaces: Ifaces = ifaces.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
                    mirror.insert(path, ifaces);
                }
                let _ = tx.send(Event::ServiceUp);
                let paths: Vec<_> = mirror.keys().cloned().collect();
                for p in &paths {
                    emit(mirror, p);
                }
            }
            Err(e) => tracing::warn!("BlueZ not available: {e}"),
        }
    };
    load(&mut mirror).await;

    loop {
        tokio::select! {
            Some(sig) = added.next() => {
                let Ok(args) = sig.args() else { continue };
                let path = OwnedObjectPath::from(args.object_path.clone().into_owned());
                let entry = mirror.entry(path.clone()).or_default();
                for (iface, props) in args.interfaces_and_properties {
                    let props: Props = props.into_iter().filter_map(|(k, v)| Some((k.to_string(), v.try_to_owned().ok()?))).collect();
                    entry.insert(iface.to_string(), props);
                }
                emit(&mirror, &path);
            }
            Some(sig) = removed.next() => {
                let Ok(args) = sig.args() else { continue };
                let path = OwnedObjectPath::from(args.object_path.clone().into_owned());
                let Some(entry) = mirror.get_mut(&path) else { continue };
                for iface in args.interfaces.iter() {
                    let iface = iface.to_string();
                    if let Some(props) = entry.remove(&iface) {
                        if iface == DEVICE_IFACE && device_info(&path, &props).is_some() {
                            let _ = tx.send(Event::DeviceRemoved(path.clone()));
                        }
                        if iface == TRANSPORT_IFACE {
                            let _ = tx.send(Event::TransportRemoved(path.clone()));
                        }
                    }
                }
                if entry.is_empty() {
                    mirror.remove(&path);
                }
            }
            Some(msg) = prop_changes.next() => {
                let Ok(msg) = msg else { continue };
                let header = msg.header();
                let Some(path) = header.path() else { continue };
                let path = OwnedObjectPath::from(path.to_owned());
                let Ok((iface, changed, invalidated)) =
                    msg.body().deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>() else { continue };
                let Some(props) = mirror.get_mut(&path).and_then(|i| i.get_mut(&iface)) else { continue };
                let was_ours = iface == DEVICE_IFACE && device_info(&path, props).is_some();
                for (k, v) in changed {
                    props.insert(k, v);
                }
                for k in invalidated {
                    props.remove(&k);
                }
                if was_ours && device_info(&path, props).is_none() {
                    let _ = tx.send(Event::DeviceRemoved(path.clone()));
                }
                emit(&mirror, &path);
            }
            Some(sig) = owner.next() => {
                let Ok(args) = sig.args() else { continue };
                if args.new_owner().is_some() {
                    tracing::info!("BlueZ (re)started");
                    load(&mut mirror).await;
                } else {
                    tracing::warn!("BlueZ stopped");
                    for (path, ifaces) in mirror.drain() {
                        if ifaces.get(DEVICE_IFACE).is_some_and(|p| device_info(&path, p).is_some()) {
                            let _ = tx.send(Event::DeviceRemoved(path));
                        }
                    }
                    let _ = tx.send(Event::ServiceDown);
                }
            }
            else => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ov<'a>(v: impl Into<Value<'a>>) -> OwnedValue {
        v.into().try_to_owned().unwrap()
    }

    #[test]
    fn device_filter() {
        let path = OwnedObjectPath::try_from("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF").unwrap();
        let mut p: Props = HashMap::new();
        p.insert("Address".into(), ov("AA:BB:CC:DD:EE:FF"));
        p.insert("Alias".into(), ov("Pixel Buds Pro"));
        p.insert("Connected".into(), ov(true));
        p.insert("UUIDs".into(), ov(vec!["0000110b-0000-1000-8000-00805f9b34fb"]));
        assert_eq!(device_info(&path, &p), None);
        p.insert("UUIDs".into(), ov(vec!["0000110b-0000-1000-8000-00805f9b34fb", MAESTRO_UUID]));
        let d = device_info(&path, &p).unwrap();
        assert_eq!(d.adapter.as_str(), "/org/bluez/hci0");
        assert!(d.connected);
        assert_eq!(d.alias, "Pixel Buds Pro");
    }

    #[test]
    fn transport_parse() {
        let path = OwnedObjectPath::try_from("/org/bluez/hci0/dev_AA/sep1/fd0").unwrap();
        let mut p: Props = HashMap::new();
        p.insert("Device".into(), ov(ObjectPath::try_from("/org/bluez/hci0/dev_AA").unwrap()));
        p.insert("UUID".into(), ov(A2DP_SOURCE_UUID));
        p.insert("Codec".into(), ov(0xffu8));
        p.insert("Configuration".into(), ov(vec![0xe0u8, 0, 0, 0, 1, 0, 0x94]));
        p.insert("State".into(), ov("idle"));
        let t = transport_info(&path, &p).unwrap();
        assert_eq!(t.configuration, vec![0xe0, 0, 0, 0, 1, 0, 0x94]);
        assert_eq!(t.codec, 0xff);
    }
}
