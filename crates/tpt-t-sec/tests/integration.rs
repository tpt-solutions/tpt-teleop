//! End-to-end Phase 11 integration: a mutually-authenticated handshake feeds
//! a `SecureMux` pair that exchanges an `ControlCommand` over real UDP, with
//! zero-copy in-place decryption into an `SpscRing`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tpt_t_core::mode::Mode;
use tpt_t_core::ser::{ControlCommand, access_root};
use tpt_t_ring::SpscRing;
use tpt_t_sec::SecureBlock;
use tpt_t_sec::cipher::CipherSuite;
use tpt_t_sec::identity::DeviceIdentity;
use tpt_t_sec::link::SecureMux;
use tpt_t_sec::rbac::Role;
use tpt_t_sec::session::{begin_handshake, finish_handshake, respond_handshake};

fn sample_cmd(seq: u64) -> ControlCommand {
    let mut c = ControlCommand::zeroed(Mode::FullTeleop);
    c.seq = seq;
    c.timestamp_ns = 1_000 + seq;
    c.axes[0] = 0.25;
    c.axes[3] = -0.5;
    c
}

#[test]
fn secure_control_over_real_udp_roundtrips_into_ring() {
    let a = DeviceIdentity::generate(1, Role::Operator).unwrap();
    let b = DeviceIdentity::generate(2, Role::Admin).unwrap();

    let suites = CipherSuite::all().to_vec();
    let (init, pending) = begin_handshake(&a, &suites).unwrap();
    let (resp, sess_b) = respond_handshake(&b, &init, &suites).unwrap();
    let sess_a = finish_handshake(&a, pending, &resp).unwrap();

    assert_eq!(sess_a.peer_id(), 2);
    assert_eq!(sess_b.peer_id(), 1);

    let mut mux_a = SecureMux::bind(0, sess_a).unwrap();
    let mut mux_b = SecureMux::bind(0, sess_b).unwrap();
    let port_b = mux_b.local_addr().unwrap().port();
    let port_a = mux_a.local_addr().unwrap().port();
    let dst_b: SocketAddr = SocketAddr::from(([127, 0, 0, 1], port_b));
    let dst_a: SocketAddr = SocketAddr::from(([127, 0, 0, 1], port_a));

    // A → B: encrypted control command.
    let cmd = sample_cmd(7);
    let sent = mux_a.send_secure_control(&cmd, dst_b, 0).unwrap();
    assert!(sent > 0);

    let ring: Arc<SpscRing<SecureBlock<256>>> = Arc::new(SpscRing::with_capacity(8));
    let ring_b = Arc::clone(&ring);
    let mut rx = tpt_t_link::mux::RxBuffer::new();

    // Drain until B decrypts a block into the ring (or time out).
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut got = 0;
    while std::time::Instant::now() < deadline {
        got += mux_b.recv_decrypt(&mut rx, &ring_b).unwrap();
        if got > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(got, 1, "expected one decrypted block");

    let block = ring.pop().expect("block present");
    let arch = access_root::<ControlCommand>(block.as_slice()).expect("valid rkyv");
    let received = tpt_t_link::mux::UdpMux::command_from_archived(arch);
    assert_eq!(received.seq, 7);
    assert_eq!(received.mode(), Some(Mode::FullTeleop));
    assert_eq!(received.axes[0], 0.25);
    assert_eq!(received.axes[3], -0.5);

    // B → A: echo an encrypted telemetry-bearing control back, proving the
    // session is bidirectional and key-synchronized.
    let cmd2 = sample_cmd(8);
    let sent2 = mux_b.send_secure_control(&cmd2, dst_a, 0).unwrap();
    assert!(sent2 > 0);

    let ring_a = SpscRing::<SecureBlock<256>>::with_capacity(8);
    let mut rx_a = tpt_t_link::mux::RxBuffer::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if mux_a.recv_decrypt(&mut rx_a, &ring_a).unwrap() > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let block_a = ring_a.pop().expect("block present");
    let arch_a = access_root::<ControlCommand>(block_a.as_slice()).expect("valid rkyv");
    let received_a = tpt_t_link::mux::UdpMux::command_from_archived(arch_a);
    assert_eq!(received_a.seq, 8);
}

#[test]
fn secure_telemetry_uses_separate_aad_and_decrypts() {
    // Regression for the AAD_CONTROL/AAD_TELEMETRY mismatch (Phase 15, item 7):
    // a telemetry envelope sealed under AAD_TELEMETRY must decrypt on the
    // peer, which selects the AAD domain from the frame's inner-channel tag.
    use tpt_t_core::ser::TelemetryKind;

    let a = DeviceIdentity::generate(11, Role::Operator).unwrap();
    let b = DeviceIdentity::generate(12, Role::Admin).unwrap();
    let suites = CipherSuite::all().to_vec();
    let (init, pending) = begin_handshake(&a, &suites).unwrap();
    let (resp, sess_b) = respond_handshake(&b, &init, &suites).unwrap();
    let sess_a = finish_handshake(&a, pending, &resp).unwrap();

    let mut mux_a = SecureMux::bind(0, sess_a).unwrap();
    let mut mux_b = SecureMux::bind(0, sess_b).unwrap();
    let dst_b: SocketAddr = SocketAddr::from(([127, 0, 0, 1], mux_b.local_addr().unwrap().port()));

    let pkt = TelemetryPacket {
        values: [3.5; 8],
        ..TelemetryPacket::zeroed(TelemetryKind::Battery, 42, 7)
    };
    let sent = mux_a.send_secure_telemetry(&pkt, dst_b, 0).unwrap();
    assert!(sent > 0);

    let ring: Arc<SpscRing<SecureBlock<256>>> = Arc::new(SpscRing::with_capacity(8));
    let mut rx = tpt_t_link::mux::RxBuffer::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if mux_b.recv_decrypt(&mut rx, &ring).unwrap() > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let block = ring.pop().expect("telemetry block decrypted");
    let arch = access_root::<TelemetryPacket>(block.as_slice()).expect("valid rkyv");
    assert_eq!(arch.seq, 7);
    assert_eq!(arch.kind, TelemetryKind::Battery);
    assert_eq!(arch.values[0], 3.5);
}
