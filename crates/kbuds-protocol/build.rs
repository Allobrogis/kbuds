// SPDX-License-Identifier: GPL-3.0-or-later
//! Generates the protobuf types from the workspace `proto/` directory.

use std::path::PathBuf;

fn main() -> std::io::Result<()> {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../proto");
    let files = [
        root.join("maestro_pw/maestro.proto"),
        root.join("maestro_pw/settings.proto"),
        root.join("pigweed/rpc_packet.proto"),
    ];
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
    }
    let mut cfg = prost_build::Config::new();
    // The .proto comments are reverse-engineering notes with indented blocks that rustdoc would
    // treat as doctests; keep them in the .proto files only.
    cfg.disable_comments(["."]);
    cfg.compile_protos(&files, &[root])
}
