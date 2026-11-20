// SPDX-License-Identifier: GPL-3.0-or-later
//! Raw frame log: one `<timestamp> TX|RX <hex>` line per frame sent or chunk received, the format
//! the capture replay test (`kbuds-protocol/tests/captures.rs`) reads.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use kbuds_protocol::client::{Direction, WireTap};
use kbuds_protocol::engine::hex;

/// UTC timestamp with microseconds, e.g. `2026-11-19T14:11:17.123456+00:00`.
pub fn timestamp(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil from days (H. Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:06}+00:00",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        d.subsec_micros()
    )
}

/// Opens (appends to) `path` and returns a tap that logs every chunk, tagged with the device.
pub fn open(path: &Path) -> std::io::Result<Arc<Mutex<File>>> {
    Ok(Arc::new(Mutex::new(OpenOptions::new().create(true).append(true).open(path)?)))
}

pub fn tap(file: Arc<Mutex<File>>, address: &str) -> WireTap {
    if let Ok(mut f) = file.lock() {
        let _ = writeln!(f, "# {} kbudsd addr={address} transport=pigweed", timestamp(SystemTime::now()));
    }
    Arc::new(move |dir: Direction, data: &[u8]| {
        if let Ok(mut f) = file.lock() {
            let _ = writeln!(f, "{} {dir} {}", timestamp(SystemTime::now()), hex(data));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn formats() {
        let t = UNIX_EPOCH + Duration::from_micros(1_795_000_277_123_456);
        assert_eq!(timestamp(t), "2026-11-18T11:11:17.123456+00:00");
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01T00:00:00.000000+00:00");
    }
}
