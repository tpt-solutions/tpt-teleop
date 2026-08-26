//! Batteries-included imports for typical tpt-teleop applications
//! (`use tpt_t_core::prelude::*;`, mirroring spec §8).

// Modes & state machine
pub use crate::{
    affinity::{core_count, pin_current, spawn_pinned},
    bus::MessageBus,
    machine::StateMachine,
    mode::{Mode, ModeError, Transition, TransitionTable},
    mpmc::MpmcRing,
    pool::{BufferPool, Pooled},
    profile::{CoreProfile, Role},
};

// Zero-copy serialization toolkit
pub use crate::ser::{
    AlignedBuf, ControlCommand, GpsSample, ImuSample, TelemetryKind, TelemetryPacket, WIRE_ALIGN,
    WireError, WireFrame, access_root, serialize_into,
};

// Lock-free primitives (re-exported so apps depend on core only)
pub use tpt_t_ring::{
    CachePadded, PointerRing, PointerRingExt, Ptr, SharedHeader, SharedSpsc, SpscRing,
};
