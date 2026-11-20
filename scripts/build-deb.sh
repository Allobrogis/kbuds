#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Builds the kbuds .deb (unsigned) into the parent directory.
# Needs the Build-Depends of debian/control (sudo apt build-dep . or mk-build-deps).
set -eu
cd "$(dirname "$0")/.."
scripts/vendor.sh
dpkg-buildpackage -us -uc -b "$@"
ls -l ../kbuds_*.deb
