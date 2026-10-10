// SPDX-License-Identifier: MIT OR Apache-2.0
//! Manual region controls, version 1. This is an interpolating geometric warp, not
//! a detector, a skin simulation or a promise of physically correct expressions.
//! The inverse map interpolates target -> source correspondences with compact
//! radial basis functions. It composes with the existing field and never resamples
//! the previous preview. Saved operations are independent of model availability.

use super::LiquifyField;
use photocraft_geom::Rect;
use serde::{Deserialize, Serialize};

pub const HANDLE_COUNT: usize = 8;
pub const MAX_EDIT_NODES: u64 = 500_000;
const MAX_SUBSTEPS: usize = 64;
const BASIS_RADIUS: f64 = 0.8;
const MIN_SEPARATION: f64 = 0.002;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RegionKind {
    Eye,
    Mouth,
}

/// Normalized local coordinates run from -1 to +1 over this ellipse. The named
/// payload is intentionally distinct from legacy brush point lists.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RigWarp {
    pub kind: RegionKind,
    /// [left, top, right, bottom] in document pixels, including skin transition.
    pub bounds: [f64; 4],
    /// Contour order: left corner, upper left/middle/right, right corner,
    /// lower right/middle/left. Fixed-size arrays bound allocations on decode.
    pub source: [[f64; 2]; HANDLE_COUNT],
    pub target: [[f64; 2]; HANDLE_COUNT],
    pub pinned: [bool; HANDLE_COUNT],
}

fn norm(p: [f64; 2], b: [f64; 4]) -> [f64; 2] {
    [(p[0] - (b[0] + b[2]) * 0.5) / ((b[2] - b[0]) * 0.5), (p[1] - (b[1] + b[3]) * 0.5) / ((b[3] - b[1]) * 0.5)]
}
fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Compact radial kernel in 2D; its value and first derivatives
/// vanish at support. Version 1 fixes its radius/profile for reproducible replay.
fn basis(r: f64) -> f64 {
    if !r.is_finite() || r >= 1.0 {
        return 0.0;
    }
    let u = 1.0 - r.max(0.0);
    u * u * u * u * (4.0 * r.max(0.0) + 1.0)
}
fn envelope(p: [f64; 2]) -> f64 {
    super::precision::smooth_falloff(p[0].hypot(p[1]), 1.0 - 0.7 / 0.85)
}

impl RigWarp {
    pub fn new(kind: RegionKind, bounds: [f64; 4]) -> Result<Self, String> {
        if !bounds.iter().all(|v| v.is_finite() && v.abs() <= 1_000_000.0) || bounds[2] - bounds[0] < 16.0 || bounds[3] - bounds[1] < 16.0 {
            return Err("Draw a region at least 16 document pixels wide and high".into());
        }
        let y = match kind {
            RegionKind::Eye => 0.23,
            RegionKind::Mouth => 0.16,
        };
        let points = [[-0.58, 0.0], [-0.3, -y * 0.75], [0.0, -y], [0.3, -y * 0.75], [0.58, 0.0], [0.3, y * 0.65], [0.0, y], [-0.3, y * 0.65]];
        let source = points.map(|p| {
            [(bounds[0] + bounds[2]) * 0.5 + p[0] * (bounds[2] - bounds[0]) * 0.5, (bounds[1] + bounds[3]) * 0.5 + p[1] * (bounds[3] - bounds[1]) * 0.5]
        });
        let rig = Self { kind, bounds, source, target: source, pinned: [false; HANDLE_COUNT] };
        rig.validate()?;
        Ok(rig)
    }

    pub fn validate(&self) -> Result<(), String> {
        let b = self.bounds;
        if !b.iter().all(|v| v.is_finite() && v.abs() <= 1_000_000.0)
            || !(16.0..=15_000.0).contains(&(b[2] - b[0]))
            || !(16.0..=15_000.0).contains(&(b[3] - b[1]))
        {
            return Err("Invalid rig bounds (16..15000 document pixels per side)".into());
        }
        for (i, (s, t)) in self.source.iter().zip(&self.target).enumerate() {
            if s.iter().chain(t).any(|v| !v.is_finite()) {
                return Err("Rig coordinates must be finite".into());
            }
            if [*s, *t].iter().any(|p| {
                let q = norm(*p, b);
                q[0].hypot(q[1]) > 0.88
            }) {
                return Err("Keep handles within the guide's inner ellipse; place a larger region for this edit".into());
            }
            if self.pinned[i] && s != t {
                return Err("A pinned handle cannot be moved".into());
            }
            for other in self.target.iter().take(i) {
                if distance(norm(*t, b), norm(*other, b)) < MIN_SEPARATION {
                    return Err("Handles are too close for a stable inverse map; leave a small opening".into());
                }
            }
        }
        Ok(())
    }

    pub fn validate_grid(&self, canvas: Rect, cell: f64) -> Result<(), String> {
        self.validate_grid_with_budget(canvas, cell, MAX_EDIT_NODES)
    }

    pub(super) fn validate_grid_with_budget(&self, canvas: Rect, cell: f64, node_budget: u64) -> Result<(), String> {
        self.validate()?;
        if !cell.is_finite() || !(1.0..=64.0).contains(&cell) {
            return Err("Invalid field spacing".into());
        }
        let area = self.rect().intersect(&canvas);
        let w = (f64::from(area.width()) / cell).ceil() as u64 + 4;
        let h = (f64::from(area.height()) / cell).ceil() as u64 + 4;
        let budget = node_budget.min(4_000_000);
        if w.saturating_mul(h) > budget {
            return Err(format!("Region exceeds the {budget}-node interaction budget; use a smaller image"));
        }
        Ok(())
    }

    pub fn rect(&self) -> Rect {
        Rect::new(self.bounds[0].floor() as i32, self.bounds[1].floor() as i32, self.bounds[2].ceil() as i32, self.bounds[3].ceil() as i32)
    }
    pub fn is_identity(&self) -> bool {
        self.source == self.target
    }
    /// Start the next gesture at the accepted current position, not at the last preview.
    pub fn next_gesture(&self) -> Self {
        let mut r = self.clone();
        r.source = r.target;
        r
    }

    /// Coordinated closure in the user-corrected guide frame. Leaves a small
    /// nonzero opening: fully coincident inverse controls are not well defined.
    pub fn close_eye(&self, fraction: f64) -> Result<Self, String> {
        if self.kind != RegionKind::Eye || !fraction.is_finite() {
            return Err("Closure needs an eye rig".into());
        }
        let mut r = self.next_gesture();
        let f = fraction.clamp(0.0, 0.95);
        for (u, l) in [(1, 7), (2, 6), (3, 5)] {
            let gap = [r.source[l][0] - r.source[u][0], r.source[l][1] - r.source[u][1]];
            for (axis, delta) in gap.into_iter().enumerate() {
                if !r.pinned[u] {
                    r.target[u][axis] += delta * f * 0.8;
                }
                if !r.pinned[l] {
                    r.target[l][axis] -= delta * f * 0.2;
                }
            }
        }
        r.validate()?;
        Ok(r)
    }

    /// Raise a mouth corner in the frame defined by the two corners. Side is
    /// explicitly image-left/right, not the subject's anatomical left/right.
    pub fn smirk(&self, image_right: bool) -> Result<Self, String> {
        if self.kind != RegionKind::Mouth {
            return Err("Smirk needs a mouth rig".into());
        }
        let mut r = self.next_gesture();
        let along = [r.source[4][0] - r.source[0][0], r.source[4][1] - r.source[0][1]];
        let len = along[0].hypot(along[1]);
        if len < 1.0 {
            return Err("Separate the mouth corners first".into());
        }
        let up = [along[1] / len, -along[0] / len];
        let ids = if image_right { [4, 3, 5] } else { [0, 1, 7] };
        for (i, amount) in ids.into_iter().zip([0.07, 0.035, 0.035]) {
            if r.pinned[i] {
                continue;
            }
            r.target[i][0] += up[0] * len * amount;
            r.target[i][1] += up[1] * len * amount;
        }
        r.validate()?;
        Ok(r)
    }
}

/// Small, fixed-size Cholesky solve prepared once per gesture update. No matrix
/// inverse per pixel and no inference/dependency added to the core application.
pub struct Prepared {
    bounds: [f64; 4],
    centers: [[f64; 2]; HANDLE_COUNT],
    coefficients: [[f64; 2]; HANDLE_COUNT],
}
impl Prepared {
    pub fn new(rig: &RigWarp) -> Result<Self, String> {
        rig.validate()?;
        let centers = rig.target.map(|p| norm(p, rig.bounds));
        let mut l = [[0.0f64; HANDLE_COUNT]; HANDLE_COUNT];
        for i in 0..HANDLE_COUNT {
            for j in 0..=i {
                let mut v = basis(distance(centers[i], centers[j]) / BASIS_RADIUS);
                for (a, b) in l[i].iter().zip(&l[j]).take(j) {
                    v -= a * b;
                }
                if i == j {
                    if !v.is_finite() || v < 1e-10 {
                        return Err("Unstable guide geometry; separate or reset handles".into());
                    }
                    l[i][j] = v.sqrt();
                } else {
                    l[i][j] = v / l[j][j];
                }
            }
        }
        let mut coefficients = [[0.0; 2]; HANDLE_COUNT];
        for axis in [0, 1] {
            let mut y = [0.0; HANDLE_COUNT];
            for i in 0..HANDLE_COUNT {
                let mut b = (rig.source[i][axis] - rig.target[i][axis]) / envelope(centers[i]);
                for k in 0..i {
                    b -= l[i][k] * y[k];
                }
                y[i] = b / l[i][i];
            }
            for i in (0..HANDLE_COUNT).rev() {
                let mut b = y[i];
                for k in i + 1..HANDLE_COUNT {
                    b -= l[k][i] * coefficients[k][axis];
                }
                coefficients[i][axis] = b / l[i][i];
                if !coefficients[i][axis].is_finite() {
                    return Err("Rig solve produced a non-finite displacement".into());
                }
            }
        }
        Ok(Self { bounds: rig.bounds, centers, coefficients })
    }

    pub fn displacement(&self, p: [f64; 2]) -> [f64; 2] {
        let n = norm(p, self.bounds);
        let e = envelope(n);
        if e == 0.0 {
            return [0.0; 2];
        }
        let mut d = [0.0; 2];
        for (c, k) in self.centers.iter().zip(self.coefficients) {
            let w = e * basis(distance(n, *c) / BASIS_RADIUS);
            d[0] += w * k[0];
            d[1] += w * k[1];
        }
        d
    }
}

/// Large closure cannot safely be represented by one interpolant: exact handles
/// alone do not imply a non-folding map. Compose small maps, each moving controls
/// by at most 20% of the smallest endpoint separation. This is an empirically
/// tested bound, NOT a proof of a positive Jacobian for every possible guide.
pub fn prepare_steps(rig: &RigWarp) -> Result<Vec<Prepared>, String> {
    rig.validate()?;
    let source = rig.source.map(|p| norm(p, rig.bounds));
    let target = rig.target.map(|p| norm(p, rig.bounds));
    let mut movement = 0.0f64;
    let mut separation = f64::INFINITY;
    for i in 0..HANDLE_COUNT {
        movement = movement.max(distance(source[i], target[i]));
        for j in 0..i {
            separation = separation.min(distance(source[i], source[j])).min(distance(target[i], target[j]));
        }
    }
    if separation < MIN_SEPARATION || !separation.is_finite() {
        return Err("Separate the guides before deformation".into());
    }
    let count = (movement / (0.2 * separation)).ceil().max(1.0) as usize;
    if count > MAX_SUBSTEPS {
        return Err("This gesture needs too many deformation steps; leave a larger opening or use a smaller adjustment".into());
    }
    let mut steps = Vec::with_capacity(count);
    for k in 0..count {
        let mut part = rig.clone();
        for i in 0..HANDLE_COUNT {
            for axis in 0..2 {
                let delta = rig.target[i][axis] - rig.source[i][axis];
                part.source[i][axis] = rig.source[i][axis] + delta * (k as f64 / count as f64);
                part.target[i][axis] = rig.source[i][axis] + delta * ((k + 1) as f64 / count as f64);
            }
        }
        steps.push(Prepared::new(&part)?);
    }
    Ok(steps)
}

/// Apply to geometry only. All reads use the unchanged incoming field. Writes
/// are staged locally; a preparation/validation failure leaves it untouched.
pub fn apply(field: &mut LiquifyField, rig: &RigWarp) -> Result<Rect, String> {
    apply_with_budget(field, rig, MAX_EDIT_NODES)
}

pub(super) fn apply_with_budget(field: &mut LiquifyField, rig: &RigWarp, node_budget: u64) -> Result<Rect, String> {
    rig.validate_grid_with_budget(field.bounds, field.cell, node_budget)?;
    if field.w < 2 || field.h < 2 || field.w.checked_mul(field.h) != Some(field.d.len()) || field.freeze.len() != field.d.len() {
        return Err("Inconsistent displacement field storage".into());
    }
    if rig.is_identity() {
        return Ok(Rect::EMPTY);
    }
    let steps = prepare_steps(rig)?;
    let area = rig.rect().intersect(&field.bounds);
    if area.is_empty() {
        return Ok(Rect::EMPTY);
    }
    let index = |p: i32, origin: i32| (f64::from(p) - f64::from(origin) - 0.5) / field.cell;
    let i0 = index(area.x0, field.bounds.x0).floor().max(0.0) as usize;
    let j0 = index(area.y0, field.bounds.y0).floor().max(0.0) as usize;
    let i1 = (index(area.x1, field.bounds.x0).ceil() as usize + 1).min(field.w);
    let j1 = (index(area.y1, field.bounds.y0).ceil() as usize + 1).min(field.h);
    let row = |j| -> Result<Vec<(usize, [f32; 2])>, String> {
        let mut writes = Vec::new();
        for i in i0..i1 {
            let p = field.node_pos(i, j);
            let idx = j * field.w + i;
            let mut q = p;
            // Inverse maps compose in reverse gesture order. Evaluate analytically
            // then sample the incoming displacement once: no field resampling for
            // every substep and no repeated raster resampling.
            for step in steps.iter().rev() {
                let k = (1.0 - f64::from(field.freeze_at(q[0], q[1]))).clamp(0.0, 1.0);
                if k == 0.0 {
                    continue;
                }
                let delta = step.displacement(q);
                q = [q[0] + k * delta[0], q[1] + k * delta[1]];
            }
            if q == p {
                continue;
            }
            let old = field.sample(q[0], q[1]);
            let value = [(q[0] + old[0] - p[0]) as f32, (q[1] + old[1] - p[1]) as f32];
            if value.iter().any(|v| !v.is_finite()) {
                return Err("Non-finite rig field; edit not applied".into());
            }
            if field.d.get(idx).is_some_and(|old| *old != value) {
                writes.push((idx, value));
            }
        }
        Ok(writes)
    };
    #[cfg(not(target_arch = "wasm32"))]
    let rows: Vec<_> = {
        use rayon::prelude::*;
        (j0..j1).into_par_iter().map(row).collect()
    };
    #[cfg(target_arch = "wasm32")]
    let rows: Vec<_> = (j0..j1).map(row).collect();
    // All rows validate before any field mutation, preserving transactional replay.
    let rows: Vec<_> = rows.into_iter().collect::<Result<_, _>>()?;
    if rows.iter().all(Vec::is_empty) {
        return Ok(Rect::EMPTY);
    }
    for (i, value) in rows.into_iter().flatten() {
        if let Some(slot) = field.d.get_mut(i) {
            *slot = value;
        }
    }
    // Include interpolation influence beyond the node footprint.
    let pad = (field.cell * 2.0).ceil() as i32 + 2;
    Ok(area.inflate(pad).intersect(&field.bounds))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_and_handle_correspondences() {
        let mut r = RigWarp::new(RegionKind::Eye, [0.0, 0.0, 300.0, 180.0]).unwrap();
        let identity = Prepared::new(&r).unwrap();
        assert_eq!(identity.displacement([150.0, 90.0]), [0.0, 0.0]);
        r.target[2][1] += 10.0;
        r.pinned[0] = true;
        let p = Prepared::new(&r).unwrap();
        for (s, t) in r.source.iter().zip(&r.target) {
            let d = p.displacement(*t);
            assert!((t[0] + d[0] - s[0]).abs() < 1e-7);
            assert!((t[1] + d[1] - s[1]).abs() < 1e-7);
        }
        assert_eq!(p.displacement([0.0, 0.0]), [0.0; 2]);
        assert_eq!(p.displacement([400.0, 90.0]), [0.0; 2]);
    }
    #[test]
    fn invalid_and_pinned_operations_are_errors() {
        let mut r = RigWarp::new(RegionKind::Eye, [0.0, 0.0, 300.0, 180.0]).unwrap();
        r.pinned[2] = true;
        r.target[2][1] += 1.0;
        assert!(r.validate().is_err());
        r.pinned[2] = false;
        r.target[2] = r.target[1];
        assert!(r.validate().is_err());
        r.target[2][0] = f64::NAN;
        assert!(r.validate().is_err());
    }
    #[test]
    fn closure_and_smirk_are_finite_and_one_sided() {
        let eye = RigWarp::new(RegionKind::Eye, [0.0, 0.0, 300.0, 180.0]).unwrap();
        let wink = eye.close_eye(0.85).unwrap();
        assert_eq!(wink.source[0], wink.target[0]);
        assert_eq!(wink.source[4], wink.target[4]);
        assert!(Prepared::new(&wink).is_ok());
        let mouth = RigWarp::new(RegionKind::Mouth, [0.0, 0.0, 300.0, 180.0]).unwrap();
        let smirk = mouth.smirk(false).unwrap();
        assert_eq!(smirk.source[4], smirk.target[4]);
        assert!(smirk.target[0][1] < smirk.source[0][1]);
    }
    #[test]
    fn freeze_and_replay_use_the_shared_field() {
        let bounds = Rect::new(0, 0, 300, 180);
        let rig = RigWarp::new(RegionKind::Eye, [0.0, 0.0, 300.0, 180.0]).unwrap().close_eye(0.6).unwrap();
        let mut a = LiquifyField::new(bounds, 2.0);
        a.freeze.fill(1.0);
        apply(&mut a, &rig).unwrap();
        assert!(a.is_identity());
        a.freeze.fill(0.0);
        apply(&mut a, &rig).unwrap();
        assert!(!a.is_identity());
        let mut b = LiquifyField::new(bounds, 2.0);
        apply(&mut b, &rig).unwrap();
        assert_eq!(a, b);
        let mut stroke = super::super::LiquifyStroke::new(super::super::LiquifyTool::RigWarpV1, 1.0);
        stroke.rig = Some(rig);
        assert_eq!(LiquifyField::from_strokes(bounds, 2.0, &[stroke]), a);
    }
    #[test]
    fn composed_wink_interpolates_handles_and_has_positive_sampled_jacobian() {
        let r = RigWarp::new(RegionKind::Eye, [0.0, 0.0, 300.0, 180.0]).unwrap().close_eye(0.85).unwrap();
        let steps = prepare_steps(&r).unwrap();
        assert!(steps.len() > 1);
        let map = |mut q: [f64; 2]| {
            for step in steps.iter().rev() {
                let d = step.displacement(q);
                q = [q[0] + d[0], q[1] + d[1]];
            }
            q
        };
        for (s, t) in r.source.iter().zip(&r.target) {
            let q = map(*t);
            assert!(distance(q, *s) < 1e-6);
        }
        // Regression on this synthetic fixture, not a general injectivity proof.
        let e = 0.01;
        for y in (10..180).step_by(10) {
            for x in (10..300).step_by(10) {
                let p = [f64::from(x), f64::from(y)];
                let l = map([p[0] - e, p[1]]);
                let rr = map([p[0] + e, p[1]]);
                let t = map([p[0], p[1] - e]);
                let b = map([p[0], p[1] + e]);
                let det = ((rr[0] - l[0]) * (b[1] - t[1]) - (rr[1] - l[1]) * (b[0] - t[0])) / (4.0 * e * e);
                assert!(det > 0.0, "fold at {p:?}: {det}");
            }
        }
    }
    #[test]
    fn extreme_substep_count_returns_an_error() {
        let r = RigWarp::new(RegionKind::Eye, [0.0, 0.0, 300.0, 180.0]).unwrap().close_eye(0.95).unwrap();
        assert!(prepare_steps(&r).is_err());
    }
}

#[cfg(test)]
mod no_movement_tests {
    use super::*;

    #[test]
    fn a_fully_frozen_rig_returns_empty_damage_and_identical_storage() {
        let bounds = Rect::new(0, 0, 300, 180);
        let mut field = LiquifyField::new(bounds, 2.0);
        field.freeze.fill(1.0);
        let before = field.clone();
        let rig = RigWarp::new(RegionKind::Eye, [0.0, 0.0, 300.0, 180.0]).unwrap().close_eye(0.25).unwrap();
        assert_eq!(apply(&mut field, &rig).unwrap(), Rect::EMPTY);
        assert_eq!(field, before);
    }
}
