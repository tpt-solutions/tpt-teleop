//! Deterministic in-house physics core for the mock hardware backend.
//!
//! # Why not rapier?
//!
//! `spec.txt` §5.5 suggests rapier "on its MIT side", but rapier ships
//! Apache-2.0-only — which §2's cargo-deny MIT-chain policy bans outright,
//! and §2 is the mechanically enforced rule. The simulator therefore uses
//! this compact integrator (semi-implicit Euler, quaternion attitude,
//! diagonal-inertia rigid bodies, plane ground contact). It is fully
//! deterministic across runs and platforms that honor IEEE-754, allocation
//! happens only at construction, and it can be swapped for a policy-exempt
//! backend later without touching callers.

/// Body handle inside [`World`].
pub type BodyId = usize;

/// Full rigid-body state. Positions/velocities are world-frame; `omega` is
/// body-frame; attitude is a unit quaternion `[w, x, y, z]`.
#[derive(Debug, Clone)]
pub struct RigidState {
    /// Center-of-mass position (m).
    pub pos: [f64; 3],
    /// Linear velocity (m/s).
    pub vel: [f64; 3],
    /// Attitude quaternion `[w, x, y, z]` (unit).
    pub quat: [f64; 4],
    /// Angular velocity, body frame (rad/s).
    pub omega: [f64; 3],
    /// Mass (kg).
    pub mass: f64,
    /// Inverse diagonal body-frame inertia (1/(kg·m²)).
    pub inv_inertia: [f64; 3],
    /// Half-extents used for the ground-plane contact model (m).
    pub half: [f64; 3],
}

impl RigidState {
    fn new(mass: f64, half: [f64; 3], pos: [f64; 3], yaw: f64) -> Self {
        // Box inertia diagonal: Ixx = m(hy²+hz²)/3 for half-extents.
        let ix = mass / 3.0 * (half[1] * half[1] + half[2] * half[2]);
        let iy = mass / 3.0 * (half[0] * half[0] + half[2] * half[2]);
        let iz = mass / 3.0 * (half[0] * half[0] + half[1] * half[1]);
        Self {
            pos,
            vel: [0.0; 3],
            quat: [((yaw * 0.5).cos()), 0.0, 0.0, ((yaw * 0.5).sin())],
            omega: [0.0; 3],
            mass,
            inv_inertia: [1.0 / ix, 1.0 / iy, 1.0 / iz],
            half,
        }
    }
}

#[inline]
fn quat_mul(a: &[f64; 4], b: &[f64; 4]) -> [f64; 4] {
    [
        a[0] * b[0] - a[1] * b[1] - a[2] * b[2] - a[3] * b[3],
        a[0] * b[1] + a[1] * b[0] + a[2] * b[3] - a[3] * b[2],
        a[0] * b[2] - a[1] * b[3] + a[2] * b[0] + a[3] * b[1],
        a[0] * b[3] + a[1] * b[2] - a[2] * b[1] + a[3] * b[0],
    ]
}

/// Rotates vector `v` (body frame) into the world frame using quaternion `q`.
#[inline]
fn quat_rotate(q: &[f64; 4], v: &[f64; 3]) -> [f64; 3] {
    // t = 2 q_xyz × v ; v' = v + w t + q_xyz × t
    let qx = q[1] * 2.0;
    let qy = q[2] * 2.0;
    let qz = q[3] * 2.0;
    let tx = qy * v[2] - qz * v[1];
    let ty = qz * v[0] - qx * v[2];
    let tz = qx * v[1] - qy * v[0];
    [
        v[0] + q[0] * tx + (q[2] * tz - q[3] * ty),
        v[1] + q[0] * ty + (q[3] * tx - q[1] * tz),
        v[2] + q[0] * tz + (q[1] * ty - q[2] * tx),
    ]
}

/// Rotates vector `v` (world frame) into the body frame (`q⁻¹ · v · q`).
#[inline]
fn quat_rotate_inv(q: &[f64; 4], v: &[f64; 3]) -> [f64; 3] {
    let conj = [q[0], -q[1], -q[2], -q[3]];
    quat_rotate(&conj, v)
}

/// Extracts ZYX Euler angles `(yaw, pitch, roll)` from a unit quaternion.
fn quat_to_euler(q: &[f64; 4]) -> (f64, f64, f64) {
    let sin_pitch = (-q[1] * q[3] + q[0] * q[2]).clamp(-1.0, 1.0);
    let pitch = sin_pitch.asin();
    let yaw = (q[1] * q[2] + q[0] * q[3]).atan2(-q[2] * q[3] + q[0] * q[1]);
    let roll = (q[2] * q[3] + q[0] * q[1]).atan2(-q[1] * q[2] + q[0] * q[3]);
    (yaw, pitch, roll)
}

/// Minimal deterministic physics world: gravity, external force/torque
/// accumulators, semi-implicit Euler, plane ground contact at z = 0.
pub struct World {
    bodies: Vec<RigidState>,
    force: Vec<[f64; 3]>,
    torque: Vec<[f64; 3]>,
    gravity: [f64; 3],
    restitution: f64,
}

impl World {
    /// New world with `gravity` (m/s²), e.g. `[0, 0, -9.81]`.
    pub fn new(gravity: [f64; 3]) -> Self {
        Self {
            bodies: Vec::new(),
            force: Vec::new(),
            torque: Vec::new(),
            gravity,
            restitution: 0.2,
        }
    }

    /// Adds a box body; returns its handle.
    pub fn add_box(&mut self, mass: f64, half: [f64; 3], pos: [f64; 3], yaw_rad: f64) -> BodyId {
        self.bodies.push(RigidState::new(mass, half, pos, yaw_rad));
        let id = self.bodies.len() - 1;
        self.force.push([0.0; 3]);
        self.torque.push([0.0; 3]);
        id
    }

    /// Accumulates a world-frame force for the next [`step`](Self::step).
    #[inline]
    pub fn add_force(&mut self, id: BodyId, f_world: [f64; 3]) {
        for (acc, f) in self.force[id].iter_mut().zip(f_world) {
            *acc += f;
        }
    }

    /// Accumulates a body-frame torque for the next step.
    #[inline]
    pub fn add_torque(&mut self, id: BodyId, tau_body: [f64; 3]) {
        for (acc, t) in self.torque[id].iter_mut().zip(tau_body) {
            *acc += t;
        }
    }

    /// Advances the simulation by one fixed `dt` and clears accumulators.
    // The linear-algebra loops below deliberately co-index parallel vectors
    // (state / force / torque) per axis — clearer than zipping five arrays.
    #[allow(clippy::needless_range_loop)]
    pub fn step(&mut self, dt: f64) {
        for i in 0..self.bodies.len() {
            let b = &mut self.bodies[i];
            // Linear: v += (g + F/m) dt ; x += v dt.
            for k in 0..3 {
                b.vel[k] += (self.gravity[k] + self.force[i][k] / b.mass) * dt;
            }
            for k in 0..3 {
                b.pos[k] += b.vel[k] * dt;
            }
            // Angular (body frame): ω += I⁻¹(τ − ω×Iω) dt.
            let ix = 1.0 / b.inv_inertia[0];
            let iy = 1.0 / b.inv_inertia[1];
            let iz = 1.0 / b.inv_inertia[2];
            let w = b.omega;
            let gyro = [
                w[1] * iz * w[2] - w[2] * iy * w[1],
                w[2] * ix * w[0] - w[0] * iz * w[2],
                w[0] * iy * w[1] - w[1] * ix * w[0],
            ];
            for k in 0..3 {
                b.omega[k] += (b.inv_inertia[k] * (self.torque[i][k] - gyro[k])) * dt;
            }
            // Attitude integration: q̇ = ½ q ⊗ [0, ω_body]; normalize.
            let wq = [0.0, b.omega[0], b.omega[1], b.omega[2]];
            let q_dot = quat_mul_half(&b.quat, &wq);
            for k in 0..4 {
                b.quat[k] += q_dot[k] * dt;
            }
            let n = (b.quat[0] * b.quat[0]
                + b.quat[1] * b.quat[1]
                + b.quat[2] * b.quat[2]
                + b.quat[3] * b.quat[3])
                .sqrt();
            if n > 1e-12 {
                for q in &mut b.quat {
                    *q /= n;
                }
            }
            // Ground contact on z = 0 plane with restitution + friction.
            let bottom = b.pos[2] - b.half[2];
            if bottom < 0.0 {
                b.pos[2] = b.half[2];
                if b.vel[2] < 0.0 {
                    b.vel[2] = if -b.vel[2] < 0.05 {
                        0.0
                    } else {
                        -b.vel[2] * self.restitution
                    };
                }
                b.vel[0] *= 0.985;
                b.vel[1] *= 0.985;
                for w_k in &mut b.omega {
                    *w_k *= 0.97;
                }
            }
        }
        for f in &mut self.force {
            *f = [0.0; 3];
        }
        for t in &mut self.torque {
            *t = [0.0; 3];
        }
    }

    /// Immutable state access.
    #[inline]
    pub fn get(&self, id: BodyId) -> &RigidState {
        &self.bodies[id]
    }

    /// ZYX Euler angles `(yaw, pitch, roll)` of body `id`.
    pub fn euler(&self, id: BodyId) -> (f64, f64, f64) {
        quat_to_euler(&self.bodies[id].quat)
    }

    /// Body-frame rotation of `v` for body `id`.
    pub fn rotate_to_body(&self, id: BodyId, v_world: &[f64; 3]) -> [f64; 3] {
        quat_rotate_inv(&self.bodies[id].quat, v_world)
    }

    /// World-frame rotation of `v` for body `id`.
    pub fn rotate_to_world(&self, id: BodyId, v_body: &[f64; 3]) -> [f64; 3] {
        quat_rotate(&self.bodies[id].quat, v_body)
    }
}

#[inline]
fn quat_mul_half(q: &[f64; 4], wq: &[f64; 4]) -> [f64; 4] {
    let p = quat_mul(q, wq);
    [0.5 * p[0], 0.5 * p[1], 0.5 * p[2], 0.5 * p[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f64 = 0.002; // 500 Hz fixed timestep

    #[test]
    fn free_fall_matches_kinematics_and_ground_settles() {
        let mut w = World::new([0.0, 0.0, -9.81]);
        let id = w.add_box(1.0, [0.05, 0.05, 0.02], [0.0, 0.0, 3.0], 0.0);

        for _ in 0..250 {
            w.step(DT); // 0.5 s
        }
        let z_05s = w.get(id).pos[2];
        // z ≈ 3.0 − ½·g·t² (semi-implicit: slightly under analytic value).
        let expected = 3.0 - 0.5 * 9.81 * 0.25;
        assert!(
            (z_05s - expected).abs() < 0.05,
            "z={z_05s} vs kinematic {expected}"
        );

        for _ in 0..2000 {
            w.step(DT); // +4 s → must settle on ground
        }
        let s = w.get(id);
        assert!((s.pos[2] - s.half[2]).abs() < 1e-6);
        assert!(s.vel[2].abs() < 1e-9, "must come to rest vertically");
    }

    #[test]
    fn upward_force_climbs_linearly() {
        let mut w = World::new([0.0, 0.0, -9.81]);
        let id = w.add_box(2.0, [0.1, 0.1, 0.03], [0.0, 0.0, 1.0], 0.0);
        for _ in 0..100 {
            w.add_force(id, [0.0, 0.0, 2.0 * 9.81 * 1.5]); // net +½g up
            w.step(DT);
        }
        let s = w.get(id);
        assert!(s.pos[2] > 1.0 && s.vel[2] > 0.0);
    }

    #[test]
    fn torque_spins_about_z_and_quaternion_stays_unit() {
        let mut w = World::new([0.0, 0.0, 0.0]); // gravity off: pure rotation
        let id = w.add_box(1.0, [0.1, 0.1, 0.02], [0.0, 0.0, 5.0], 0.0);
        for _ in 0..500 {
            w.add_torque(id, [0.0, 0.0, 0.01]); // Izz = (0.04+0.04)/3 ≈ .0267
            w.step(DT);
        }
        let s = w.get(id);
        let n: f64 = s.quat.iter().map(|q| q * q).sum();
        assert!((n - 1.0).abs() < 1e-9, "|q|={n}");
        // α = τ/Izz ≈ 0.01/0.00667 ≈ 1.5 rad/s² ⇒ ωz ≈ 1.5 after 1 s.
        assert!(s.omega[2] > 1.2, "should spin up, ωz={}", s.omega[2]);
        let (yaw, _, _) = w.euler(id);
        assert!(yaw.abs() > 0.3, "yaw should accumulate");
    }

    #[test]
    fn determinism_identical_runs_bit_for_bit() {
        let run = || {
            let mut w = World::new([0.0, 0.0, -9.81]);
            let id = w.add_box(1.2, [0.12, 0.12, 0.04], [0.0, 0.0, 0.5], 0.3);
            for i in 0..600u64 {
                w.add_force(id, [(i as f64) * 0.001, 0.0, 11.0]);
                w.add_torque(id, [0.002, -0.001, 0.0005]);
                w.step(DT);
            }
            let s = w.get(id);
            [
                s.pos[0].to_bits(),
                s.pos[1].to_bits(),
                s.pos[2].to_bits(),
                s.quat[0].to_bits(),
                s.vel[2].to_bits(),
            ]
        };
        assert_eq!(
            run(),
            run(),
            "same inputs must produce identical state bits"
        );
    }

    #[test]
    fn rotate_inv_is_inverse_of_rotate() {
        let raw = [0.8f64, 0.1, -0.4, 0.42];
        let n = (raw.iter().map(|q| q * q).sum::<f64>()).sqrt();
        let q = [raw[0] / n, raw[1] / n, raw[2] / n, raw[3] / n]; // unit!
        let v = [1.0, -2.0, 0.5];
        let world = quat_rotate(&q, &v);
        let back = quat_rotate_inv(&q, &world);
        for k in 0..3 {
            assert!((back[k] - v[k]).abs() < 1e-12);
        }
    }
}
