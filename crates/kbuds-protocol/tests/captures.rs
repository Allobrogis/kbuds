// SPDX-License-Identifier: GPL-3.0-or-later
//! Replays real hardware captures (frame logs as written by `kbudsd --frame-log`:
//! `<timestamp> TX|RX <hex>` lines) through the codec and the engine.
//!
//! The captures hold device serials and addresses, so they are not part of this repository. The
//! test reads the directory `$KBUDS_CAPTURES` (default: `captures/` next to the workspace) and is
//! skipped when it is absent.

use std::path::PathBuf;

use kbuds_protocol::engine::{Engine, Event};
use kbuds_protocol::hdlc::{self, Decoded, Reassembler};
use kbuds_protocol::rpc::Packet;

fn captures_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("KBUDS_CAPTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../captures"));
    dir.is_dir().then_some(dir)
}

fn lines(path: &PathBuf) -> Vec<(String, Vec<u8>)> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let _ts = it.next()?;
            let dir = it.next()?.to_string();
            let data = hex::decode(it.next()?).ok()?;
            (dir == "TX" || dir == "RX").then_some((dir, data))
        })
        .collect()
}

#[test]
fn replay_hardware_captures() {
    let Some(dir) = captures_dir() else {
        eprintln!("captures not found, skipping");
        return;
    };
    let mut logs: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "log"))
        .collect();
    logs.sort();
    let (mut tx_frames, mut rx_frames) = (0, 0);
    for log in &logs {
        let mut engine = Engine::new();
        for (dir, data) in lines(log) {
            if dir == "TX" {
                // Every TX frame in the capture re-encodes byte for byte.
                for item in Reassembler::new().feed(&data) {
                    let Decoded::Frame(f) = item else { panic!("{}: bad TX frame {item:?}", log.display()) };
                    let p = Packet::decode(&f.payload).unwrap();
                    assert_eq!(hdlc::encode(f.address, &p.encode()), data, "{}", log.display());
                    tx_frames += 1;
                }
            } else {
                engine.feed(&data);
                while let Some(ev) = engine.next_event() {
                    match ev {
                        Event::Invalid(why) => panic!("{}: {why}", log.display()),
                        Event::Unsolicited { .. } => rx_frames += 1,
                        _ => {}
                    }
                }
            }
        }
    }
    eprintln!("{} logs, {tx_frames} TX frames re-encoded, {rx_frames} RX packets decoded", logs.len());
}
