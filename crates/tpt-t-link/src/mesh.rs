//! Swarm neighbor discovery (spec §5.2 "Mesh Networking").
//!
//! Every unit periodically emits a [`MeshBeacon`] on the mux's `Mesh`
//! channel. Beacons are tiny fixed-layout POD datagrams: unit id, beacon
//! sequence, timestamp, and the port the unit listens on. Receiving units
//! keep a fixed-capacity [`NeighborTable`] (open-addressed by unit id) with
//! TTL expiry — no allocation, no locks; the table lives on the network
//! thread and is read on demand.
//!
//! Beacons are unicast/multicast-agnostic: the service sends them to every
//! configured bootstrap peer and replies once to newly discovered neighbors,
//! so a swarm self-configures from any seed address without broadcast
//! privileges (deterministic and firewall-friendly).

use std::net::SocketAddr;

use tpt_t_ring::cast::bytes_of;

/// Beacon magic `"MSH\x01"` (mesh protocol revision 1).
pub const MESH_MAGIC: u32 = 0x4D53_4801;

/// Wire layout of one discovery beacon — 40 bytes dense, no implicit
/// padding (explicit pads keep every field aligned and the total a multiple
/// of the 8-byte alignment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct MeshBeacon {
    /// Always [`MESH_MAGIC`].
    pub magic: u32,
    /// Protocol revision.
    pub version: u16,
    /// Reserved flag bits (must be zero in v1).
    pub flags: u8,
    /// Reserved.
    pub _pad0: u8,
    /// Stable per-unit identifier (fleet-assigned).
    pub unit_id: u64,
    /// Monotonic beacon sequence (anti-replay / liveness ordering).
    pub seq: u32,
    /// Explicit pad so `ts_ns` lands on an 8-byte boundary.
    pub _pad1: u32,
    /// UNIX ns timestamp at emit time.
    pub ts_ns: u64,
    /// UDP port this unit's mux listens on (peers reply here).
    pub listen_port: u16,
    /// Reserved.
    pub _pad2: u16,
    /// Trailing pad so `size_of` is a multiple of the struct alignment.
    pub _pad3: u32,
}

// SAFETY: repr(C) dense primitives only; density asserted by test below.
unsafe impl tpt_t_ring::cast::PlainBytes for MeshBeacon {}

/// `size_of::<MeshBeacon>()` — handy for bounds checks.
pub const BEACON_LEN: usize = core::mem::size_of::<MeshBeacon>();

impl MeshBeacon {
    /// Builds a fresh beacon.
    pub fn new(unit_id: u64, seq: u32, ts_ns: u64, listen_port: u16) -> Self {
        Self {
            magic: MESH_MAGIC,
            version: 1,
            flags: 0,
            _pad0: 0,
            unit_id,
            seq,
            _pad1: 0,
            ts_ns,
            listen_port,
            _pad2: 0,
            _pad3: 0,
        }
    }

    /// Encodes into `out`; returns bytes written (always [`BEACON_LEN`]).
    /// Callers pass full datagram buffers, so this cannot truncate.
    pub fn encode_into(&self, out: &mut [u8]) -> usize {
        let bytes = bytes_of(self);
        out[..bytes.len()].copy_from_slice(bytes);
        bytes.len()
    }

    /// Decodes and validates a beacon from `bytes`. Works from arbitrary
    /// (unaligned) datagram buffers by copying through an aligned local.
    /// Sound for any input bytes: every field is an integer with no invalid
    /// bit patterns; semantic validation happens below on magic/version.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < BEACON_LEN {
            return None;
        }
        let mut aligned = core::mem::MaybeUninit::<Self>::uninit();
        // SAFETY: destination is aligned and sized for Self; source holds at
        // least BEACON_LEN readable bytes; write precedes the read.
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                aligned.as_mut_ptr().cast::<u8>(),
                BEACON_LEN,
            );
        }
        // SAFETY: MeshBeacon is plain-old-data integers only — every bit
        // pattern is a valid value.
        let beacon = unsafe { aligned.assume_init() };
        (beacon.magic == MESH_MAGIC && beacon.version == 1).then_some(beacon)
    }
}

/// One discovered neighbor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NeighborEntry {
    /// The unit's stable id.
    pub unit_id: u64,
    /// Where its mux receives traffic (control/telemetry/ICE all in one).
    pub addr: SocketAddr,
    /// Monotonic ns of the last accepted beacon.
    pub last_seen_ns: u64,
    /// Sequence of the last accepted beacon (wrap-safe ordering).
    pub last_seq: u32,
    /// Monotonic ns of the last accepted *address* change (flap rate-limit).
    pub last_flap_ns: u64,
}

/// Neighbor-table admission policy (spec §5.2 hardening).
#[derive(Debug, Clone, Copy)]
pub struct MeshConfig {
    /// Maximum tolerated difference between a beacon's timestamp and local
    /// time; beacons further off are rejected as implausible (clock-skew /
    /// replay protection).
    pub max_clock_skew_ns: u64,
    /// Minimum spacing between accepted address changes for the same unit;
    /// changes that arrive sooner are treated as flapping and ignored.
    pub flap_cooldown_ns: u64,
}

impl Default for MeshConfig {
    fn default() -> Self {
        Self {
            max_clock_skew_ns: 30_000_000_000, // 30 s
            flap_cooldown_ns: 2_000_000_000,   // 2 s
        }
    }
}

/// Fixed-capacity open-addressed neighbor table (default 128 slots).
///
/// Owned by the network thread; `&mut` methods make exclusivity explicit and
/// lock-freeness trivially true.
#[derive(Debug)]
pub struct NeighborTable {
    slots: Box<[Option<NeighborEntry>]>,
    len: usize,
    cfg: MeshConfig,
}

impl Default for NeighborTable {
    fn default() -> Self {
        Self::with_capacity(128)
    }
}

impl NeighborTable {
    /// Creates a table with `capacity` slots (rounded up to ≥ 1).
    pub fn with_capacity(capacity: usize) -> Self {
        Self::with_config(capacity, MeshConfig::default())
    }

    /// Creates a table with `capacity` slots and a custom admission policy.
    pub fn with_config(capacity: usize, cfg: MeshConfig) -> Self {
        Self {
            slots: (0..capacity.max(1)).map(|_| None).collect(),
            len: 0,
            cfg,
        }
    }

    /// Number of live entries.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when no neighbors are known.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn probe_start(unit_id: u64) -> usize {
        // Fibonacci hash: cheap avalanche across the low bits.
        (unit_id.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 47) as usize
    }

    /// Records a beacon from `unit_id`. Returns `true` when this is a *new*
    /// or *advanced* observation worth replying to; stale replays from the
    /// same address return `false`. Beacons with an implausible timestamp are
    /// dropped, and too-frequent address changes (flapping) are rate-limited.
    pub fn observe(
        &mut self,
        unit_id: u64,
        addr: SocketAddr,
        seq: u32,
        beacon_ts_ns: u64,
        now_ns: u64,
    ) -> bool {
        // Clock-skew / replay guard: a beacon far ahead of or behind local
        // time cannot be trusted.
        let skew = beacon_ts_ns.abs_diff(now_ns);
        if skew > self.cfg.max_clock_skew_ns {
            return false;
        }
        let cap = self.slots.len();
        let mut idx = Self::probe_start(unit_id) % cap;
        for _ in 0..cap {
            match &mut self.slots[idx] {
                Some(e) if e.unit_id == unit_id => {
                    let advanced = seq.wrapping_sub(e.last_seq) as i32 > 0;
                    let addr_changed = e.addr != addr;
                    if addr_changed {
                        // Rate-limit flapping: ignore address changes that
                        // arrive before the cooldown elapses (keep the old
                        // address so a flapping attacker can't hijack routing).
                        if now_ns.saturating_sub(e.last_flap_ns) < self.cfg.flap_cooldown_ns {
                            e.last_seen_ns = now_ns;
                            return false;
                        }
                        e.addr = addr;
                        e.last_flap_ns = now_ns;
                        e.last_seen_ns = now_ns;
                        if advanced {
                            e.last_seq = seq;
                        }
                        return true;
                    }
                    if advanced {
                        e.last_seq = seq;
                        e.last_seen_ns = now_ns;
                        return true;
                    }
                    e.last_seen_ns = now_ns;
                    return false;
                }
                None => {
                    self.slots[idx] = Some(NeighborEntry {
                        unit_id,
                        addr,
                        last_seen_ns: now_ns,
                        last_seq: seq,
                        last_flap_ns: now_ns,
                    });
                    self.len += 1;
                    return true;
                }
                Some(_) => idx = (idx + 1) % cap, // linear probe continues
            }
        }
        false // table full: silently ignore (fixed memory contract)
    }

    /// Looks a neighbor up by id.
    pub fn find(&self, unit_id: u64) -> Option<NeighborEntry> {
        let cap = self.slots.len();
        let mut idx = Self::probe_start(unit_id) % cap;
        for _ in 0..cap {
            match &self.slots[idx] {
                Some(e) if e.unit_id == unit_id => return Some(*e),
                None => return None, // probe chain would have held it here
                Some(_) => idx = (idx + 1) % cap,
            }
        }
        None
    }

    /// Drops entries silent for longer than `ttl_ns`; returns how many.
    pub fn expire(&mut self, now_ns: u64, ttl_ns: u64) -> usize {
        let mut removed = 0;
        for slot in self.slots.iter_mut() {
            if let Some(e) = slot {
                if now_ns.saturating_sub(e.last_seen_ns) > ttl_ns {
                    *slot = None;
                    removed += 1;
                }
            }
        }
        self.len -= removed;
        removed
    }

    /// Iterates live entries (probe order — not sorted; readers needing order
    /// should collect and sort off the hot path).
    pub fn iter(&self) -> impl Iterator<Item = NeighborEntry> + '_ {
        self.slots.iter().filter_map(|s| *s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddrV4};

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
    }

    #[test]
    fn beacon_layout_is_dense_and_cast_roundtrips() {
        assert_eq!(std::mem::size_of::<MeshBeacon>(), 40);
        let b = MeshBeacon::new(0xDEAD_BEEF, 7, 123_456, 443);
        let mut buf = [0u8; 64];
        let n = b.encode_into(&mut buf);
        assert_eq!(n, BEACON_LEN);

        // Decode from the aligned stack copy...
        assert_eq!(MeshBeacon::decode(&buf[..n]), Some(b));
        // ...and from a deliberately unaligned offset inside a datagram.
        let mut datagram = [0u8; 128];
        datagram[1..1 + n].copy_from_slice(&buf[..n]);
        assert_eq!(MeshBeacon::decode(&datagram[1..1 + n]), Some(b));

        // Corrupt magic → rejected.
        let mut bad = buf;
        bad[0] ^= 0xFF;
        assert_eq!(MeshBeacon::decode(&bad[..n]), None);
        // Truncated → rejected.
        assert_eq!(MeshBeacon::decode(&buf[..20]), None);
    }

    #[test]
    fn observe_inserts_updates_and_rejects_stale_sequences() {
        // Zero flap cooldown preserves the legacy "address change is an event"
        // semantics; dedicated tests below exercise the cooldown.
        let mut t = NeighborTable::with_config(
            8,
            MeshConfig {
                max_clock_skew_ns: 30_000_000_000,
                flap_cooldown_ns: 0,
            },
        );
        assert!(t.is_empty());

        // New neighbor.
        assert!(t.observe(42, addr(1000), 1, 100, 100));
        assert_eq!(t.len(), 1);
        assert_eq!(t.find(42).unwrap().last_seq, 1);

        // Same-seq replay from same address: refresh only, no advance.
        assert!(!t.observe(42, addr(1000), 1, 200, 200));
        assert_eq!(t.find(42).unwrap().last_seen_ns, 200);

        // Advanced seq updates.
        assert!(t.observe(42, addr(1000), 2, 300, 300));
        assert_eq!(t.find(42).unwrap().last_seq, 2);

        // Stale (older) seq ignored.
        assert!(!t.observe(42, addr(1000), 1, 400, 400));
        assert_eq!(t.find(42).unwrap().last_seq, 2);

        // Address change counts as an event (unit moved ports).
        assert!(t.observe(42, addr(2000), 2, 500, 500));
        assert_eq!(t.find(42).unwrap().addr.port(), 2000);

        // Second unit lands independently.
        assert!(t.observe(7, addr(3000), 9, 600, 600));
        assert_eq!(t.len(), 2);
        let mut ids: Vec<_> = t.iter().map(|e| e.unit_id).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![7, 42]);
    }

    #[test]
    fn sequence_ordering_is_wrap_safe() {
        let mut t = NeighborTable::with_capacity(4);
        t.observe(1, addr(1), u32::MAX - 2, 0, 0);
        // +1 across the wrap boundary must count as an advance.
        assert!(t.observe(1, addr(1), u32::MAX - 1, 1, 1));
        assert!(t.observe(1, addr(1), u32::MAX, 2, 2));
        assert!(t.observe(1, addr(1), 0, 3, 3));
        assert!(t.observe(1, addr(1), 1, 4, 4));
        // A jump far backwards (old seq) does not.
        assert!(!t.observe(1, addr(1), u32::MAX - 2, 5, 5));
        assert_eq!(t.find(1).unwrap().last_seq, 1);
    }

    #[test]
    fn expire_removes_only_silent_neighbors() {
        let mut t = NeighborTable::default();
        t.observe(1, addr(10), 1, 1_000, 1_000);
        t.observe(2, addr(20), 1, 5_000, 5_000);

        assert_eq!(t.expire(6_000, 4_000), 1, "unit 1 silent 5s > 4s ttl");
        assert!(t.find(1).is_none());
        assert!(t.find(2).is_some());
        assert_eq!(t.len(), 1);

        assert_eq!(t.expire(6_000, 4_000), 0);
    }

    #[test]
    fn full_table_degrades_without_corruption() {
        let mut t = NeighborTable::with_capacity(2);
        assert!(t.observe(1, addr(1), 1, 0, 0));
        assert!(t.observe(2, addr(2), 1, 0, 0));
        assert!(!t.observe(3, addr(3), 1, 0, 0), "no slot available");
        assert_eq!(t.len(), 2);
        // Existing entries still resolve.
        assert_eq!(t.find(2).unwrap().addr.port(), 2);
    }

    #[test]
    fn implausible_timestamp_is_rejected() {
        let mut t = NeighborTable::with_capacity(8);
        // Beacon timestamp wildly ahead of local time (> 30s skew) is dropped.
        assert!(!t.observe(1, addr(10), 1, 100_000_000_000, 1_000));
        // Beacon timestamp wildly behind local time is dropped.
        assert!(!t.observe(2, addr(20), 1, 1_000, 100_000_000_000));
        assert!(t.is_empty());
        // A believable, in-skew beacon is accepted.
        assert!(t.observe(3, addr(30), 1, 1_000, 1_000));
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn address_flapping_is_rate_limited() {
        let cfg = MeshConfig {
            max_clock_skew_ns: 30_000_000_000,
            flap_cooldown_ns: 2_000_000_000,
        };
        let mut t = NeighborTable::with_config(8, cfg);
        assert!(t.observe(1, addr(100), 1, 1_000, 1_000));
        // Immediate flap to a new address is suppressed (kept on port 100).
        assert!(!t.observe(1, addr(200), 1, 1_100, 1_100));
        assert_eq!(t.find(1).unwrap().addr.port(), 100);
        // After the cooldown elapses, the address change is accepted.
        assert!(t.observe(1, addr(200), 1, 3_100_000_000, 3_100_000_000));
        assert_eq!(t.find(1).unwrap().addr.port(), 200);
    }
}
