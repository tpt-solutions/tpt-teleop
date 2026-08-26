//! AI training-pipeline export: turn rkyv wire buffers (the bytes logged by
//! the FDR) into feature/label tensors serialized as NumPy `.npy` — the native
//! interchange format for both PyTorch and JAX.
//!
//! The pipeline is fully offline (it runs after a flight, never on the control
//! path): parse the FDR frames, extract numeric features per record, stack them
//! into a `(n_samples, feature_dim)` matrix plus a `(n_samples,)` label vector,
//! and write two `.npy` files. The same `.npy` file is loadable by
//! `torch.from_numpy(np.load('features.npy'))` and by
//! `jnp.asarray(np.load('features.npy'))`, so a single writer serves both
//! frameworks (spec §5.8 "PyTorch-compatible" and "JAX-compatible").

use std::fs::File;
use std::path::Path;

use tpt_t_core::ser::{ControlCommand, FLAG_AI_ORIGIN, GpsSample, ImuSample, TelemetryPacket};

use crate::npy::f32_npy;
use crate::record::{FdrEntry, RecordKind, from_bytes_aligned};

/// One training example: a feature vector and its scalar label.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    /// Input features.
    pub features: Vec<f32>,
    /// Supervised label.
    pub label: f32,
}

/// A stacked dataset ready to export.
#[derive(Debug, Clone)]
pub struct AiDataset {
    /// Number of features per sample (all samples are padded/truncated to this).
    pub feature_dim: usize,
    /// Samples.
    pub samples: Vec<Sample>,
}

/// Extracts a [`Sample`] from a logged payload, or `None` if the payload is
/// too short / unparseable for its kind. Feature semantics are documented per
/// branch; they are deliberately simple, deterministic transforms of the wire
/// data and can be extended without touching the storage format.
pub fn extract_features(kind: RecordKind, payload: &[u8]) -> Option<Sample> {
    match kind {
        RecordKind::Control => {
            let c: ControlCommand = from_bytes_aligned(payload)?;
            let mut f = Vec::with_capacity(8);
            f.push(c.mode as f32);
            f.push((c.flags & FLAG_AI_ORIGIN != 0) as u8 as f32);
            f.extend_from_slice(&c.axes);
            Some(Sample {
                features: f,
                label: c.mode as f32,
            })
        }
        RecordKind::Imu => {
            let s: ImuSample = from_bytes_aligned(payload)?;
            let mut f = Vec::with_capacity(6);
            f.extend_from_slice(&s.gyro_rps);
            f.extend_from_slice(&s.accel_g);
            Some(Sample {
                features: f,
                label: 0.0,
            })
        }
        RecordKind::Gps => {
            let s: GpsSample = from_bytes_aligned(payload)?;
            let f = vec![
                s.lat_deg as f32,
                s.lon_deg as f32,
                s.alt_m as f32,
                s.speed_mps,
                s.course_deg,
                s.sats as f32,
                s.fix_ok as f32,
            ];
            Some(Sample {
                features: f,
                label: s.fix_ok as f32,
            })
        }
        RecordKind::Telemetry => {
            let s: TelemetryPacket = from_bytes_aligned(payload)?;
            let f: Vec<f32> = s.values.to_vec();
            Some(Sample {
                features: f,
                label: s.kind as f32,
            })
        }
        RecordKind::End => None,
    }
}

impl AiDataset {
    /// Builds a dataset from raw FDR entries, padding every feature vector to
    /// the maximum observed dimension.
    pub fn from_entries(entries: &[FdrEntry]) -> Self {
        let mut samples = Vec::new();
        let mut dim = 0usize;
        for e in entries {
            if let Some(kind) = e.kind() {
                if let Some(s) = extract_features(kind, e.payload()) {
                    dim = dim.max(s.features.len());
                    samples.push(s);
                }
            }
        }
        for s in &mut samples {
            s.features.resize(dim, 0.0);
        }
        Self {
            feature_dim: dim,
            samples,
        }
    }

    /// Number of samples.
    pub fn num_samples(&self) -> usize {
        self.samples.len()
    }

    /// Row-major `(n_samples * feature_dim,)` feature matrix.
    pub fn features_matrix(&self) -> Vec<f32> {
        let mut m = Vec::with_capacity(self.samples.len() * self.feature_dim);
        for s in &self.samples {
            m.extend_from_slice(&s.features);
        }
        m
    }

    /// `(n_samples,)` label vector.
    pub fn labels_vec(&self) -> Vec<f32> {
        self.samples.iter().map(|s| s.label).collect()
    }

    /// Writes `features.npy` (`[n, feature_dim]`, `<f4`) and `labels.npy`
    /// (`[n]`, `<f4`) under `dir`. Identical output serves both PyTorch and
    /// JAX (they both load `.npy` via NumPy).
    fn write_npy_pair(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let mut ff = File::create(dir.join("features.npy"))?;
        f32_npy(
            &mut ff,
            &[self.num_samples(), self.feature_dim],
            &self.features_matrix(),
        )?;
        let mut lf = File::create(dir.join("labels.npy"))?;
        f32_npy(&mut lf, &[self.num_samples()], &self.labels_vec())?;
        Ok(())
    }

    /// Exports PyTorch-ready tensors (`.npy` loadable by `torch.from_numpy`).
    pub fn export_pytorch(&self, dir: &Path) -> std::io::Result<()> {
        self.write_npy_pair(dir)
    }

    /// Exports JAX-ready tensors (`.npy` loadable by `jnp.asarray`).
    pub fn export_jax(&self, dir: &Path) -> std::io::Result<()> {
        self.write_npy_pair(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::npy::read_npy;
    use crate::record::parse_entries;
    use tpt_t_core::mode::Mode;

    #[test]
    fn control_features_are_extracted() {
        let c = ControlCommand {
            axes: [0.1, 0.2, 0.3, 0.4, 0.5, 0.6],
            ..ControlCommand::zeroed(Mode::Assist)
        };
        let payload = tpt_t_ring::cast::bytes_of(&c);
        let s = extract_features(RecordKind::Control, payload).unwrap();
        assert_eq!(s.features.len(), 8);
        assert_eq!(s.label, Mode::Assist.as_u8() as f32);
        assert_eq!(&s.features[2..8], &[0.1, 0.2, 0.3, 0.4, 0.5, 0.6]);
    }

    #[test]
    fn dataset_exports_loadable_npy() {
        // Assemble a synthetic FDR stream: 4 control frames.
        let mut stream = Vec::new();
        for i in 0..4u64 {
            let c = ControlCommand {
                axes: [i as f32, 0.0, 0.0, 0.0, 0.0, 0.0],
                ..ControlCommand::zeroed(Mode::FullTeleop)
            };
            let e = FdrEntry::new(RecordKind::Control, tpt_t_ring::cast::bytes_of(&c), i).unwrap();
            stream.extend_from_slice(tpt_t_ring::cast::bytes_of(&e));
        }
        let end = FdrEntry::new(RecordKind::End, &[], 0).unwrap();
        stream.extend_from_slice(tpt_t_ring::cast::bytes_of(&end));

        let entries = parse_entries(&stream);
        let ds = AiDataset::from_entries(&entries);
        assert_eq!(ds.num_samples(), 4);
        assert_eq!(ds.feature_dim, 8);

        let dir = std::env::temp_dir().join("tpt_ai_test");
        let _ = std::fs::create_dir_all(&dir);
        ds.export_pytorch(&dir).unwrap();
        ds.export_jax(&dir).unwrap();

        let fb = std::fs::read(dir.join("features.npy")).unwrap();
        let fv = read_npy(&fb).unwrap();
        assert_eq!(fv.descr, "<f4");
        assert_eq!(fv.shape, vec![4, 8]);

        let lb = std::fs::read(dir.join("labels.npy")).unwrap();
        let lv = read_npy(&lb).unwrap();
        assert_eq!(lv.shape, vec![4]);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
