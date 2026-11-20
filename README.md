# kbuds: Google Pixel Buds on Linux

Battery, noise control, multipoint and audio-ownership control for **Pixel Buds Pro, Pro 2 and 2a**
(the "Pigweed" models; tested on Pixel Buds Pro firmware 575). It talks to the buds over their
proprietary Maestro RFCOMM channel.

| Component | What |
|---|---|
| `crates/kbuds-protocol` | Protocol library: pw_hdlc framing, pw_rpc client engine (sans-IO), `maestro_pw` protobufs, settings, model gating, tokio client |
| `crates/kbudsd` | Per-user daemon (`systemd --user`): owns the Maestro channel, D-Bus API `io.github.kbuds.Daemon`, BlueZ battery provider, audio-stack checks |
| `crates/kbudsctl` | Command-line client |
| `gui/` | Qt 6 / QML settings app (`kbuds`), Material style |
| `dbus/` | The D-Bus API, as introspection XML: the single source of truth for the daemon (conformance test), the CLI (proxies generated in `build.rs`) and the GUI (`qdbusxml2cpp`) |
| `data/` | systemd user unit, D-Bus activation file, WirePlumber rule |

## Build

```
cargo build --release            # needs protoc (protobuf-compiler)
cargo test --workspace
cmake -B build-gui gui && cmake --build build-gui   # needs qt6-base-dev, qt6-declarative-dev
```

Run the daemon from the tree: `target/release/kbudsd -v` (add `--frame-log maestro.log` to record
the raw traffic, one `<timestamp> TX|RX <hex>` line per frame sent or chunk received). The buds
accept only one Maestro client: stop `kbudsd` before using any other tool that talks to them.

## Package

```
sudo apt build-dep .    # or install the Build-Depends of debian/control
scripts/build-deb.sh    # vendors the crates, then dpkg-buildpackage -b -us -uc
sudo apt install ../kbuds_0.1.0_amd64.deb
```

Installing starts `kbudsd` in every running user session (upgrades restart it), and restarts
WirePlumber when the 1 s suspend rule is new or changed; reconnect the buds once for the rule to
apply to them. Removing the package stops the daemon and drops the rule. The daemon does not run
for system users such as the login-screen greeter.

## CLI

```
kbudsctl                      # status of the connected buds
kbudsctl battery
kbudsctl set anc aware        # off | active | aware | adaptive
kbudsctl set multipoint on
kbudsctl exclusive on         # "PC only": the other host is disconnected until off
kbudsctl audio fix            # reconnect A2DP if the buds chose Opus Dual Mono or fell back to HFP
kbudsctl monitor
```

## Notes

- The battery shows up in UPower/desktop panels through BlueZ's `BatteryProviderManager1`. Older
  BlueZ versions need `bluetoothd --experimental` for it.
- The WirePlumber rule matches nodes whose description contains "Pixel Buds". If you renamed the
  buds, copy `/usr/share/wireplumber/wireplumber.conf.d/51-kbuds-pixelbuds.conf` to
  `~/.config/wireplumber/wireplumber.conf.d/` and adjust the match.

## Licence

GPL-3.0-or-later.

kbuds is not affiliated with or endorsed by Google. Pixel Buds is a trademark of Google LLC.
