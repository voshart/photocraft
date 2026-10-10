//! Bounded weak-perspective 2.5D rotation, using an inferred depth surface.
//! Original implementation: no canonical mesh, textures or proprietary assets.
use super::{FaceSliders, LiquifyField};
use photocraft_geom::Rect;
use serde::{Deserialize, Serialize};

const SIDE: usize = 25;
const INVERSE_SIDE: usize = 257;

/// A regular face-depth mesh in document pixels. Z uses the image-width scale,
/// as specified by MediaPipe; smaller Z is closer to the camera.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FaceDepth {
    pub bounds: [f64; 4],
    pub samples: [[f64; SIDE]; SIDE],
}

impl FaceDepth {
    pub fn from_landmarks(points: &[[f64; 3]]) -> Result<Self, String> {
        if points.len() != 478 || points.iter().flatten().any(|v| !v.is_finite() || v.abs() > 1_000_000.0) {
            return Err("Depth needs 478 finite face landmarks".into());
        }
        let outline = [234, 67, 10, 297, 454, 365, 152, 136];
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        let mut back = 0.0;
        for id in outline {
            let p = points.get(id).ok_or("Missing depth landmark")?;
            for axis in 0..2 {
                lo[axis] = lo[axis].min(p[axis]);
                hi[axis] = hi[axis].max(p[axis]);
            }
            back += p[2] / 8.0;
        }
        let width = hi[0] - lo[0];
        if width < 8.0 || hi[1] - lo[1] < 8.0 {
            return Err("Face is too small for head rotation".into());
        }
        // A virtual pivot slightly behind the visible face gives nose/cheek parallax.
        back += width * 0.12;
        let mut out = Self { bounds: [lo[0], lo[1], hi[0], hi[1]], samples: [[0.0; SIDE]; SIDE] };
        let sigma = width * 0.085;
        for (j, row) in out.samples.iter_mut().enumerate() {
            for (i, sample) in row.iter_mut().enumerate() {
                let x = lo[0] + i as f64 / (SIDE - 1) as f64 * width;
                let y = lo[1] + j as f64 / (SIDE - 1) as f64 * (hi[1] - lo[1]);
                let mut sum = 0.0;
                let mut weight = 0.0;
                for p in points.iter().take(468) {
                    let w = (-((p[0] - x).powi(2) + (p[1] - y).powi(2)) / (2.0 * sigma * sigma)).exp();
                    sum += w * (p[2] - back);
                    weight += w;
                }
                if weight < 1e-20 {
                    return Err("Face depth has insufficient coverage".into());
                }
                *sample = sum / weight;
            }
        }
        out.validate()?;
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), String> {
        let b = self.bounds;
        let w = b[2] - b[0];
        let h = b[3] - b[1];
        if b.iter().any(|v| !v.is_finite() || v.abs() > 1_000_000.0)
            || !(8.0..=15000.0).contains(&w)
            || !(8.0..=15000.0).contains(&h)
            || self.samples.iter().flatten().any(|z| !z.is_finite() || z.abs() > w * 2.0)
        {
            return Err("Invalid face depth mesh".into());
        }
        Ok(())
    }

    fn support(&self) -> [f64; 4] {
        let b = self.bounds;
        let x = (b[2] - b[0]) * 0.325;
        let y = (b[3] - b[1]) * 0.325;
        [b[0] - x, b[1] - y, b[2] + x, b[3] + y]
    }

    pub(super) fn validate_grid(&self, canvas: Rect, cell: f64, budget: u64) -> Result<(), String> {
        self.validate()?;
        if !cell.is_finite() || !(1.0..=64.0).contains(&cell) {
            return Err("Invalid face field spacing".into());
        }
        let b = self.support();
        let area = Rect::new(b[0].floor() as i32, b[1].floor() as i32, b[2].ceil() as i32, b[3].ceil() as i32).intersect(&canvas);
        let w = (f64::from(area.width()) / cell).ceil() as u64 + 4;
        let h = (f64::from(area.height()) / cell).ceil() as u64 + 4;
        if w.saturating_mul(h) > budget {
            return Err("Head rotation exceeds the face interaction budget".into());
        }
        Ok(())
    }

    fn depth(&self, p: [f64; 2]) -> f64 {
        let b = self.bounds;
        let x = ((p[0] - b[0]) / (b[2] - b[0]) * (SIDE - 1) as f64).clamp(0.0, (SIDE - 1) as f64);
        let y = ((p[1] - b[1]) / (b[3] - b[1]) * (SIDE - 1) as f64).clamp(0.0, (SIDE - 1) as f64);
        let i = (x as usize).min(SIDE - 2);
        let j = (y as usize).min(SIDE - 2);
        let at = |a: usize, b: usize| self.samples.get(b).and_then(|r| r.get(a)).copied().unwrap_or(0.0);
        let fx = x - i as f64;
        let fy = y - j as f64;
        (at(i, j) * (1.0 - fx) + at(i + 1, j) * fx) * (1.0 - fy) + (at(i, j + 1) * (1.0 - fx) + at(i + 1, j + 1) * fx) * fy
    }

    /// Smoothly anchor the surrounding hair/background. Zero angles are bit-exact identity.
    pub fn project(&self, p: [f64; 2], yaw: f64, pitch: f64) -> [f64; 2] {
        if yaw == 0.0 && pitch == 0.0 {
            return p;
        }
        let (sy, cy) = yaw.to_radians().sin_cos();
        let (sp, cp) = pitch.to_radians().sin_cos();
        self.project_rotated(p, [sy, cy, sp, cp])
    }

    fn project_rotated(&self, p: [f64; 2], rotation: [f64; 4]) -> [f64; 2] {
        let b = self.bounds;
        let c = [(b[0] + b[2]) * 0.5, (b[1] + b[3]) * 0.5];
        let x = p[0] - c[0];
        let y = p[1] - c[1];
        let radius = (x / ((b[2] - b[0]) * 0.5)).hypot(y / ((b[3] - b[1]) * 0.5));
        if radius >= 1.65 {
            return p;
        }
        let t = ((radius - 0.85) / 0.8).clamp(0.0, 1.0);
        let feather = 1.0 - t * t * (3.0 - 2.0 * t);
        let z = self.depth(p);
        let [sy, cy, sp, cp] = rotation;
        let rx = x * cy - z * sy;
        let rz = x * sy + z * cy;
        let ry = y * cp + rz * sp;
        [p[0] + (rx - x) * feather, p[1] + (ry - y) * feather]
    }
}

pub(super) struct PoseMap {
    bounds: [f64; 4],
    inverse: Vec<[f64; 2]>,
}

impl PoseMap {
    pub(super) fn prepare(depth: &FaceDepth, s: &FaceSliders) -> Result<Option<Self>, String> {
        s.validate()?;
        depth.validate()?;
        if s.head_yaw == 0.0 && s.head_pitch == 0.0 {
            return Ok(None);
        }
        let bounds = depth.support();
        let epsilon = (depth.bounds[2] - depth.bounds[0]) * 0.0001;
        let (sy, cy) = s.head_yaw.to_radians().sin_cos();
        let (sp, cp) = s.head_pitch.to_radians().sin_cos();
        let project = |p| depth.project_rotated(p, [sy, cy, sp, cp]);
        let jacobian = |p: [f64; 2]| {
            let x0 = project([p[0] - epsilon, p[1]]);
            let x1 = project([p[0] + epsilon, p[1]]);
            let y0 = project([p[0], p[1] - epsilon]);
            let y1 = project([p[0], p[1] + epsilon]);
            [(x1[0] - x0[0]) / (2.0 * epsilon), (y1[0] - y0[0]) / (2.0 * epsilon), (x1[1] - x0[1]) / (2.0 * epsilon), (y1[1] - y0[1]) / (2.0 * epsilon)]
        };
        let mut inverse = Vec::with_capacity(INVERSE_SIDE * INVERSE_SIDE);
        for j in 0..INVERSE_SIDE {
            for i in 0..INVERSE_SIDE {
                let p = [
                    bounds[0] + i as f64 / (INVERSE_SIDE - 1) as f64 * (bounds[2] - bounds[0]),
                    bounds[1] + j as f64 / (INVERSE_SIDE - 1) as f64 * (bounds[3] - bounds[1]),
                ];
                let mut q = p;
                let [a, b, c, d] = jacobian(p);
                if a * d - b * c < 0.12 {
                    return Err("This head turn would fold the face. Reduce the angle.".into());
                }
                let mut converged = false;
                for _ in 0..16 {
                    let f = project(q);
                    let e = [f[0] - p[0], f[1] - p[1]];
                    let [a, b, c, d] = jacobian(q);
                    let det = a * d - b * c;
                    if !det.is_finite() || det < 0.12 {
                        return Err("This head turn would fold the face. Use a smaller angle or a more frontal photo.".into());
                    }
                    if e[0].hypot(e[1]) < 0.0001 {
                        converged = true;
                        break;
                    }
                    let step = [(d * e[0] - b * e[1]) / det, (-c * e[0] + a * e[1]) / det];
                    q = [q[0] - step[0], q[1] - step[1]];
                    if q.iter().any(|v| !v.is_finite() || v.abs() > 2_000_000.0) {
                        break;
                    }
                }
                if !converged {
                    return Err("Head rotation could not be inverted safely. Reduce the angle.".into());
                }
                inverse.push([q[0] - p[0], q[1] - p[1]]);
            }
        }
        Ok(Some(Self { bounds, inverse }))
    }

    fn sample(&self, p: [f64; 2]) -> [f64; 2] {
        let b = self.bounds;
        if p[0] <= b[0] || p[1] <= b[1] || p[0] >= b[2] || p[1] >= b[3] {
            return [0.0; 2];
        }
        let x = (p[0] - b[0]) / (b[2] - b[0]) * (INVERSE_SIDE - 1) as f64;
        let y = (p[1] - b[1]) / (b[3] - b[1]) * (INVERSE_SIDE - 1) as f64;
        let i = (x as usize).min(INVERSE_SIDE - 2);
        let j = (y as usize).min(INVERSE_SIDE - 2);
        let at = |a, b| self.inverse.get(b * INVERSE_SIDE + a).copied().unwrap_or([0.0; 2]);
        let fx = x - i as f64;
        let fy = y - j as f64;
        let [a, b, c, d] = [at(i, j), at(i + 1, j), at(i, j + 1), at(i + 1, j + 1)];
        std::array::from_fn(|k| (a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy)
    }

    pub(super) fn apply(&self, field: &mut LiquifyField, budget: u64) -> Result<Rect, String> {
        if field.w < 2
            || field.h < 2
            || field.w.checked_mul(field.h) != Some(field.d.len())
            || field.freeze.len() != field.d.len()
            || field.d.len() as u64 > budget
            || !field.cell.is_finite()
            || !(1.0..=64.0).contains(&field.cell)
        {
            return Err("Invalid or oversized head-rotation field".into());
        }
        let b = self.bounds;
        let area = Rect::new(b[0].floor() as i32, b[1].floor() as i32, b[2].ceil() as i32, b[3].ceil() as i32).intersect(&field.bounds);
        let row = |j| -> Result<Vec<(usize, [f32; 2])>, String> {
            let mut writes = Vec::new();
            for i in 0..field.w {
                let p = field.node_pos(i, j);
                let delta = self.sample(p);
                if delta == [0.0; 2] {
                    continue;
                }
                let k = (1.0 - f64::from(field.freeze_at(p[0], p[1]))).clamp(0.0, 1.0);
                if k == 0.0 {
                    continue;
                }
                let q = [p[0] + delta[0] * k, p[1] + delta[1] * k];
                let old = field.sample(q[0], q[1]);
                let value = [(q[0] + old[0] - p[0]) as f32, (q[1] + old[1] - p[1]) as f32];
                if value.iter().any(|v| !v.is_finite()) {
                    return Err("Invalid head-rotation displacement".into());
                }
                writes.push((j * field.w + i, value));
            }
            Ok(writes)
        };
        #[cfg(not(target_arch = "wasm32"))]
        let rows: Vec<_> = {
            use rayon::prelude::*;
            (0..field.h).into_par_iter().map(row).collect()
        };
        #[cfg(target_arch = "wasm32")]
        let rows: Vec<_> = (0..field.h).map(row).collect();
        let rows: Vec<_> = rows.into_iter().collect::<Result<_, _>>()?;
        if rows.iter().all(Vec::is_empty) {
            return Ok(Rect::EMPTY);
        }
        for (index, value) in rows.into_iter().flatten() {
            if let Some(slot) = field.d.get_mut(index) {
                *slot = value;
            }
        }
        Ok(area.inflate((field.cell * 2.0).ceil() as i32 + 2).intersect(&field.bounds))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn depth() -> FaceDepth {
        let mut d = FaceDepth { bounds: [50., 40., 250., 270.], samples: [[0.0; SIDE]; SIDE] };
        for (j, row) in d.samples.iter_mut().enumerate() {
            for (i, z) in row.iter_mut().enumerate() {
                let r = ((i as f64 - 12.) / 12.).hypot((j as f64 - 12.) / 12.);
                *z = -20. - 40. * (1. - r).max(0.);
            }
        }
        d
    }
    #[test]
    fn depth_turn_has_parallax_inverse_and_fixed_background() {
        let d = depth();
        assert_eq!(d.project([150., 155.], 0., 0.), [150., 155.]);
        let nose = d.project([150., 155.], 15., 0.);
        let cheek = d.project([150., 240.], 15., 0.);
        assert!(nose[0] - 150. > cheek[0] - 150.);
        assert_eq!(d.project([0., 0.], 30., 0.), [0., 0.]);
        for yaw in [-30., -15., 15., 30.] {
            let map = PoseMap::prepare(&d, &FaceSliders { head_yaw: yaw, ..Default::default() }).unwrap().unwrap();
            for p in [[150., 155.], [130., 100.], [190., 210.]] {
                let target = d.project(p, yaw, 0.);
                let inv = map.sample(target);
                assert!((target[0] + inv[0] - p[0]).hypot(target[1] + inv[1] - p[1]) < 0.15);
            }
        }
    }
    #[test]
    fn depth_replay_limits_and_failures_are_transactional() {
        let mut face = super::super::tests::synthetic_face();
        face.depth = Some(Box::new(depth()));
        let mut field = LiquifyField::new(Rect::new(0, 0, 300, 300), 2.);
        face.apply(&mut field).unwrap();
        assert!(field.is_identity());
        face.sliders.head_yaw = 15.;
        face.sliders.smile = 30.;
        face.apply(&mut field).unwrap();
        assert!(!field.is_identity());
        let encoded = serde_json::to_string(&face).unwrap();
        let restored: super::super::FaceWarp = serde_json::from_str(&encoded).unwrap();
        let mut replay = LiquifyField::new(field.bounds, field.cell);
        restored.apply(&mut replay).unwrap();
        assert_eq!(field, replay);
        let old = field.clone();
        for (yaw, pitch) in [(30.1, 0.), (25., 25.), (f64::NAN, 0.)] {
            face.sliders.head_yaw = yaw;
            face.sliders.head_pitch = pitch;
            assert!(face.apply(&mut field).is_err());
            assert_eq!(old, field);
        }
        face.sliders.head_yaw = 15.;
        face.sliders.head_pitch = 0.;
        face.depth.as_mut().unwrap().samples[12][12] = f64::NAN;
        assert!(face.apply(&mut field).is_err());
        assert_eq!(old, field);
        face.depth = None;
        assert!(face.apply(&mut field).is_err());
    }
    #[test]
    fn folded_depth_is_rejected_and_frozen_photo_stays_exact() {
        let mut face = super::super::tests::synthetic_face();
        face.depth = Some(Box::new(depth()));
        face.sliders.head_yaw = 15.;
        let mut field = LiquifyField::new(Rect::new(0, 0, 300, 300), 2.);
        field.freeze.fill(1.0);
        let old = field.clone();
        assert!(face.apply(&mut field).unwrap().is_empty());
        assert_eq!(old, field);
        let depth = face.depth.as_mut().unwrap();
        for row in &mut depth.samples {
            for (i, z) in row.iter_mut().enumerate() {
                *z = if i % 2 == 0 { 150. } else { -150. };
            }
        }
        field.freeze.fill(0.0);
        let old = field.clone();
        assert!(face.apply(&mut field).is_err());
        assert_eq!(old, field);
    }
}
