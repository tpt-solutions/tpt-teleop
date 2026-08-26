//! OpenXR 6-DOF hand-tracking abstraction (spec §5.1).
//!
//! The full OpenXR loader is a platform-installed runtime; binding it is
//! deferred until a runtime target ships on the test bench. What lands here
//! **now** is the integration surface downstream code compiles against:
//! pose types, the [`HandTrackingSource`] trait, and [`NullHandTracking`]
//! for headless runs. Swapping in a real `XrHandTracking` source later
//! cannot break callers — they only ever see this trait.

/// One tracked joint: position (m) + orientation quaternion `[w,x,y,z]`.
pub type JointPose = [f32; 7];

/// Per-hand tracking snapshot.
#[derive(Debug, Clone, Copy, Default)]
pub struct HandPose {
    /// Wrist joint pose.
    pub wrist: JointPose,
    /// Index-tip pinch strength 0..1 (grip-style triggers).
    pub pinch: f32,
    /// Whether the hand is currently inside tracking volume.
    pub tracked: bool,
}

/// 6-DOF VR/AR hand-tracking source.
pub trait HandTrackingSource: Send {
    /// Fills both hands with the freshest poses; `false` when no new frame
    /// since the previous call.
    fn poll_hands(&mut self, left: &mut HandPose, right: &mut HandPose) -> bool;
}

/// Always-untracked source for headless tests and non-VR units.
#[derive(Debug, Default)]
pub struct NullHandTracking;

impl NullHandTracking {
    /// Constructor matching the trait-object ergonomics of real sources.
    pub fn new() -> Self {
        Self
    }
}

impl HandTrackingSource for NullHandTracking {
    fn poll_hands(&mut self, _left: &mut HandPose, _right: &mut HandPose) -> bool {
        false
    }
}
