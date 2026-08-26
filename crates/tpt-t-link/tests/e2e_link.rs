//! End-to-end link pipeline tests (Phase 7 acceptance):
//! safety-loop output ring → rkyv serialize into framed datagram → UDP
//! transmit → demux → validated event, plus mesh discovery and reliable
//! channel behavior across two real loopback [`NetService`]s.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tpt_t_core::mode::Mode;
use tpt_t_core::ser::ControlCommand;
use tpt_t_link::mux::MAX_DATAGRAM;
use tpt_t_link::service::{Event, NetService, ServiceConfig, ServiceCore};
use tpt_t_ring::SpscRing;

type SinkLog = Arc<Mutex<Vec<Event>>>;

fn cmd(seq: u64, throttle: f32) -> ControlCommand {
    let mut c = ControlCommand::zeroed(Mode::FullTeleop);
    c.seq = seq;
    c.timestamp_ns = 1_000_000 + seq * 1_000;
    c.axes[3] = throttle; // climb axis
    c
}

fn sink_for(log: SinkLog) -> impl FnMut(Event, &mut ServiceCore) + 'static {
    move |ev, _core| {
        log.lock().unwrap().push(ev);
    }
}

fn service(cfg: ServiceConfig) -> (NetService, Arc<SpscRing<ControlCommand>>) {
    // Capacity 64: safety output ring for one tick's worth of commands.
    let outbox = Arc::new(SpscRing::<ControlCommand>::with_capacity(64));
    let svc = NetService::new(cfg, Arc::clone(&outbox)).expect("bind service");
    (svc, outbox)
}

#[test]
fn safety_output_flows_through_link_to_peer() {
    let log_b: SinkLog = Arc::new(Mutex::new(Vec::new()));

    // Bind B first so A can be aimed at it.
    let (mut b, _outbox_b) = service(ServiceConfig {
        unit_id: 0xB,
        ..ServiceConfig::default()
    });
    let (mut a, outbox_a) = service(ServiceConfig {
        unit_id: 0xA,
        primary_peer: Some(b.local_addr().unwrap()),
        ..ServiceConfig::default()
    });

    // "Safety loop" approves two commands into A's output ring.
    outbox_a.push(cmd(1, 0.25)).unwrap();
    outbox_a.push(cmd(2, -0.5)).unwrap();

    // One poll tick on each side: A transmits its outbox; B receives it.
    let mut sink_b = sink_for(Arc::clone(&log_b));
    b.poll(Duration::from_millis(30), &mut sink_b).unwrap();
    let mut sink_a = sink_for(Arc::new(Mutex::new(Vec::new())));
    a.poll(Duration::from_millis(150), &mut sink_a).unwrap();
    b.poll(Duration::from_millis(100), &mut sink_b).unwrap();

    let rx = log_b.lock().unwrap();
    let controls: Vec<(u64, f32, u16)> = rx
        .iter()
        .filter_map(|e| match e {
            Event::Control { cmd, from, .. } => Some((cmd.seq, cmd.axes[3], from.port())),
            _ => None,
        })
        .collect();
    assert_eq!(controls.len(), 2, "both commands must arrive: {rx:?}");
    assert_eq!(controls[0].0, 1);
    assert!((controls[0].1 - 0.25).abs() < 1e-6);
    assert_eq!(controls[1].0, 2);
    assert!((controls[1].1 - (-0.5)).abs() < 1e-6);
    // Sender address traces back to A's mux port.
    assert_eq!(controls[0].2, a.local_addr().unwrap().port());
}

#[test]
fn mesh_discovery_connects_two_services() {
    let log_a: SinkLog = Arc::new(Mutex::new(Vec::new()));
    let (mut a, _tx) = service(ServiceConfig {
        unit_id: 111,
        beacon_interval_ns: 10_000_000, // 10 ms heartbeat
        ..ServiceConfig::default()
    });
    let addr_a = a.local_addr().unwrap();

    let log_b: SinkLog = Arc::new(Mutex::new(Vec::new()));
    let (mut b, _txb) = service(ServiceConfig {
        unit_id: 222,
        beacon_interval_ns: 10_000_000,
        beacon_peers: vec![addr_a],
        ..ServiceConfig::default()
    });

    // Drive both sides long enough for beacons + reply-beacon exchange.
    let deadline = std::time::Instant::now() + Duration::from_millis(600);
    while std::time::Instant::now() < deadline {
        let mut sink_a = sink_for(Arc::clone(&log_a));
        a.poll(Duration::from_millis(15), &mut sink_a).unwrap();
        let mut sink_b = sink_for(Arc::clone(&log_b));
        b.poll(Duration::from_millis(15), &mut sink_b).unwrap();
        if !log_a.lock().unwrap().is_empty() && !log_b.lock().unwrap().is_empty() {
            break;
        }
    }

    let saw_b_on_a = log_a
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, Event::NeighborUp(222, _)));
    let saw_a_on_b = log_b
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, Event::NeighborUp(111, _)));
    assert!(saw_b_on_a, "A must discover B");
    assert!(saw_a_on_b, "B must discover A");

    // Tables agree.
    let ids_a: Vec<_> = a.neighbors().map(|n| n.unit_id).collect();
    let ids_b: Vec<_> = b.neighbors().map(|n| n.unit_id).collect();
    assert!(ids_a.contains(&222));
    assert!(ids_b.contains(&111));
}

#[test]
fn reliable_control_roundtrip_feeds_backpressure_rtt() {
    let log_b: SinkLog = Arc::new(Mutex::new(Vec::new()));
    let (mut b, _) = service(ServiceConfig {
        unit_id: 0xB,
        ..ServiceConfig::default()
    });
    let addr_b = b.local_addr().unwrap();
    let (mut a, outbox_a) = service(ServiceConfig {
        unit_id: 0xA,
        reliable_control: true,
        primary_peer: Some(addr_b),
        ..ServiceConfig::default()
    });

    outbox_a.push(cmd(10, 0.1)).unwrap();

    // Arm B, let A transmit, let B process + ack, let A ingest the ack.
    let mut sink_b = sink_for(Arc::clone(&log_b));
    b.poll(Duration::from_millis(30), &mut sink_b).unwrap();
    let mut sink_a = sink_for(Arc::new(Mutex::new(Vec::new())));
    a.poll(Duration::from_millis(120), &mut sink_a).unwrap();
    b.poll(Duration::from_millis(80), &mut sink_b).unwrap();
    a.poll(Duration::from_millis(120), &mut sink_a).unwrap();

    assert!(
        a.rtt_estimate_ns() > 0,
        "ack must deliver an RTT sample into backpressure"
    );

    // B decoded exactly one reliable command.
    let rx = log_b.lock().unwrap();
    let reliable_seqs: Vec<u64> = rx
        .iter()
        .filter_map(|e| match e {
            Event::Control { cmd, reliable, .. } => reliable.then_some(cmd.seq),
            _ => None,
        })
        .collect();
    assert_eq!(reliable_seqs, vec![10]);
}

#[test]
fn corrupted_inbound_is_counted_not_delivered() {
    use tpt_t_core::ser::FRAME_MAGIC;
    use tpt_t_link::crc::crc32;
    use tpt_t_link::mux::Channel;

    let log: SinkLog = Arc::new(Mutex::new(Vec::new()));
    let (mut victim, _txv) = service(ServiceConfig::default());
    let (mut attacker, outbox_attacker) = service(ServiceConfig {
        unit_id: 0xE,
        primary_peer: Some(victim.local_addr().unwrap()),
        ..ServiceConfig::default()
    });

    // A legitimate frame first (so the link is warm and proven), then a
    // hand-crafted frame whose payload bit flips after the CRC was written.
    outbox_attacker.push(cmd(5, 0.0)).unwrap();
    let mut sink = sink_for(Arc::clone(&log));
    attacker.poll(Duration::from_millis(50), &mut sink).unwrap();

    let dst = victim.local_addr().unwrap();
    let mut bad = [0u8; MAX_DATAGRAM];
    bad[..4].copy_from_slice(&FRAME_MAGIC.to_le_bytes());
    bad[4..6].copy_from_slice(&1u16.to_le_bytes()); // PROTOCOL_VERSION
    bad[8] = Channel::Control.as_u8();
    let payload_len = 16u16;
    bad[6..8].copy_from_slice(&payload_len.to_le_bytes());
    let end = tpt_t_link::mux::HEADER_LEN + payload_len as usize;
    let sum = crc32(&bad[..end]);
    bad[end..end + 4].copy_from_slice(&sum.to_le_bytes());
    bad[tpt_t_link::mux::HEADER_LEN + 2] ^= 0xFF; // corrupt AFTER the CRC covers it

    attacker.raw_send(dst, &bad[..end + 4]).unwrap();

    victim.poll(Duration::from_millis(80), &mut sink).unwrap();

    // The corrupt frame never surfaces; the good one did exactly once.
    let seqs: Vec<u64> = log
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            Event::Control { cmd, .. } => Some(cmd.seq),
            _ => None,
        })
        .collect();
    assert_eq!(seqs, vec![5], "corrupt frame rejected, good frame kept");
    assert!(victim.crc_errors() >= 1, "CRC reject must be counted");
}
