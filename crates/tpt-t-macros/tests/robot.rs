//! End-to-end compile + runtime test for `#[derive(tpt_t::Robot)]`.
//!
//! This is an integration test, so it compiles as a separate crate that uses
//! the `tpt_t` proc-macro directly — exercising the real codegen path
//! (lock-free rings, thread-pinning launch, zero-copy serialization).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tpt_t::Robot;
use tpt_t_core::profile::CoreProfile;
use tpt_t_core::robot::RobotDevice;
use tpt_t_core::ser::AlignedBuf;

use rkyv::{Archive, Deserialize, Serialize};

#[derive(Archive, Serialize, Deserialize)]
#[repr(C)]
struct Frame {
    seq: u32,
}

#[derive(Archive, Serialize, Deserialize)]
#[repr(C)]
struct Command {
    throttle: f32,
}

struct Camera(Arc<AtomicBool>);
impl RobotDevice for Camera {
    fn run(self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct Motor(Arc<AtomicBool>);
impl RobotDevice for Motor {
    fn run(self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[derive(Robot)]
#[robot(thread_per_core = false)]
struct Bot {
    #[camera(id = 0, element = Frame, capacity = 64)]
    cam: Camera,
    #[motor(id = 1, element = Command, capacity = 64)]
    arm: Motor,
}

#[test]
fn codegen_emits_rings_ser_and_roles() {
    let ran_cam = Arc::new(AtomicBool::new(false));
    let ran_motor = Arc::new(AtomicBool::new(false));
    let bot = Bot {
        cam: Camera(ran_cam.clone()),
        arm: Motor(ran_motor.clone()),
    };

    // thread_per_core was set to false in the attribute.
    const { assert!(!Bot::THREAD_PER_CORE); }
    // One role per wired device, in declaration order.
    assert_eq!(Bot::roles().len(), 2);

    // 1. Lock-free rings from struct fields.
    let ch = bot.channels();
    assert!(Bot::push_cam(&ch, Frame { seq: 1 }).is_ok());
    assert!(Bot::push_arm(&ch, Command { throttle: 0.5 }).is_ok());
    assert_eq!(Bot::pop_cam(&ch).unwrap().seq, 1);
    let popped = Bot::pop_arm(&ch).unwrap();
    assert!((popped.throttle - 0.5).abs() < 1e-6);

    // 3. Zero-copy serialization boilerplate.
    let mut buf = AlignedBuf::new();
    let n = Bot::serialize_cam(&Frame { seq: 9 }, &mut buf).unwrap();
    assert!(n > 0);
    assert_eq!(buf.as_ptr() as usize % tpt_t_core::ser::WIRE_ALIGN, 0);

    let _ = (ran_cam, ran_motor);
}

#[test]
fn launch_moves_each_device_into_its_own_thread() {
    let ran_cam = Arc::new(AtomicBool::new(false));
    let ran_motor = Arc::new(AtomicBool::new(false));
    let bot = Bot {
        cam: Camera(ran_cam.clone()),
        arm: Motor(ran_motor.clone()),
    };
    let profile = CoreProfile::parse("video = 0\ncontrol = 1\n").unwrap();
    let handles = bot.launch(&profile).expect("launch should succeed");
    for h in handles {
        h.join().expect("device thread panicked");
    }
    // Each device's RobotDevice::run executed on its own thread.
    assert!(ran_cam.load(Ordering::SeqCst));
    assert!(ran_motor.load(Ordering::SeqCst));
}
