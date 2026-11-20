// SPDX-License-Identifier: GPL-3.0-or-later
//! Generates the zbus proxies from the D-Bus interface XML in `dbus/` (the single source of truth,
//! also consumed by the daemon's conformance tests and the Qt GUI's qdbusxml2cpp).

use std::path::PathBuf;

use zbus::names::BusName;
use zbus::zvariant::ObjectPath;
use zbus_xml::Node;
use zbus_xmlgen::CodeGenerator;

const SERVICE: &str = "io.github.kbuds.Daemon";

fn main() {
    let dbus = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../dbus");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let service = BusName::try_from(SERVICE).unwrap();
    for (file, path, out_name) in [
        ("io.github.kbuds.Manager1.xml", Some("/io/github/kbuds"), "manager1.rs"),
        ("io.github.kbuds.Device1.xml", None, "device1.rs"),
    ] {
        let src = dbus.join(file);
        println!("cargo:rerun-if-changed={}", src.display());
        let xml = std::fs::read_to_string(&src).unwrap_or_else(|e| panic!("{}: {e}", src.display()));
        let node = Node::from_reader(xml.as_bytes()).unwrap_or_else(|e| panic!("{file}: {e}"));
        let path = path.map(|p| ObjectPath::try_from(p).unwrap());
        let code = CodeGenerator::new()
            .with_service(Some(&service))
            .with_path(path.as_ref())
            .with_format(false)
            .file_code(node.interfaces(), &[], file, "kbudsctl/build.rs", env!("CARGO_PKG_VERSION"))
            .expect("code generation");
        // The generated header uses inner doc comments (`//!`), which include!() does not accept.
        let code: String =
            code.lines().filter(|l| !l.trim_start().starts_with("//!")).map(|l| format!("{l}\n")).collect();
        std::fs::write(out.join(out_name), code).unwrap();
    }
}
