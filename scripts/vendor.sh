#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Vendors the Rust dependencies into vendor/ for offline (.deb) builds and points cargo at them.
set -eu
cd "$(dirname "$0")/.."
rm -rf vendor
mkdir -p .cargo
cargo vendor --locked --versioned-dirs vendor > .cargo/config.toml
echo "vendored $(ls vendor | wc -l) crates; .cargo/config.toml written" >&2
python3 -I scripts/gen-copyright.py > debian/copyright
echo "debian/copyright regenerated" >&2
