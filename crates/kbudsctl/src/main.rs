// SPDX-License-Identifier: GPL-3.0-or-later
//! kbudsctl: command-line client of the kbuds daemon (`io.github.kbuds.Daemon` on the session bus).

mod dbus;

use std::collections::HashMap;
use std::io::Write;
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use futures_util::StreamExt;
use serde_json::{Map, Value as Json, json};
use zbus::fdo::PropertiesProxy;
use zbus::names::InterfaceName;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::dbus::device1::Device1Proxy;
use crate::dbus::manager1::Manager1Proxy;

const SERVICE: &str = "io.github.kbuds.Daemon";
const DEVICE_IFACE: &str = "io.github.kbuds.Device1";

/// Control Google Pixel Buds through the kbuds daemon.
#[derive(Parser)]
#[command(name = "kbudsctl", version, about, after_help = "Exit status: 0 ok, 1 failure, 2 not connected, 3 not supported by the buds.")]
struct Cli {
    /// Device to use: Bluetooth address or name (default: the connected one).
    #[arg(short, long, global = true)]
    device: Option<String>,
    /// Machine-readable output.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// List known Pixel Buds.
    Devices,
    /// Show everything about a device (default command).
    Status,
    /// Show battery levels.
    Battery,
    /// Read a setting.
    Get {
        #[arg(value_enum)]
        setting: GetSetting,
    },
    /// Change a setting.
    Set {
        #[command(subcommand)]
        setting: SetSetting,
    },
    /// "PC only": take exclusive use of the buds (the other host is disconnected), or release it.
    Exclusive {
        #[arg(value_enum)]
        state: OnOff,
    },
    /// Show the audio stack state, or fix it.
    Audio {
        #[command(subcommand)]
        action: Option<AudioCmd>,
    },
    /// Print changes as they happen (Ctrl+C to stop).
    Monitor,
    /// Print a shell completion script.
    #[command(hide = true)]
    Completions { shell: clap_complete::Shell },
    /// Print the man page (roff).
    #[command(hide = true)]
    Man,
}

#[derive(Clone, Copy, ValueEnum)]
enum GetSetting {
    Anc,
    Multipoint,
    Exclusive,
}

#[derive(Subcommand)]
enum SetSetting {
    /// Noise control.
    Anc {
        #[arg(value_enum)]
        mode: AncMode,
    },
    /// Connect to two hosts at once.
    Multipoint {
        #[arg(value_enum)]
        state: OnOff,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum AncMode {
    Off,
    Active,
    Aware,
    Adaptive,
}

impl AncMode {
    fn as_str(self) -> &'static str {
        match self {
            AncMode::Off => "off",
            AncMode::Active => "active",
            AncMode::Aware => "aware",
            AncMode::Adaptive => "adaptive",
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum OnOff {
    On,
    Off,
}

impl OnOff {
    fn bool(self) -> bool {
        matches!(self, OnOff::On)
    }
}

#[derive(Subcommand)]
enum AudioCmd {
    /// Reconnect A2DP from this PC so the stream is configured as stereo (fixes Dual Mono / HFP fallback).
    Fix,
    /// Run the fix automatically when a Dual Mono stream is detected.
    Auto {
        #[arg(value_enum)]
        state: OnOff,
    },
}

/// A failure with its exit status.
struct Failure {
    code: u8,
    msg: String,
}

impl Failure {
    fn new(code: u8, msg: impl Into<String>) -> Self {
        Self { code, msg: msg.into() }
    }
}

impl From<zbus::Error> for Failure {
    fn from(e: zbus::Error) -> Self {
        match &e {
            zbus::Error::MethodError(name, msg, _) => {
                let msg = msg.clone().unwrap_or_else(|| name.to_string());
                let code = match name.as_str() {
                    "org.freedesktop.DBus.Error.ServiceUnknown" | "org.freedesktop.DBus.Error.NameHasNoOwner" => {
                        return Failure::new(2, "kbudsd is not running (systemctl --user start kbudsd)");
                    }
                    "io.github.kbuds.Error.NotConnected" => 2,
                    "io.github.kbuds.Error.NotSupported" => 3,
                    _ => 1,
                };
                Failure::new(code, msg)
            }
            zbus::Error::FDO(fdo) => match fdo.as_ref() {
                zbus::fdo::Error::ServiceUnknown(_) | zbus::fdo::Error::NameHasNoOwner(_) => {
                    Failure::new(2, "kbudsd is not running (systemctl --user start kbudsd)")
                }
                other => Failure::new(1, other.to_string()),
            },
            _ => Failure::new(1, e.to_string()),
        }
    }
}

type Result<T> = std::result::Result<T, Failure>;

/// All Device1 properties of one device.
struct Props(HashMap<String, OwnedValue>);

impl Props {
    fn s(&self, k: &str) -> String {
        self.0.get(k).and_then(|v| <&str>::try_from(&**v).ok()).unwrap_or("").to_string()
    }
    fn b(&self, k: &str) -> bool {
        self.0.get(k).and_then(|v| bool::try_from(&**v).ok()).unwrap_or(false)
    }
    fn i(&self, k: &str) -> i64 {
        self.0
            .get(k)
            .and_then(|v| i32::try_from(&**v).map(i64::from).or_else(|_| u32::try_from(&**v).map(i64::from)).ok())
            .unwrap_or(-1)
    }
    fn list(&self, k: &str) -> Vec<String> {
        self.0.get(k).and_then(|v| v.try_clone().ok()).and_then(|v| Vec::<String>::try_from(v).ok()).unwrap_or_default()
    }
    fn json(&self) -> Json {
        let mut keys: Vec<_> = self.0.keys().collect();
        keys.sort();
        let mut m = Map::new();
        for k in keys {
            m.insert(k.clone(), to_json(&self.0[k]));
        }
        Json::Object(m)
    }
}

fn to_json(v: &Value<'_>) -> Json {
    match v {
        Value::Bool(b) => json!(b),
        Value::I32(i) => json!(i),
        Value::U32(i) => json!(i),
        Value::U8(i) => json!(i),
        Value::Str(s) => json!(s.as_str()),
        Value::ObjectPath(p) => json!(p.as_str()),
        Value::Array(a) => Json::Array(a.iter().map(to_json).collect()),
        Value::Value(v) => to_json(v),
        other => json!(other.to_string()),
    }
}

struct Ctx {
    conn: zbus::Connection,
}

impl Ctx {
    async fn device_paths(&self) -> Result<Vec<OwnedObjectPath>> {
        Ok(Manager1Proxy::new(&self.conn).await?.devices().await?)
    }

    async fn props(&self, path: &OwnedObjectPath) -> Result<Props> {
        let p = PropertiesProxy::builder(&self.conn).destination(SERVICE)?.path(path.clone())?.build().await?;
        let all = p.get_all(InterfaceName::from_static_str_unchecked(DEVICE_IFACE)).await.map_err(zbus::Error::from)?;
        Ok(Props(all))
    }

    /// The device named by `--device`, else the one with a session, else the only one.
    async fn pick(&self, wanted: Option<&str>) -> Result<(OwnedObjectPath, Props)> {
        let mut all = Vec::new();
        for p in self.device_paths().await? {
            let props = self.props(&p).await?;
            all.push((p, props));
        }
        if all.is_empty() {
            return Err(Failure::new(2, "no Pixel Buds known to the daemon (pair them first)"));
        }
        if let Some(w) = wanted {
            return all
                .into_iter()
                .find(|(_, p)| p.s("Address").eq_ignore_ascii_case(w) || p.s("Name").eq_ignore_ascii_case(w))
                .ok_or_else(|| Failure::new(1, format!("no device {w:?}")));
        }
        let rank = |p: &Props| match (p.s("State").as_str(), p.b("BluetoothConnected")) {
            ("connected", _) => 0,
            (_, true) => 1,
            _ => 2,
        };
        all.sort_by_key(|(_, p)| rank(p));
        if all.len() > 1 && rank(&all[0].1) == rank(&all[1].1) {
            let names: Vec<_> = all.iter().map(|(_, p)| format!("{} ({})", p.s("Name"), p.s("Address"))).collect();
            return Err(Failure::new(1, format!("several devices, choose one with --device: {}", names.join(", "))));
        }
        Ok(all.swap_remove(0))
    }

    async fn device(&self, path: &OwnedObjectPath) -> Result<Device1Proxy<'static>> {
        Ok(Device1Proxy::builder(&self.conn).destination(SERVICE)?.path(path.clone())?.build().await?)
    }
}

fn level(l: i64) -> String {
    if l < 0 { "–".into() } else { format!("{l}%") }
}

fn battery_line(p: &Props) -> String {
    let one = |name: &str, lvl: &str, chg: &str| {
        let mut s = format!("{name} {}", level(p.i(lvl)));
        if p.b(chg) && p.i(lvl) >= 0 {
            s.push_str(" (charging)");
        }
        s
    };
    [one("L", "BatteryLeft", "ChargingLeft"), one("R", "BatteryRight", "ChargingRight"), one("Case", "BatteryCase", "ChargingCase")]
        .join("   ")
}

fn or_dash(s: String) -> String {
    if s.is_empty() { "–".into() } else { s }
}

fn print_status(p: &Props) {
    let model = match p.s("Model").as_str() {
        "pro" => "Pixel Buds Pro",
        "pro2" => "Pixel Buds Pro 2",
        "2a" => "Pixel Buds 2a",
        "unsupported" => "unsupported model",
        _ => "",
    };
    println!("{} ({}){}", p.s("Name"), p.s("Address"), if model.is_empty() { String::new() } else { format!(", {model}") });
    let state = p.s("State");
    let conn = if state == "connected" {
        format!("connected (host {}, primary bud {})", p.s("HostId"), p.s("Primary"))
    } else if p.b("BluetoothConnected") {
        format!("{state} (Bluetooth connected)")
    } else {
        "Bluetooth disconnected".into()
    };
    println!("  Connection   {conn}");
    if state != "connected" {
        return;
    }
    println!("  Battery      {}", battery_line(p));
    let place = |s: String| match s.as_str() {
        "in_case" => "in case".to_string(),
        "out" => "out".to_string(),
        _ => "not connected".to_string(),
    };
    println!("  Placement    L {}   R {}", place(p.s("PlacementLeft")), place(p.s("PlacementRight")));
    let modes = p.list("AncModes");
    if !modes.is_empty() {
        println!("  Noise ctrl   {}   (modes: {})", or_dash(p.s("Anc")), modes.join(", "));
    }
    if p.b("MultipointSupported") {
        println!("  Multipoint   {}", if p.b("Multipoint") { "on" } else { "off" });
    }
    println!(
        "  Audio owner  {}{}",
        if p.b("OtherHostHasAudio") { "another host" } else { "this PC (or nobody)" },
        if p.b("Exclusive") { ", PC only (exclusive)" } else { "" }
    );
    let mode = p.s("A2dpChannelMode");
    let problem = p.s("AudioProblem");
    println!(
        "  Audio        {}{}{}",
        p.s("AudioProfile"),
        if mode.is_empty() { String::new() } else { format!(", Opus {}", mode.replace('_', " ")) },
        match problem.as_str() {
            "dual_mono" => "   ! buds-configured Dual Mono: the PC cannot take the audio back (kbudsctl audio fix)",
            "hfp_fallback" => "   ! headset (call) profile: if no call is active, run kbudsctl audio fix",
            _ => "",
        }
    );
    println!(
        "  Firmware     L {}   R {}   Case {}",
        or_dash(p.s("FirmwareLeft")),
        or_dash(p.s("FirmwareRight")),
        or_dash(p.s("FirmwareCase"))
    );
}

async fn run(cli: Cli) -> Result<()> {
    let cmd = cli.cmd.unwrap_or(Cmd::Status);
    match &cmd {
        Cmd::Completions { shell } => {
            clap_complete::generate(*shell, &mut Cli::command(), "kbudsctl", &mut std::io::stdout());
            return Ok(());
        }
        Cmd::Man => {
            clap_mangen::Man::new(Cli::command()).render(&mut std::io::stdout()).map_err(|e| Failure::new(1, e.to_string()))?;
            return Ok(());
        }
        _ => {}
    }
    let conn = zbus::Connection::session().await.map_err(|e| Failure::new(2, format!("session bus: {e}")))?;
    let ctx = Ctx { conn };
    let dev = cli.device.as_deref();
    match cmd {
        Cmd::Devices => {
            let mut out = Vec::new();
            for p in ctx.device_paths().await? {
                out.push(ctx.props(&p).await?);
            }
            if cli.json {
                println!("{}", Json::Array(out.iter().map(Props::json).collect()));
            } else if out.is_empty() {
                println!("no Pixel Buds known (pair them first)");
            } else {
                for p in out {
                    let bt = if p.b("BluetoothConnected") { "Bluetooth connected" } else { "Bluetooth disconnected" };
                    println!("{}  {:<24} {:<13} {bt}", p.s("Address"), p.s("Name"), p.s("State"));
                }
            }
        }
        Cmd::Status => {
            let (_, p) = ctx.pick(dev).await?;
            if cli.json { println!("{}", p.json()) } else { print_status(&p) }
        }
        Cmd::Battery => {
            let (_, p) = ctx.pick(dev).await?;
            if p.s("State") != "connected" {
                return Err(Failure::new(2, format!("{}: not connected", p.s("Name"))));
            }
            if cli.json {
                let j = |l: &str, c: &str| json!({ "level": (p.i(l) >= 0).then(|| p.i(l)), "charging": p.b(c) });
                println!(
                    "{}",
                    json!({ "left": j("BatteryLeft", "ChargingLeft"), "right": j("BatteryRight", "ChargingRight"),
                            "case": j("BatteryCase", "ChargingCase") })
                );
            } else {
                println!("{}", battery_line(&p));
            }
        }
        Cmd::Get { setting } => {
            let (_, p) = ctx.pick(dev).await?;
            if p.s("State") != "connected" {
                return Err(Failure::new(2, format!("{}: not connected", p.s("Name"))));
            }
            let (key, val) = match setting {
                GetSetting::Anc => ("anc", json!(or_dash(p.s("Anc")))),
                GetSetting::Multipoint => ("multipoint", json!(if p.b("Multipoint") { "on" } else { "off" })),
                GetSetting::Exclusive => ("exclusive", json!(if p.b("Exclusive") { "on" } else { "off" })),
            };
            if cli.json { println!("{}", json!({ key: val })) } else { println!("{}", val.as_str().unwrap_or("")) }
        }
        Cmd::Set { setting } => {
            let (path, p) = ctx.pick(dev).await?;
            let d = ctx.device(&path).await?;
            match setting {
                SetSetting::Anc { mode } => {
                    let one_out = (p.s("PlacementLeft") == "out") != (p.s("PlacementRight") == "out");
                    if one_out && matches!(mode, AncMode::Active | AncMode::Adaptive) {
                        eprintln!("note: with only one bud out the buds switch to aware on their own");
                    }
                    d.set_anc(mode.as_str()).await?;
                    if !cli.json {
                        println!("noise control: {} -> {} (confirmed by read-back)", or_dash(p.s("Anc")), mode.as_str());
                    }
                }
                SetSetting::Multipoint { state } => {
                    d.set_multipoint(state.bool()).await?;
                    if !cli.json {
                        println!("multipoint: {}", if state.bool() { "on" } else { "off" });
                    }
                }
            }
        }
        Cmd::Exclusive { state } => {
            let (path, _) = ctx.pick(dev).await?;
            ctx.device(&path).await?.set_exclusive(state.bool()).await?;
            if !cli.json {
                println!(
                    "{}",
                    if state.bool() {
                        "PC only: on (the other host is disconnected until you turn it off or the daemon stops)"
                    } else {
                        "PC only: off"
                    }
                );
            }
        }
        Cmd::Audio { action } => {
            let (path, p) = ctx.pick(dev).await?;
            let d = ctx.device(&path).await?;
            match action {
                None => {
                    if cli.json {
                        println!(
                            "{}",
                            json!({ "profile": p.s("AudioProfile"), "channel_mode": p.s("A2dpChannelMode"),
                                    "problem": p.s("AudioProblem"), "auto_fix": p.b("AutoFixAudio") })
                        );
                    } else {
                        println!("profile:      {}", p.s("AudioProfile"));
                        println!("channel mode: {}", or_dash(p.s("A2dpChannelMode")));
                        println!("problem:      {}", if p.s("AudioProblem").is_empty() { "none".into() } else { p.s("AudioProblem") });
                        println!("auto fix:     {}", if p.b("AutoFixAudio") { "on" } else { "off" });
                    }
                }
                Some(AudioCmd::Fix) => {
                    d.fix_audio().await?;
                    if !cli.json {
                        println!("A2DP reconnected");
                    }
                }
                Some(AudioCmd::Auto { state }) => {
                    d.set_auto_fix_audio(state.bool()).await?;
                }
            }
        }
        Cmd::Monitor => monitor(&ctx, dev, cli.json).await?,
        Cmd::Completions { .. } | Cmd::Man => unreachable!(),
    }
    Ok(())
}

async fn monitor(ctx: &Ctx, wanted: Option<&str>, json_out: bool) -> Result<()> {
    let mut paths = Vec::new();
    for p in ctx.device_paths().await? {
        let props = ctx.props(&p).await?;
        if wanted.is_none_or(|w| props.s("Address").eq_ignore_ascii_case(w) || props.s("Name").eq_ignore_ascii_case(w)) {
            paths.push((p, props.s("Address")));
        }
    }
    if paths.is_empty() {
        return Err(Failure::new(2, "no matching device"));
    }
    let mut streams = Vec::new();
    for (path, addr) in paths {
        let proxy = PropertiesProxy::builder(&ctx.conn).destination(SERVICE)?.path(path)?.build().await?;
        let s = proxy.receive_properties_changed().await?;
        streams.push(s.map(move |sig| (addr.clone(), sig)));
    }
    let mut all = futures_util::stream::select_all(streams);
    eprintln!("monitoring, Ctrl+C to stop");
    while let Some((addr, sig)) = all.next().await {
        let Ok(args) = sig.args() else { continue };
        let mut changed: Vec<_> = args.changed_properties().iter().collect();
        changed.sort_by_key(|(k, _)| **k);
        for (k, v) in changed {
            if json_out {
                println!("{}", json!({ "device": addr, "property": k, "value": to_json(v) }));
            } else {
                println!("{addr} {k} = {}", to_json(v));
            }
        }
        let _ = std::io::stdout().flush();
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            eprintln!("kbudsctl: {}", f.msg);
            ExitCode::from(f.code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_is_valid() {
        Cli::command().debug_assert();
    }
}
