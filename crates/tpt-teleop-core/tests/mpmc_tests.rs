//! MPMC queue stress/correctness suite (integration tests).

use std::sync::Arc;
use std::thread;

use tpt_teleop_core::mpmc::MpmcRing;

#[test]
fn spsc_via_mpmc_preserves_order() {
    let q: MpmcRing<u64> = MpmcRing::with_capacity(8);
    for i in 0..100u64 {
        while q.push(i * 3).is_err() {
            std::hint::spin_loop();
        }
        assert_eq!(q.pop(), Some(i * 3));
    }
    assert!(q.is_empty());
}

#[test]
fn full_and_empty_semantics() {
    let q: MpmcRing<u32> = MpmcRing::with_capacity(4);
    assert_eq!(q.capacity(), 4);
    for i in 0..4 {
        assert!(q.push(i).is_ok());
    }
    assert_eq!(q.push(99), Err(99));
    assert_eq!(q.len(), 4);
    assert_eq!(q.pop(), Some(0));
    assert!(q.push(99).is_ok());
    assert_eq!(q.pop(), Some(1));
    assert_eq!(q.pop(), Some(2));
    assert_eq!(q.pop(), Some(3));
    assert_eq!(q.pop(), Some(99));
    assert_eq!(q.pop(), None);
}

#[test]
fn four_producers_four_consumers_exactly_once() {
    const PRODUCERS: u64 = 4;
    const PER_PRODUCER: u64 = 50_000;
    const N: u64 = PRODUCERS * PER_PRODUCER;

    let q = Arc::new(MpmcRing::<u64>::with_capacity(256));
    let mut handles = Vec::new();

    for p in 0..PRODUCERS {
        let q = Arc::clone(&q);
        handles.push(thread::spawn(move || {
            for i in 0..PER_PRODUCER {
                let v = p * PER_PRODUCER + i;
                while q.push(v).is_err() {
                    std::hint::spin_loop();
                }
            }
        }));
    }

    let consumers = 4u64;
    let mut sum_handles = Vec::new();
    for _ in 0..consumers {
        let q = Arc::clone(&q);
        sum_handles.push(thread::spawn(move || {
            let mut sum = 0u128;
            let mut got = 0u64;
            while got < N / consumers {
                if let Some(v) = q.pop() {
                    sum += v as u128;
                    got += 1;
                } else {
                    std::hint::spin_loop();
                }
            }
            sum
        }));
    }

    for h in handles.drain(..) {
        h.join().unwrap();
    }
    let total: u128 = sum_handles.into_iter().map(|h| h.join().unwrap()).sum();
    assert_eq!(
        total,
        (N * (N - 1) / 2) as u128,
        "every value delivered exactly once"
    );
}
