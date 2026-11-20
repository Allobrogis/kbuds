# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

kbuds brings Google Pixel Buds Pro / Pro 2 / 2a (the "Pigweed" models) to Linux: battery, ANC, multipoint and
"PC only" (exclusive) audio, over the buds' proprietary Maestro RFCOMM channel. See `README.md` for user-facing usage.

## Commands

```
cargo build --release                      # needs protoc (protobuf-compiler)
cargo test --workspace
cargo test -p kbuds-protocol --test client # one integration test file
cargo test -p kbudsd session::             # tests matching a path/name filter
cmake -B build-gui gui && cmake --build build-gui   # GUI; needs qt6-base-dev, qt6-declarative-dev
cmake --build build-gui --target all_qmllint        # QML lint (keep at 0 warnings)
target/release/kbudsd -v                   # run the daemon from the tree (-vv trace; RUST_LOG overrides)
target/release/kbudsd -v --frame-log maestro.log    # raw traffic: one `<ts> TX|RX <hex>` line per frame sent / chunk read
scripts/build-deb.sh                       # vendor crates + dpkg-buildpackage -b -us -uc -> ../kbuds_*.deb
```

- `crates/kbuds-protocol/tests/captures.rs` replays frame logs from `$KBUDS_CAPTURES` (default `../captures`)
  and silently skips when absent. Captures contain device serials and addresses: never commit them.
- MSRV is 1.93 (Ubuntu 26.04's rustc, used by `debian/rules` which forces `/usr/bin` first on PATH). Don't use
  newer language/std features even if the local rustup toolchain accepts them.
- After `scripts/vendor.sh` (or `build-deb.sh`) runs, `.cargo/config.toml` redirects crates.io to `vendor/`; when
  adding or bumping a dependency, rerun `scripts/vendor.sh` (it also regenerates `debian/copyright`) or move that
  config aside.
- The daemon needs the real system bus (BlueZ) and session bus; end-to-end and hardware testing can't happen in a
  sandbox without bus access.

## CI and releases (`.gitlab-ci.yml`)

Every push to `main` builds and tests the .deb on `ubuntu:26.04` (job `deb`, artifact kept 1 week). To release:
bump `version` in `Cargo.toml` and add a `debian/changelog` entry, commit, then `git tag -s vX.Y.Z -m "kbuds X.Y.Z"`
and push the tag (`vX.Y.Z-rc1` ↔ changelog `X.Y.Z~rc1`). The tag pipeline fails if the tag, changelog and
`Cargo.toml` disagree; otherwise it uploads the .deb to the generic package registry and creates the GitLab release
linking to it, with the changelog entry as notes.

## Architecture

Hard rule: **Google RPCs live only in the daemon; standard Bluetooth profiles live only in
BlueZ/PipeWire.** The GUI and CLI never touch Bluetooth, they only talk to `kbudsd` over the session bus.

### `crates/kbuds-protocol` (library)

Layered bottom-up: `hdlc` (pw_hdlc framing, CRC) → `rpc` (pw_rpc packets/status) + `ids` (service/method hashes)
→ `address` (endpoints per bud, host id `MAESTRO_A`/`MAESTRO_B`) → `engine` (**sans-IO** request/response
matcher) → `maestro` / `settings` / `models` (typed requests, decoded replies, allow-listed settings, feature
gating by device type + firmware). `client` (feature `tokio`) drives the engine over any `AsyncRead+AsyncWrite`.
`testing` (feature `testing`) is a scripted fake Pixel Buds Pro serving an in-memory duplex stream, modelling
observed hardware quirks; daemon tests use it as a dev-dependency. Protobufs come from `proto/` via `build.rs`
(prost; .proto comments are deliberately stripped from generated code).
pw_rpc addresses services and methods by a 65599 hash of their names (`ids.rs`): never rename `maestro_pw` services
or rpcs. Message and field names are free (only field numbers reach the wire). Compare
`protoc --descriptor_set_out` output before and after a proto edit.

### `crates/kbudsd` (per-user daemon, `systemd --user`, bus name `io.github.kbuds.Daemon`)

- `main.rs`: connects system + session buses, registers the Maestro external profile and battery provider, runs
  the `bluez::monitor` event loop and spawns one supervisor per Pixel Buds device.
- `bluez.rs`: mirrors the `org.bluez` object tree; reports devices exposing `MAESTRO_UUID` and A2DP transports.
- `profile.rs`: BlueZ `Profile1` (client role). `Device1.ConnectProfile(MAESTRO_UUID)` makes BlueZ hand the RFCOMM
  socket over `NewConnection`; `Router` dispatches it to the right device; `fdstream.rs` wraps the fd for tokio.
- `device.rs`: per-device supervisor — connects while the buds are connected, runs sessions, **reconnects after
  the buds close the DLC** (every primary handover does), watches audio, publishes state on D-Bus.
- `session.rs`: one Maestro session. Host id, primary bud, subscriptions and the exclusive toggle are learned/applied
  fresh every session and never carried over.
- `state.rs` (`DeviceState`, mirrors `Device1.xml`), `api.rs` (zbus interfaces), `battery.rs` (BlueZ
  `BatteryProvider1`, exports the lower bud's level), `audio.rs` (checks for Opus Dual Mono / HFP fallback and
  reconnects A2DP so the PC configures Opus Stereo), `config.rs` (`$XDG_CONFIG_HOME/kbuds/kbudsd.toml`).

### D-Bus API: `dbus/*.xml` is the single source of truth

`io.github.kbuds.Manager1` (`/io/github/kbuds`) and `io.github.kbuds.Device1`. Consumers:
- `kbudsd/src/api.rs` implements them by hand; its `tests` module asserts the served interfaces match the XML
  exactly (names, signatures, access), so an API change that misses either side fails `cargo test`.
- `kbudsctl/build.rs` generates zbus proxies from the XML (`zbus_xmlgen`), included by `kbudsctl/src/dbus.rs`.
- `gui/CMakeLists.txt` generates Qt proxies with `qt_add_dbus_interface` (`KBUDS_DBUS_DIR` overrides the path).

To change the API: edit the XML, then `api.rs`/`state.rs` in the daemon, then the CLI and `gui/src/device.{h,cpp}` +
QML as needed.

### `crates/kbudsctl`, `gui/`

`kbudsctl` is a thin clap client; it also emits shell completions and its man page (`kbudsctl completions <sh>`,
`kbudsctl man`), which `debian/rules` installs. The GUI is Qt 6.8+ QML, Material style (forced in `main.cpp` so
desktop platform themes don't override it); `DaemonClient` tracks the manager and `Device` wraps one Device1 proxy.

### `data/` and packaging

systemd user unit (`Type=dbus`, `ConditionUser=!@system`), D-Bus activation file, and a WirePlumber rule giving the
buds' A2DP sink a 1 s suspend timeout (needed so the buds hand audio back to the other multipoint host promptly).
`debian/` builds offline from `vendor/`; `override_dh_clean` uses `-Xvendor/` because vendored `Cargo.toml.orig`
files are checksummed. Maintainer scripts start/restart `kbudsd` and WirePlumber in running user sessions.

## Conventions

- Code files (Rust, C++, QML, CMake, scripts, `debian/rules`) start with
  `SPDX-License-Identifier: GPL-3.0-or-later`.
- Comments and protos state protocol facts with their evidence level (`[confirmed]`,
  `[confirmed on hardware: Pro fw 575]`, `[inferred]`).
- Commit subjects are imperative, optionally prefixed by component (`kbudsd: …`, `GUI: …`, `Package: …`).
