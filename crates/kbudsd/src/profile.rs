// SPDX-License-Identifier: GPL-3.0-or-later
//! External BlueZ profile for the Maestro UUID (client role).
//!
//! `Device1.ConnectProfile(MAESTRO_UUID)` makes BlueZ resolve the RFCOMM channel by SDP, connect,
//! and hand us the socket through `Profile1.NewConnection`. This avoids our own SDP client.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};

use kbuds_protocol::MAESTRO_UUID;
use tokio::sync::mpsc;
use zbus::interface;
use zbus::zvariant::{self, OwnedObjectPath, OwnedValue, Value};

use crate::bluez::ProfileManager1Proxy;

pub const PROFILE_PATH: &str = "/io/github/kbuds/profile";

#[derive(Debug)]
pub enum ProfileEvent {
    /// BlueZ connected the Maestro channel; the RFCOMM socket.
    Connected(OwnedFd),
    /// BlueZ asks us to drop the connection.
    DisconnectRequested,
}

/// Routes `NewConnection` fds to the device supervisors.
#[derive(Clone, Default)]
pub struct Router {
    routes: Arc<Mutex<HashMap<OwnedObjectPath, mpsc::UnboundedSender<ProfileEvent>>>>,
}

impl Router {
    pub fn register(&self, device: OwnedObjectPath) -> mpsc::UnboundedReceiver<ProfileEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.routes.lock().unwrap().insert(device, tx);
        rx
    }

    pub fn unregister(&self, device: &OwnedObjectPath) {
        self.routes.lock().unwrap().remove(device);
    }

    fn deliver(&self, device: &OwnedObjectPath, ev: ProfileEvent) -> bool {
        match self.routes.lock().unwrap().get(device) {
            Some(tx) => tx.send(ev).is_ok(),
            None => false,
        }
    }
}

/// `org.bluez.Profile1` served on the system bus.
pub struct Profile {
    pub router: Router,
}

#[interface(name = "org.bluez.Profile1")]
impl Profile {
    fn release(&self) {
        tracing::info!("BlueZ released the Maestro profile");
    }

    fn new_connection(
        &self,
        device: OwnedObjectPath,
        fd: zvariant::OwnedFd,
        _properties: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<()> {
        tracing::debug!("NewConnection from {device}");
        let fd: OwnedFd = fd.into();
        if self.router.deliver(&device, ProfileEvent::Connected(fd)) {
            Ok(())
        } else {
            Err(zbus::fdo::Error::Failed("no supervisor for this device".into()))
        }
    }

    fn request_disconnection(&self, device: OwnedObjectPath) {
        tracing::debug!("RequestDisconnection for {device}");
        self.router.deliver(&device, ProfileEvent::DisconnectRequested);
    }
}

/// Registers the profile with BlueZ (again after a BlueZ restart).
pub async fn register(system: &zbus::Connection) -> zbus::Result<()> {
    let pm = ProfileManager1Proxy::new(system).await?;
    let mut opts: HashMap<&str, Value<'_>> = HashMap::new();
    opts.insert("Name", Value::from("kbuds Maestro"));
    opts.insert("Role", Value::from("client"));
    opts.insert("AutoConnect", Value::from(false));
    // BT_SECURITY_MEDIUM: the buds are paired.
    opts.insert("RequireAuthentication", Value::from(true));
    opts.insert("RequireAuthorization", Value::from(false));
    let path = zvariant::ObjectPath::try_from(PROFILE_PATH).unwrap();
    match pm.register_profile(&path, MAESTRO_UUID, opts).await {
        Ok(()) => Ok(()),
        Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "org.bluez.Error.AlreadyExists" => Ok(()),
        Err(e) => Err(e),
    }
}
