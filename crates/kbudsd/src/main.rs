// SPDX-License-Identifier: GPL-3.0-or-later
//! kbudsd: per-user Pixel Buds daemon (`systemd --user`).
//!
//! Owns the Maestro RFCOMM channel of every paired Pixel Buds (Pigweed models) and exposes them on
//! the session bus as `io.github.kbuds.Daemon` (`dbus/*.xml`).

mod api;
mod audio;
mod battery;
mod bluez;
mod config;
mod device;
mod fdstream;
mod framelog;
mod profile;
mod session;
mod state;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use anyhow::{Context, bail};
use clap::Parser;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedObjectPath;

use crate::device::{Ctx, DeviceHandle, Input};

/// Pixel Buds daemon: exposes battery, noise control, multipoint and audio state on D-Bus.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Append raw Maestro traffic to FILE, one `<timestamp> TX|RX <hex>` line per frame sent or
    /// chunk received.
    #[arg(long, value_name = "FILE")]
    frame_log: Option<PathBuf>,
    /// More logging (-v debug, -vv trace). RUST_LOG overrides.
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

fn init_logging(verbose: u8) {
    use tracing_subscriber::EnvFilter;
    let default = match verbose {
        0 => "kbudsd=info,kbuds_protocol=info",
        1 => "kbudsd=debug,kbuds_protocol=debug",
        _ => "kbudsd=trace,kbuds_protocol=trace",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr);
    // Under systemd, journald adds timestamps itself.
    if std::env::var_os("JOURNAL_STREAM").is_some() {
        builder.without_time().with_target(false).with_ansi(false).init();
    } else {
        builder.init();
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    init_logging(args.verbose);
    match run(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> anyhow::Result<()> {
    let config = Arc::new(Mutex::new(config::Config::load()));
    let frame_log = match &args.frame_log {
        Some(p) => Some(framelog::open(p).with_context(|| format!("opening {}", p.display()))?),
        None => None,
    };

    let system = zbus::Connection::system().await.context("connecting to the system bus")?;
    let router = profile::Router::default();
    system.object_server().at(profile::PROFILE_PATH, profile::Profile { router: router.clone() }).await?;
    let batteries = battery::Batteries::new(system.clone()).await?;

    let session_bus = zbus::Connection::session().await.context("connecting to the session bus")?;
    let device_list: Arc<Mutex<Vec<OwnedObjectPath>>> = Default::default();
    session_bus.object_server().at(api::MANAGER_PATH, api::Manager { devices: device_list.clone() }).await?;
    match session_bus.request_name_with_flags(api::BUS_NAME, RequestNameFlags::DoNotQueue.into()).await {
        Ok(RequestNameReply::PrimaryOwner) => {}
        Ok(_) | Err(zbus::Error::NameTaken) => bail!("{} is already owned: is kbudsd already running?", api::BUS_NAME),
        Err(e) => return Err(e).context("requesting the bus name"),
    }
    tracing::info!("kbudsd {} running as {}", env!("CARGO_PKG_VERSION"), api::BUS_NAME);

    let ctx = Arc::new(Ctx { system: system.clone(), session_bus: session_bus.clone(), router, batteries, frame_log, config });
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut monitor = tokio::spawn(bluez::monitor(system.clone(), tx));
    let mut devices: HashMap<OwnedObjectPath, DeviceHandle> = HashMap::new();
    let mut transport_owner: HashMap<OwnedObjectPath, OwnedObjectPath> = HashMap::new();
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;

    let manager_changed = async |added: Option<&OwnedObjectPath>, removed: Option<&OwnedObjectPath>| {
        let Ok(emitter) = SignalEmitter::new(&session_bus, api::MANAGER_PATH) else { return };
        if let Some(p) = added {
            let _ = api::Manager::device_added(&emitter, p.as_ref()).await;
        }
        if let Some(p) = removed {
            let _ = api::Manager::device_removed(&emitter, p.as_ref()).await;
        }
        if let Ok(iface) = session_bus.object_server().interface::<_, api::Manager>(api::MANAGER_PATH).await {
            let _ = iface.get().await.devices_changed(iface.signal_emitter()).await;
        }
    };

    loop {
        tokio::select! {
            ev = rx.recv() => {
                let Some(ev) = ev else { break };
                match ev {
                    bluez::Event::ServiceUp => {
                        ctx.batteries.forget_registrations();
                        // The monitor re-sends every device right after this event.
                        if let Err(e) = profile::register(&system).await {
                            tracing::error!("cannot register the Maestro profile with BlueZ: {e}");
                        }
                    }
                    bluez::Event::ServiceDown => ctx.batteries.forget_registrations(),
                    bluez::Event::Device(info) => {
                        ctx.batteries.ensure_registered(&info.adapter).await;
                        if let Some(d) = devices.get(&info.path) {
                            let _ = d.input.send(Input::Info(info));
                        } else {
                            tracing::info!("found {} ({})", info.alias, info.address);
                            match device::spawn(ctx.clone(), info.clone()).await {
                                Ok(d) => {
                                    let path = d.dbus_path.clone();
                                    device_list.lock().unwrap().push(path.clone());
                                    devices.insert(info.path.clone(), d);
                                    manager_changed(Some(&path), None).await;
                                }
                                Err(e) => tracing::error!("cannot publish {}: {e}", info.address),
                            }
                        }
                    }
                    bluez::Event::DeviceRemoved(path) => {
                        if let Some(d) = devices.remove(&path) {
                            let dbus_path = d.dbus_path.clone();
                            d.stop(&ctx).await;
                            device_list.lock().unwrap().retain(|p| p != &dbus_path);
                            manager_changed(None, Some(&dbus_path)).await;
                        }
                    }
                    bluez::Event::Transport(t) => {
                        transport_owner.insert(t.path.clone(), t.device.clone());
                        if let Some(d) = devices.get(&t.device) {
                            let _ = d.input.send(Input::Transport(t));
                        }
                    }
                    bluez::Event::TransportRemoved(path) => {
                        if let Some(dev) = transport_owner.remove(&path)
                            && let Some(d) = devices.get(&dev)
                        {
                            let _ = d.input.send(Input::TransportRemoved(path));
                        }
                    }
                }
            }
            r = &mut monitor => {
                match r {
                    Ok(Ok(())) => bail!("BlueZ monitor stopped"),
                    Ok(Err(e)) => bail!("BlueZ monitor failed: {e}"),
                    Err(e) => bail!("BlueZ monitor crashed: {e}"),
                }
            }
            _ = sigterm.recv() => break,
            _ = sigint.recv() => break,
        }
    }

    tracing::info!("shutting down");
    for (_, d) in devices.drain() {
        d.stop(&ctx).await;
    }
    Ok(())
}
