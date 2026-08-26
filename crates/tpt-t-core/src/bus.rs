//! Custom lock-free message bus (spec §4, tpt-t-core).
//!
//! Fan-out pub/sub built directly on wait-free SPSC rings: every subscriber
//! owns one [`SpscRing`]; `publish` copies the (plain-old-data) message into
//! each subscriber ring. No locks, no allocations after construction, no
//! crossbeam/tokio channels.
//!
//! `T: Copy` keeps semantics simple: messages are small POD wire structs
//! ([`ControlCommand`](crate::ser::ControlCommand), telemetry samples), so a
//! per-subscriber cache-line-sized memcpy is cheaper than any indirection.
//! Large payloads (video frames) never travel through the bus — they go via
//! pointer tokens over rings (see tpt-t-ring::ptr).

use tpt_t_ring::SpscRing;

/// Identifier for a live subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubscriberId(pub usize);

/// Lock-free fan-out message bus.
pub struct MessageBus<T> {
    subscribers: Vec<SpscRing<T>>,
    per_subscriber_capacity: usize,
}

impl<T: Copy> MessageBus<T> {
    /// Empty bus; subscribers attach later via [`subscribe`](Self::subscribe).
    pub fn new(per_subscriber_capacity: usize) -> Self {
        Self {
            subscribers: Vec::new(),
            per_subscriber_capacity,
        }
    }

    /// Attaches a new subscriber; rings are pre-allocated here (the only
    /// allocation in the bus's life).
    pub fn subscribe(&mut self) -> SubscriberId {
        self.subscribers
            .push(SpscRing::with_capacity(self.per_subscriber_capacity));
        SubscriberId(self.subscribers.len() - 1)
    }

    /// Number of attached subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }

    /// Publishes `value` to every subscriber. Returns how many accepted it;
    /// a subscriber whose ring is full drops this message (backpressure by
    /// policy — control-plane messages are superseded by newer ones anyway).
    ///
    /// Note: acceptance is evaluated per subscriber independently, so a
    /// partially-delivered publish is NOT retried here (retrying would
    /// duplicate the message on the already-accepted rings). Use
    /// [`try_deliver`](Self::try_deliver) for exactly-once fan-out loops.
    pub fn publish(&self, value: T) -> usize {
        let mut delivered = 0;
        for ring in &self.subscribers {
            if ring.push(value).is_ok() {
                delivered += 1;
            }
        }
        delivered
    }

    /// Attempts delivery to one subscriber only. `false` = its ring is full.
    /// Building block for exactly-once fan-out:
    ///
    /// ```ignore
    /// while !(got_a && got_b) { ... try_deliver per id ... }
    /// ```
    pub fn try_deliver(&self, id: SubscriberId, value: T) -> bool {
        self.ring(id).push(value).is_ok()
    }

    /// Drains all pending messages for `id` into `out` (FIFO); returns count.
    pub fn poll(&self, id: SubscriberId, out: &mut Vec<T>) -> usize {
        let mut n = 0;
        while let Some(v) = self.ring(id).pop() {
            out.push(v);
            n += 1;
        }
        n
    }

    /// Non-consuming snapshot of pending count for `id`.
    pub fn pending(&self, id: SubscriberId) -> usize {
        self.ring(id).len()
    }

    fn ring(&self, id: SubscriberId) -> &SpscRing<T> {
        &self.subscribers[id.0]
    }
}

impl<T> core::fmt::Debug for MessageBus<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MessageBus")
            .field("subscribers", &self.subscribers.len())
            .field("per_subscriber_capacity", &self.per_subscriber_capacity)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn fanout_reaches_all_subscribers_in_order() {
        let mut bus: MessageBus<u32> = MessageBus::new(16);
        let a = bus.subscribe();
        let b = bus.subscribe();

        for i in 0..10 {
            assert_eq!(bus.publish(i), 2);
        }

        let mut got_a = Vec::new();
        let mut got_b = Vec::new();
        assert_eq!(bus.poll(a, &mut got_a), 10);
        assert_eq!(bus.poll(b, &mut got_b), 10);
        assert_eq!(got_a, (0..10).collect::<Vec<_>>());
        assert_eq!(got_b, got_a);
        assert_eq!(bus.pending(a), 0);
    }

    #[test]
    fn full_subscriber_ring_drops_message_gracefully() {
        let mut bus: MessageBus<u8> = MessageBus::new(4); // pow2 => 4
        let id = bus.subscribe();
        for i in 0..6u8 {
            let expected = if i < 4 { 1 } else { 0 };
            assert_eq!(bus.publish(i), expected);
        }
        let mut got = Vec::new();
        bus.poll(id, &mut got);
        assert_eq!(got, vec![0, 1, 2, 3]); // newest dropped, FIFO preserved
    }

    #[test]
    fn cross_thread_publisher_to_two_consumers() {
        const N: u32 = 50_000;
        let mut bus: MessageBus<u32> = MessageBus::new(1024);
        let sa = bus.subscribe();
        let sb = bus.subscribe();
        let bus = Arc::new(bus);

        let consumers: Vec<_> = [sa, sb]
            .into_iter()
            .map(|id| {
                let bus = Arc::clone(&bus);
                thread::spawn(move || {
                    // Every subscriber receives the full stream, FIFO, no
                    // gaps and no duplicates.
                    let mut expect = 0u32;
                    let mut buf = Vec::new();
                    while expect < N {
                        buf.clear();
                        bus.poll(id, &mut buf);
                        for v in buf.iter() {
                            assert_eq!(*v, expect, "out-of-order delivery");
                            expect += 1;
                        }
                        if buf.is_empty() {
                            std::hint::spin_loop();
                        }
                    }
                    expect
                })
            })
            .collect();

        for i in 0..N {
            // Exactly-once fan-out: track per-subscriber acceptance so a
            // full ring never produces duplicates on retry.
            let mut done = [false; 2];
            while !done[0] || !done[1] {
                if !done[0] {
                    done[0] = bus.try_deliver(sa, i);
                }
                if !done[1] {
                    done[1] = bus.try_deliver(sb, i);
                }
                std::hint::spin_loop();
            }
        }

        for c in consumers {
            assert_eq!(c.join().unwrap(), N);
        }
    }
}
