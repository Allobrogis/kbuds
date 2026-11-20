// SPDX-License-Identifier: GPL-3.0-or-later
//! The tokio client against the fake buds.

use std::time::Duration;

use kbuds_protocol::client::{Connection, Notification, StreamMsg, timing};
use kbuds_protocol::engine::{RpcError, StreamEnd};
use kbuds_protocol::maestro::{self, RuntimeInfo};
use kbuds_protocol::rpc::{PacketType, Status};
use kbuds_protocol::settings::{AncMode, Setting, SettingId};
use kbuds_protocol::testing::{self, FakeState};
use kbuds_protocol::{Bud, Endpoint, HostId, ids};

async fn connect(state: FakeState) -> (Connection, tokio::sync::mpsc::UnboundedReceiver<Notification>, testing::FakeHandle) {
    let (host_io, dev_io) = tokio::io::duplex(8192);
    let fake = testing::spawn(dev_io, state);
    let (conn, notes) = Connection::open(host_io, timing::FIRST_FRAME, None).await.unwrap();
    (conn, notes, fake)
}

#[tokio::test]
async fn host_id_hello_and_primary_detection() {
    for host in [HostId::A, HostId::B] {
        let (conn, mut notes, _fake) =
            connect(FakeState { host, primary: Bud::Right, ..Default::default() }).await;
        assert_eq!(conn.host_id(), host);
        // The hello is reported as unsolicited.
        let Some(Notification::Unsolicited { from, packet }) = notes.recv().await else { panic!() };
        assert_eq!(from, Some(Endpoint::RightBtCore));
        assert_eq!(packet.call_id, u32::MAX);
        let l = conn.software_info(Bud::Left).await.unwrap();
        let r = conn.software_info(Bud::Right).await.unwrap();
        assert!(l.is_secondary && !r.is_secondary);
        assert_eq!(r.versions.left.as_deref(), Some("575"));
        assert_eq!(conn.hardware_info(Bud::Right).await.unwrap().device_type, 4);
    }
}

#[tokio::test]
async fn settings_read_write_and_failed_precondition() {
    let (conn, _notes, fake) = connect(FakeState::default()).await;
    let t = timing::READ_SETTING;
    assert_eq!(conn.read_setting(Bud::Left, SettingId::Anc, t).await.unwrap(), Some(Setting::Anc(Some(AncMode::Active))));
    conn.write_setting(Bud::Left, &Setting::Anc(Some(AncMode::Aware)), t).await.unwrap();
    assert_eq!(conn.read_setting(Bud::Left, SettingId::Anc, t).await.unwrap(), Some(Setting::Anc(Some(AncMode::Aware))));
    fake.state.lock().unwrap().in_case = [true, true];
    assert_eq!(conn.read_setting(Bud::Left, SettingId::Anc, t).await.unwrap(), Some(Setting::Anc(None)));
    assert_eq!(
        conn.write_setting(Bud::Left, &Setting::Anc(Some(AncMode::Off)), t).await.unwrap_err(),
        RpcError::Status(Status::FailedPrecondition)
    );
    // UNKNOWN(2) status is an error, not a value.
    assert_eq!(
        conn.read_setting(Bud::Left, SettingId::UserEq, t).await.unwrap_err(),
        RpcError::Status(Status::Unknown)
    );
}

#[tokio::test]
async fn unary_timeout() {
    let (conn, _notes, _fake) =
        connect(FakeState { silent: vec!["GetHardwareInfo"], ..Default::default() }).await;
    let r = conn
        .unary(Endpoint::LeftBtCore, ids::GET_HARDWARE_INFO, vec![], Duration::from_millis(50))
        .await;
    assert_eq!(r.unwrap_err(), RpcError::Timeout);
    // The connection still works afterwards.
    conn.software_info(Bud::Left).await.unwrap();
}

#[tokio::test]
async fn runtime_stream_push_and_cancel() {
    let (conn, _notes, fake) = connect(FakeState::default()).await;
    let mut sub = conn.subscribe(Endpoint::LeftBtCore, ids::SUBSCRIBE_RUNTIME_INFO, vec![], false).await.unwrap();
    let Some(StreamMsg::Item(p)) = sub.next().await else { panic!() };
    assert_eq!(RuntimeInfo::decode(&p).unwrap().battery.unwrap().left.unwrap().percent, Some(80));
    fake.state.lock().unwrap().battery[0] = Some((55, true));
    fake.push_runtime();
    let Some(StreamMsg::Item(p)) = sub.next().await else { panic!() };
    let b = RuntimeInfo::decode(&p).unwrap().battery.unwrap().left.unwrap();
    assert_eq!((b.percent, b.charging), (Some(55), true));
    sub.cancel();
    // The fake saw CLIENT_ERROR.
    conn.software_info(Bud::Left).await.unwrap();
    let rx = fake.received.lock().unwrap().clone();
    assert!(rx.iter().any(|r| r.kind == PacketType::ClientError && r.method == "SubscribeRuntimeInfo"));
}

#[tokio::test]
async fn settings_subscription_rules() {
    // Secondary ends the stream at once with OK; primary keeps it and echoes call_id.
    let (conn, _notes, fake) = connect(FakeState { host: HostId::B, ..Default::default() }).await;
    let m = ids::SUBSCRIBE_SETTINGS_CHANGES;
    let mut right = conn.subscribe(Endpoint::RightBtCore, m, vec![], true).await.unwrap();
    assert_eq!(right.next().await, Some(StreamMsg::End(StreamEnd::Completed(Status::Ok))));
    let mut left = conn.subscribe(Endpoint::LeftBtCore, m, vec![], true).await.unwrap();
    fake.push_setting(&Setting::Multipoint(true));
    let Some(StreamMsg::Item(p)) = left.next().await else { panic!() };
    assert_eq!(kbuds_protocol::settings::decode(&p).unwrap(), Some(Setting::Multipoint(true)));
}

#[tokio::test]
async fn maestro_a_push_without_subscription_is_unsolicited() {
    let (_conn, mut notes, fake) = connect(FakeState::default()).await;
    let _hello = notes.recv().await;
    fake.push_setting(&Setting::Anc(Some(AncMode::Off)));
    let Some(Notification::Unsolicited { packet, .. }) = notes.recv().await else { panic!() };
    assert_eq!((packet.kind, packet.method_id), (PacketType::ServerStream, ids::SUBSCRIBE_SETTINGS_CHANGES.method_id));
}

#[tokio::test]
async fn exclusive_and_quiet_mode() {
    let (conn, _notes, fake) = connect(FakeState { quiet_mode: true, ..Default::default() }).await;
    let mut q = conn.subscribe(Endpoint::LeftBtCore, ids::SUBSCRIBE_QUIET_MODE, vec![], true).await.unwrap();
    let Some(StreamMsg::Item(p)) = q.next().await else { panic!() };
    assert!(maestro::decode_quiet_mode(&p).unwrap());
    conn.force_multipoint_switch(Bud::Left, false).await.unwrap();
    assert!(fake.state.lock().unwrap().exclusive);
    fake.push_quiet_mode();
    let Some(StreamMsg::Item(p)) = q.next().await else { panic!() };
    assert!(!maestro::decode_quiet_mode(&p).unwrap());
    conn.force_multipoint_switch(Bud::Left, true).await.unwrap();
    assert!(!fake.state.lock().unwrap().exclusive);
    let rx = fake.received.lock().unwrap().clone();
    let fs: Vec<_> = rx.iter().filter(|r| r.method == "ForceMultipointSwitch").map(|r| r.payload.clone()).collect();
    assert_eq!(fs, vec![vec![0x08, 0x0a], vec![0x08, 0x0a, 0x10, 0x01]]);
}

#[tokio::test]
async fn close_fails_pending_and_notifies() {
    let (conn, mut notes, fake) =
        connect(FakeState { silent: vec!["GetHardwareInfo"], ..Default::default() }).await;
    let mut sub = conn.subscribe(Endpoint::LeftBtCore, ids::SUBSCRIBE_RUNTIME_INFO, vec![], false).await.unwrap();
    let pending = {
        let c = conn.clone();
        tokio::spawn(async move { c.hardware_info(Bud::Left).await })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    fake.close();
    assert_eq!(pending.await.unwrap().unwrap_err(), RpcError::ConnectionClosed);
    let _first = sub.next().await;
    assert_eq!(sub.next().await, Some(StreamMsg::End(StreamEnd::ConnectionClosed)));
    loop {
        match notes.recv().await {
            Some(Notification::Closed(_)) => break,
            Some(_) => continue,
            None => panic!("no Closed"),
        }
    }
    assert!(conn.software_info(Bud::Left).await.is_err());
}

#[tokio::test]
async fn open_times_out_without_first_frame() {
    let (host_io, _dev_io) = tokio::io::duplex(64);
    let r = Connection::open(host_io, Duration::from_millis(50), None).await;
    assert!(matches!(r, Err(kbuds_protocol::client::OpenError::NoFirstFrame(_))));
}
