//! Virtual CAN bus over SPSC rings — two endpoints, zero locks, no allocs.

use std::sync::Arc;

use tpt_t_ring::SpscRing;

use crate::can::CanBus;
use crate::types::{CanFrame, HalError};

/// One endpoint of a virtual point-to-point CAN bus.
///
/// Fault injection mirrors real-world failure modes: `drop_every(n)` drops
/// every n-th transmit (congestion), and natural ring-full behaves like TX
/// buffer overflow on silicon.
pub struct CanEndpoint {
    tx: Arc<SpscRing<CanFrame>>,
    rx: Arc<SpscRing<CanFrame>>,
    tx_count: u64,
    drop_every: u32, // 0 = never inject
}

// SAFETY: rings are Send/Sync for CanFrame (POD); endpoint ownership is
// one-tx-one-rx by construction of can_pair.
unsafe impl Send for CanEndpoint {}

impl CanBus for CanEndpoint {
    fn send(&mut self, frame: &CanFrame) -> Result<(), HalError> {
        self.tx_count = self.tx_count.wrapping_add(1);
        if self.drop_every > 0 && self.tx_count % self.drop_every as u64 == 0 {
            return Err(HalError::Dropped);
        }
        self.tx.push(*frame).map_err(|_| HalError::Dropped)
    }

    fn recv(&mut self, out: &mut CanFrame) -> bool {
        match self.rx.pop() {
            Some(frame) => {
                *out = frame;
                true
            }
            None => false,
        }
    }
}

impl CanEndpoint {
    /// Injects dropped transmits: every `n`-th successful-or-attempted send
    /// returns [`HalError::Dropped`] without queueing. `0` disables (default).
    pub fn set_drop_every(&mut self, n: u32) {
        self.drop_every = n;
    }

    /// Total attempted transmissions (wrapping counter).
    pub fn tx_attempts(&self) -> u64 {
        self.tx_count
    }
}

/// Creates a connected virtual bus pair: `(endpoint_a, endpoint_b)`.
/// Whatever A sends, B receives, and vice versa.
pub fn can_pair(capacity: usize) -> (CanEndpoint, CanEndpoint) {
    let a2b = Arc::new(SpscRing::with_capacity(capacity));
    let b2a = Arc::new(SpscRing::with_capacity(capacity));
    (
        CanEndpoint {
            tx: Arc::clone(&a2b),
            rx: Arc::clone(&b2a),
            tx_count: 0,
            drop_every: 0,
        },
        CanEndpoint {
            tx: b2a,
            rx: a2b,
            tx_count: 0,
            drop_every: 0,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::can::ids;

    #[test]
    fn bidirectional_roundtrip() {
        let (mut a, mut b) = can_pair(8);

        a.send(&CanFrame::new(ids::MOTOR_CMD, &[1, 2, 3, 4]))
            .unwrap();
        b.send(&CanFrame::new(ids::IMU_DATA, &[9])).unwrap();

        let mut out = CanFrame::new(0, &[]);
        assert!(b.recv(&mut out));
        assert_eq!(
            (out.id, out.payload()),
            (ids::MOTOR_CMD, &[1u8, 2, 3, 4][..])
        );
        assert!(a.recv(&mut out));
        assert_eq!((out.id, out.payload()), (ids::IMU_DATA, &[9u8][..]));
        assert!(!b.recv(&mut out)); // drained
    }

    #[test]
    fn fifo_order_preserved() {
        let (mut a, mut b) = can_pair(16);
        for i in 0..10u32 {
            a.send(&CanFrame::new(i, &[])).unwrap();
        }
        let mut out = CanFrame::new(0, &[]);
        for i in 0..10u32 {
            assert!(b.recv(&mut out));
            assert_eq!(out.id, i);
        }
    }

    #[test]
    fn overflow_is_dropped_error() {
        let (mut a, _b) = can_pair(4); // capacity rounds to 4
        for i in 0..4u32 {
            assert!(a.send(&CanFrame::new(i, &[])).is_ok());
        }
        assert!(matches!(
            a.send(&CanFrame::new(99, &[])),
            Err(HalError::Dropped)
        ));
    }

    #[test]
    fn fault_injection_drops_every_nth() {
        let (mut a, mut b) = can_pair(8);
        a.set_drop_every(3);

        let mut delivered = 0;
        let mut out = CanFrame::new(0, &[]);
        for i in 1..=6u32 {
            if a.send(&CanFrame::new(i, &[])).is_ok() {
                assert!(b.recv(&mut out));
                assert_eq!(out.id, i);
                delivered += 1;
            }
        }
        // Sends 3 and 6 were injected-drop failures.
        assert_eq!(delivered, 4);
        assert_eq!(a.tx_attempts(), 6);
    }

    #[test]
    fn cross_thread_ping_pong() {
        let (mut a, mut b) = can_pair(4);
        let peer = std::thread::spawn(move || {
            let mut out = CanFrame::new(0, &[]);
            let mut seen = 0;
            while seen < 100 {
                if b.recv(&mut out) {
                    // echo back with id+0x1000
                    b.send(&CanFrame::new(out.id + 0x1000, out.payload()))
                        .unwrap();
                    seen += 1;
                } else {
                    std::hint::spin_loop();
                }
            }
        });
        let mut f = CanFrame::new(1, &[]);
        for i in 0..100u32 {
            f.id = i;
            while a.send(&f).is_err() {
                std::hint::spin_loop();
            }
            let mut back = CanFrame::new(0, &[]);
            loop {
                if a.recv(&mut back) {
                    assert_eq!(back.id, i + 0x1000);
                    break;
                }
                std::hint::spin_loop();
            }
        }
        peer.join().unwrap();
    }
}
