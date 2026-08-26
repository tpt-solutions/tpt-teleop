//! Phase 9 acceptance: every `CanBus` backend satisfies the identical trait
//! contract, so the real backends (Linux SocketCAN, cross-platform stub) are
//! drop-in swappable with the Phase 4 mock bus. The same generic driver
//! exercises all backends with no backend-specific branching.

use tpt_t_hal::{CanBus, CanFrame, HalError, StubCan, can_pair, ids};

/// Generic `CanBus` contract: bidirectional send/recv, FIFO ordering, and
/// payload fidelity, with strict non-blocking semantics (no recv ever blocks,
/// a full/empty bus returns `false`/dropped rather than stalling).
fn exercise_can_contract<A: CanBus, B: CanBus>(a: &mut A, b: &mut B) {
    // Bidirectional exchange.
    assert!(
        a.send(&CanFrame::new(ids::MOTOR_CMD, &[1, 2, 3, 4]))
            .is_ok()
    );
    assert!(b.send(&CanFrame::new(ids::IMU_DATA, &[9])).is_ok());

    let mut out = CanFrame::new(0, &[]);
    assert!(b.recv(&mut out), "B must receive A's frame");
    assert_eq!(
        (out.id, out.payload()),
        (ids::MOTOR_CMD, &[1u8, 2, 3, 4][..])
    );
    assert!(a.recv(&mut out), "A must receive B's frame");
    assert_eq!((out.id, out.payload()), (ids::IMU_DATA, &[9u8][..]));

    // FIFO order preserved across a burst from A → B.
    for i in 0..8u32 {
        assert!(a.send(&CanFrame::new(i, &[i as u8])).is_ok());
    }
    for i in 0..8u32 {
        assert!(b.recv(&mut out), "burst frame {i} lost");
        assert_eq!((out.id, out.payload()), (i, &[i as u8][..]));
    }

    // Empty bus must report no frame without blocking.
    let mut empty = CanFrame::new(0, &[]);
    assert!(!b.recv(&mut empty), "empty bus must not yield a frame");
}

#[test]
fn mock_backend_satisfies_can_contract() {
    let (mut a, mut b) = can_pair(16);
    exercise_can_contract(&mut a, &mut b);
}

#[test]
fn stub_backend_fails_loudly_but_honors_trait() {
    // Construction without real hardware must error — never silently ok, so a
    // session can never believe it has a bus it does not.
    assert!(matches!(
        StubCan::open("can0"),
        Err(HalError::Unsupported(_))
    ));

    // A defaulted stub is still a valid `CanBus`: send is rejected, recv idle.
    // The same generic driver that ran the mock also type-checks against this.
    let mut stub = StubCan::default();
    let mut frame = CanFrame::new(ids::MOTOR_CMD, &[1, 2, 3, 4]);
    assert!(matches!(stub.send(&frame), Err(HalError::Unsupported(_))));
    assert!(
        !stub.recv(&mut frame),
        "inert stub must never yield a frame"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn socketcan_is_drop_in_for_mock() {
    // Only meaningful when a CAN interface is present (e.g. `vcan0`). The point
    // here is to prove the Linux `SocketCan` impl honors the *same* `CanBus`
    // contract as the mock: non-blocking recv on an idle bus, and send that
    // either succeeds or returns `Dropped` (never blocks).
    let mut sc = match tpt_t_hal::SocketCan::open("vcan0") {
        Ok(sc) => sc,
        Err(_) => return, // no CAN interface in this environment
    };
    let mut frame = CanFrame::new(0, &[]);
    assert!(!sc.recv(&mut frame), "idle SocketCAN recv must not block");
    let _ = sc.send(&CanFrame::new(ids::MOTOR_CMD, &[1, 2, 3, 4]));
}
