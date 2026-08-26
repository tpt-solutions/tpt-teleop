//! BufferPool correctness/stress suite (integration tests).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use tpt_t_core::pool::BufferPool;

#[test]
fn exhaust_then_recycle_retains_contents() {
    let pool = BufferPool::<u64>::new(4);
    let mut guards = Vec::new();
    for i in 0..4u64 {
        let g = pool.get_with(|| i * 10).unwrap();
        assert_eq!(*g, i * 10);
        guards.push(g);
    }
    assert!(pool.get_default().is_none(), "exhausted");
    assert_eq!(pool.in_use(), 4);

    drop(guards.remove(1));
    assert_eq!(pool.in_use(), 3);

    let again = pool.get_default().unwrap();
    assert_eq!(*again, 10, "recycled slot retains prior contents");
}

#[test]
fn concurrent_hammer_never_oversubscribes() {
    const SLOTS: usize = 8;
    let pool = Arc::new(BufferPool::<usize>::new(SLOTS));
    let stop = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();
    for t in 0..4usize {
        let p = Arc::clone(&pool);
        let s = Arc::clone(&stop);
        handles.push(thread::spawn(move || {
            let mut ops = 0usize;
            while s.load(Ordering::Relaxed) == 0 {
                if let Some(mut g) = p.get_with(|| t) {
                    assert!(p.in_use() <= SLOTS);
                    *g += 1; // touch memory through &mut
                    drop(g);
                    ops += 1;
                } else {
                    std::hint::spin_loop();
                }
            }
            ops
        }));
    }

    thread::sleep(std::time::Duration::from_millis(100));
    stop.store(1, Ordering::Relaxed);
    let total: usize = handles.into_iter().map(|h| h.join().unwrap()).sum();
    assert!(total > 0);
    assert_eq!(pool.in_use(), 0, "all guards dropped");
}
