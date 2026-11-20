// SPDX-License-Identifier: GPL-3.0-or-later
//! Battery to the desktop: an `org.bluez.BatteryProvider1` per device, registered with BlueZ's
//! `BatteryProviderManager1`, so UPower and the panels show it. One percentage per device: the
//! lower of the two buds, as the buds' own HFP `+BIEV` does.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use zbus::fdo::ObjectManager;
use zbus::interface;
use zbus::zvariant::{ObjectPath, OwnedObjectPath};

use crate::bluez::BatteryProviderManager1Proxy;

pub const ROOT: &str = "/io/github/kbuds/battery";

pub struct Provider {
    device: OwnedObjectPath,
    percentage: u8,
}

#[interface(name = "org.bluez.BatteryProvider1")]
impl Provider {
    #[zbus(property)]
    fn device(&self) -> OwnedObjectPath {
        self.device.clone()
    }
    #[zbus(property)]
    fn percentage(&self) -> u8 {
        self.percentage
    }
    #[zbus(property)]
    fn source(&self) -> String {
        "kbuds".into()
    }
}

/// Battery providers on the system bus.
#[derive(Clone)]
pub struct Batteries {
    conn: zbus::Connection,
    registered: Arc<Mutex<HashSet<OwnedObjectPath>>>,
    unsupported_logged: Arc<Mutex<bool>>,
}

fn child_path(device: &OwnedObjectPath) -> OwnedObjectPath {
    let leaf = device.as_str().rsplit('/').next().unwrap_or("dev");
    OwnedObjectPath::try_from(format!("{ROOT}/{leaf}")).expect("valid path")
}

impl Batteries {
    pub async fn new(conn: zbus::Connection) -> zbus::Result<Self> {
        conn.object_server().at(ROOT, ObjectManager).await?;
        Ok(Self { conn, registered: Default::default(), unsupported_logged: Default::default() })
    }

    /// Registers our root with the adapter's manager once (again after a BlueZ restart).
    pub async fn ensure_registered(&self, adapter: &OwnedObjectPath) {
        if self.registered.lock().unwrap().contains(adapter) {
            return;
        }
        let r = async {
            let m = BatteryProviderManager1Proxy::builder(&self.conn).path(adapter.clone())?.build().await?;
            m.register_battery_provider(&ObjectPath::try_from(ROOT).unwrap()).await
        }
        .await;
        match r {
            Ok(()) => {
                tracing::info!("battery provider registered on {adapter}");
                self.registered.lock().unwrap().insert(adapter.clone());
            }
            Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "org.bluez.Error.AlreadyExists" => {
                self.registered.lock().unwrap().insert(adapter.clone());
            }
            Err(e) => {
                let mut logged = self.unsupported_logged.lock().unwrap();
                if !*logged {
                    *logged = true;
                    tracing::warn!(
                        "cannot register battery provider on {adapter} ({e}); desktop battery display \
                         needs BlueZ's BatteryProviderManager1 (bluetoothd --experimental on older BlueZ)"
                    );
                }
            }
        }
    }

    /// BlueZ went away: registrations are gone.
    pub fn forget_registrations(&self) {
        self.registered.lock().unwrap().clear();
    }

    /// Publishes `percentage` for `device`, or removes it with `None`.
    pub async fn update(&self, device: &OwnedObjectPath, percentage: Option<u8>) {
        let path = child_path(device);
        let server = self.conn.object_server();
        match percentage {
            None => {
                let _ = server.remove::<Provider, _>(&path).await;
            }
            Some(p) => match server.interface::<_, Provider>(&path).await {
                Ok(iface) => {
                    let mut i = iface.get_mut().await;
                    if i.percentage != p {
                        i.percentage = p;
                        let _ = i.percentage_changed(iface.signal_emitter()).await;
                    }
                }
                Err(_) => {
                    let _ = server.at(&path, Provider { device: device.clone(), percentage: p }).await;
                }
            },
        }
    }
}
