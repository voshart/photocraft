//! Absolute, model-independent face sliders. Detection only supplies the initial contours.
//! Saved edits contain five bounded regions and numbers, so replay needs no model or runtime.

use super::{
    LiquifyField,
    rig::{RegionKind, RigWarp, prepare_steps},
};
use photocraft_geom::Rect;
use serde::{Deserialize, Serialize};

#[path = "liquify_face_pose.rs"]
pub mod pose;

/// Whole-face edits can span a 24?36 MP photo; bounded by the session field budget.
const MAX_FACE_NODES: u64 = 4_000_000;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct FaceSliders {
    /// Relative weak-perspective head turn, in degrees. Combined turn is capped at 30°.
    pub head_yaw: f64,
    pub head_pitch: f64,
    pub eye_size: f64,
    pub eye_width: f64,
    pub eye_height: f64,
    pub eye_spacing: f64,
    pub eye_tilt: f64,
    pub right_eye_size: Option<f64>,
    pub right_eye_width: Option<f64>,
    pub right_eye_height: Option<f64>,
    pub right_eye_tilt: Option<f64>,
    /// Left and right always mean the sides of the displayed photo.
    pub left_eye_close: f64,
    pub right_eye_close: f64,
    pub mouth_width: f64,
    pub mouth_height: f64,
    pub upper_lip: f64,
    pub lower_lip: f64,
    pub smile: f64,
    pub nose_width: f64,
    pub nose_height: f64,
    pub face_width: f64,
    pub chin_height: f64,
    pub forehead: f64,
    pub jaw_width: f64,
}

impl FaceSliders {
    pub fn validate(&self) -> Result<(), String> {
        if !self.head_yaw.is_finite() || !self.head_pitch.is_finite() || self.head_yaw.hypot(self.head_pitch) > 30.0 + 1e-9 {
            return Err("Head turn must be finite and within 30 degrees in total".into());
        }
        let values = [
            self.eye_size,
            self.eye_width,
            self.eye_height,
            self.eye_spacing,
            self.eye_tilt,
            self.mouth_width,
            self.mouth_height,
            self.smile,
            self.nose_width,
            self.nose_height,
            self.face_width,
            self.chin_height,
            self.forehead,
            self.jaw_width,
            self.upper_lip,
            self.lower_lip,
        ];
        if [self.right_eye_size, self.right_eye_width, self.right_eye_height, self.right_eye_tilt]
            .into_iter()
            .flatten()
            .any(|v| !v.is_finite() || !(-100.0..=100.0).contains(&v))
        {
            return Err("Eye sliders must be finite and within -100..100".into());
        }
        if values.iter().any(|v| !v.is_finite() || !(-100.0..=100.0).contains(v))
            || [self.left_eye_close, self.right_eye_close].iter().any(|v| !v.is_finite() || !(0.0..=100.0).contains(v))
        {
            return Err("Face sliders must be finite and within their displayed ranges".into());
        }
        Ok(())
    }
}

/// Image-left eye, image-right eye, mouth, nose, face outline. Fixed count bounds decoding.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FaceWarp {
    pub regions: [RigWarp; 5],
    pub sliders: FaceSliders,
    /// Absent on older, 2D-only saved edits. A fixed-size depth surface bounds decoding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<Box<pose::FaceDepth>>,
}

fn region(kind: RegionKind, points: [[f64; 2]; 8]) -> Result<RigWarp, String> {
    let min = points.iter().fold([f64::INFINITY; 2], |a, p| [a[0].min(p[0]), a[1].min(p[1])]);
    let max = points.iter().fold([f64::NEG_INFINITY; 2], |a, p| [a[0].max(p[0]), a[1].max(p[1])]);
    let center = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
    // Enough room for both the contour and smooth skin transition at slider extremes.
    let half = [(max[0] - min[0]).max(16.0) * 1.2, (max[1] - min[1]).max((max[0] - min[0]) * 0.4).max(16.0) * 1.2];
    let bounds = [center[0] - half[0], center[1] - half[1], center[0] + half[0], center[1] + half[1]];
    let r = RigWarp { kind, bounds, source: points, target: points, pinned: [false; 8] };
    r.validate()?;
    Ok(r)
}

impl FaceWarp {
    /// MediaPipe's published landmark indices, in document coordinates.
    pub fn from_landmarks(points: &[[f64; 2]]) -> Result<Self, String> {
        if points.len() != 478 || points.iter().flatten().any(|v| !v.is_finite() || v.abs() > 1_000_000.0) {
            return Err("A face needs 478 valid MediaPipe landmarks".into());
        }
        let contour = |ids: [usize; 8]| -> Result<[[f64; 2]; 8], String> {
            let mut out = [[0.0; 2]; 8];
            for (slot, id) in out.iter_mut().zip(ids) {
                *slot = *points.get(id).ok_or("Missing face landmark")?;
            }
            Ok(out)
        };
        let mut eyes = [
            region(RegionKind::Eye, contour([33, 160, 159, 158, 133, 153, 145, 144])?)?,
            region(RegionKind::Eye, contour([362, 385, 386, 387, 263, 373, 374, 380])?)?,
        ];
        eyes.sort_by(|a, b| a.bounds[0].total_cmp(&b.bounds[0]));
        let [left, right] = eyes;
        Ok(Self {
            regions: [
                left,
                right,
                region(RegionKind::Mouth, contour([61, 40, 0, 270, 291, 321, 17, 91])?)?,
                region(RegionKind::Mouth, contour([98, 45, 168, 275, 327, 326, 2, 97])?)?,
                region(RegionKind::Mouth, contour([234, 67, 10, 297, 454, 365, 152, 136])?)?,
            ],
            sliders: FaceSliders::default(),
            depth: None,
        })
    }

    pub fn from_depth_landmarks(points: &[[f64; 3]]) -> Result<Self, String> {
        if points.len() != 478 {
            return Err("Depth needs exactly 478 face landmarks".into());
        }
        let xy: Vec<_> = points.iter().map(|p| [p[0], p[1]]).collect();
        let mut face = Self::from_landmarks(&xy)?;
        face.depth = Some(Box::new(pose::FaceDepth::from_landmarks(points)?));
        Ok(face)
    }

    /// Project a feature handle through the same depth rotation as the image.
    pub fn project_point(&self, p: [f64; 2]) -> [f64; 2] {
        self.depth.as_ref().map_or(p, |depth| depth.project(p, self.sliders.head_yaw, self.sliders.head_pitch))
    }

    pub fn adjusted(&self) -> Result<[RigWarp; 5], String> {
        self.sliders.validate()?;
        if let Some(depth) = &self.depth {
            depth.validate()?;
        }
        if (self.sliders.head_yaw != 0.0 || self.sliders.head_pitch != 0.0) && self.depth.is_none() {
            return Err("Detect this face again to get depth for head rotation".into());
        }
        for r in &self.regions {
            r.validate()?;
            if !r.is_identity() || r.pinned.iter().any(|v| *v) {
                return Err("Face slider base contours must be unchanged and unpinned".into());
            }
        }
        let s = &self.sliders;
        let mut out = self.regions.clone();
        for (index, r) in out.iter_mut().enumerate() {
            let a = *r.source.first().ok_or("Missing region start")?;
            let b = *r.source.get(4).ok_or("Missing region end")?;
            let width = (b[0] - a[0]).hypot(b[1] - a[1]);
            if width < 1.0 {
                return Err("Face feature is too small or hidden; use a clearer, larger photo".into());
            }
            let along = [(b[0] - a[0]) / width, (b[1] - a[1]) / width];
            let down = [-along[1], along[0]];
            let center = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
            let (sx, sy, tilt) = match index {
                0 | 1 => {
                    let (size, width, height, tilt) = if index == 0 {
                        (s.eye_size, s.eye_width, s.eye_height, s.eye_tilt)
                    } else {
                        (
                            s.right_eye_size.unwrap_or(s.eye_size),
                            s.right_eye_width.unwrap_or(s.eye_width),
                            s.right_eye_height.unwrap_or(s.eye_height),
                            s.right_eye_tilt.unwrap_or(s.eye_tilt),
                        )
                    };
                    (1.0 + (size + width) * 0.0015, 1.0 + (size + height) * 0.002, tilt * 0.0015 * if index == 0 { 1.0 } else { -1.0 })
                }
                2 => (1.0 + s.mouth_width * 0.002, 1.0 + s.mouth_height * 0.003, 0.0),
                3 => (1.0 + s.nose_width * 0.002, 1.0 + s.nose_height * 0.002, 0.0),
                _ => (1.0 + s.face_width * 0.001, 1.0, 0.0),
            };
            for (i, (source, target)) in r.source.iter().zip(r.target.iter_mut()).enumerate() {
                let delta = [source[0] - center[0], source[1] - center[1]];
                let x = (delta[0] * along[0] + delta[1] * along[1]) * sx;
                let mut y = (delta[0] * down[0] + delta[1] * down[1]) * sy;
                if index == 2 && matches!(i, 0 | 1 | 3 | 4 | 5 | 7) {
                    let corner = if matches!(i, 0 | 4) { 1.0 } else { 0.45 };
                    y -= s.smile * width * 0.0008 * corner;
                }
                if index == 2 {
                    if matches!(i, 1..=3) {
                        y -= s.upper_lip * width * 0.0004;
                    }
                    if matches!(i, 5..=7) {
                        y += s.lower_lip * width * 0.0004;
                    }
                }
                if index == 4 && matches!(i, 1..=3) {
                    y -= s.forehead * width * 0.0005 * if i == 2 { 1.0 } else { 0.5 };
                }
                if index == 4 && matches!(i, 5..=7) {
                    y += s.chin_height * width * 0.0006 * if i == 6 { 1.0 } else { 0.5 };
                }
                let (sn, cs) = tilt.sin_cos();
                let x2 = x * cs - y * sn + if index == 4 && matches!(i, 5 | 7) { x * s.jaw_width * 0.0015 } else { 0.0 };
                let y2 = x * sn + y * cs;
                *target = [center[0] + along[0] * x2 + down[0] * y2, center[1] + along[1] * x2 + down[1] * y2];
                if index < 2 {
                    let spacing = s.eye_spacing * width * 0.001 * if index == 0 { -1.0 } else { 1.0 };
                    target[0] += along[0] * spacing;
                    target[1] += along[1] * spacing;
                }
                // Exact zero is identity, including roundoff in the rotated local frame.
                if sx == 1.0
                    && sy == 1.0
                    && tilt == 0.0
                    && (index != 2 || (s.smile == 0.0 && s.upper_lip == 0.0 && s.lower_lip == 0.0))
                    && (index != 4 || (s.chin_height == 0.0 && s.forehead == 0.0 && s.jaw_width == 0.0))
                    && (index >= 2 || s.eye_spacing == 0.0)
                {
                    *target = *source;
                }
            }
            if index < 2 {
                let f = if index == 0 { s.left_eye_close } else { s.right_eye_close } * 0.0085;
                // Close in the transformed contour, while preserving the original inverse-map sources.
                if f != 0.0 {
                    r.target = r.next_gesture().close_eye(f)?.target;
                }
            }
            prepare_steps(r)?;
        }
        Ok(out)
    }

    pub fn validate_grid(&self, bounds: Rect, cell: f64) -> Result<(), String> {
        for r in self.adjusted()? {
            if !r.is_identity() {
                r.validate_grid_with_budget(bounds, cell, MAX_FACE_NODES)?;
            }
        }
        if let Some(depth) = &self.depth {
            depth.validate_grid(bounds, cell, MAX_FACE_NODES)?;
            pose::PoseMap::prepare(depth, &self.sliders)?;
        }
        Ok(())
    }

    /// Transactional: a rejected region never leaves a partially edited field.
    pub fn apply(&self, field: &mut LiquifyField) -> Result<Rect, String> {
        let regions = self.adjusted()?;
        let rotation = self.depth.as_ref().map(|depth| pose::PoseMap::prepare(depth, &self.sliders)).transpose()?.flatten();
        for r in &regions {
            if !r.is_identity() {
                r.validate_grid_with_budget(field.bounds, field.cell, MAX_FACE_NODES)?;
            }
        }
        if regions.iter().all(RigWarp::is_identity) && rotation.is_none() {
            return Ok(Rect::EMPTY);
        }
        let mut candidate = field.clone();
        let mut dirty = Rect::EMPTY;
        for r in regions {
            if !r.is_identity() {
                dirty = dirty.union(&super::rig::apply_with_budget(&mut candidate, &r, MAX_FACE_NODES)?);
            }
        }
        if let Some(rotation) = rotation {
            dirty = dirty.union(&rotation.apply(&mut candidate, MAX_FACE_NODES)?);
        }
        *field = candidate;
        Ok(dirty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn synthetic_face() -> FaceWarp {
        let eye = RigWarp::new(RegionKind::Eye, [35.0, 40.0, 135.0, 110.0]).unwrap();
        let mut right = eye.clone();
        right.bounds[0] += 130.0;
        right.bounds[2] += 130.0;
        for p in &mut right.source {
            p[0] += 130.0;
        }
        right.target = right.source;
        FaceWarp {
            regions: [
                eye,
                right,
                RigWarp::new(RegionKind::Mouth, [65.0, 160.0, 235.0, 245.0]).unwrap(),
                RigWarp::new(RegionKind::Mouth, [100.0, 95.0, 200.0, 180.0]).unwrap(),
                RigWarp::new(RegionKind::Mouth, [0.0, 0.0, 300.0, 300.0]).unwrap(),
            ],
            sliders: FaceSliders::default(),
            depth: None,
        }
    }
    #[test]
    fn face_sliders_zero_is_exact_identity_and_replay_is_model_independent() {
        let mut face = synthetic_face();
        assert!(face.adjusted().unwrap().iter().all(RigWarp::is_identity));
        let mut field = LiquifyField::new(Rect::new(0, 0, 300, 300), 2.0);
        assert!(face.apply(&mut field).unwrap().is_empty());
        face.sliders.eye_size = 60.0;
        face.sliders.smile = 50.0;
        face.apply(&mut field).unwrap();
        assert!(!field.is_identity());
        let decoded: FaceWarp = serde_json::from_str(&serde_json::to_string(&face).unwrap()).unwrap();
        let mut replay = LiquifyField::new(field.bounds, field.cell);
        decoded.apply(&mut replay).unwrap();
        assert_eq!(field, replay);
    }
    #[test]
    fn face_sliders_reject_bad_parameters_without_partial_changes() {
        let mut face = synthetic_face();
        let mut field = LiquifyField::new(Rect::new(0, 0, 300, 300), 2.0);
        let before = field.clone();
        face.sliders.smile = f64::NAN;
        assert!(face.apply(&mut field).is_err());
        assert_eq!(field, before);
        face.sliders.smile = 101.0;
        assert!(face.apply(&mut field).is_err());
        assert!(FaceWarp::from_landmarks(&[]).is_err());
    }
    #[test]
    fn face_sliders_all_extremes_remain_valid() {
        for value in [-100.0, 100.0] {
            let mut face = synthetic_face();
            face.sliders = FaceSliders {
                eye_size: value,
                eye_width: value,
                eye_height: value,
                eye_spacing: value,
                eye_tilt: value,
                left_eye_close: 100.0,
                right_eye_close: 100.0,
                mouth_width: value,
                mouth_height: value,
                smile: value,
                nose_width: value,
                nose_height: value,
                face_width: value,
                chin_height: value,
                forehead: value,
                jaw_width: value,
                upper_lip: value,
                lower_lip: value,
                ..Default::default()
            };
            assert!(face.adjusted().is_ok());
        }
    }
    #[test]
    fn face_eye_sliders_can_be_linked_or_independent() {
        let mut face = synthetic_face();
        face.sliders.eye_size = 50.0;
        let linked = face.adjusted().unwrap();
        assert!(!linked[0].is_identity() && !linked[1].is_identity());
        face.sliders.right_eye_size = Some(0.0);
        let separate = face.adjusted().unwrap();
        assert!(!separate[0].is_identity());
        assert!(separate[1].is_identity());
        assert_eq!(linked[0], separate[0]);
        face.sliders.eye_tilt = 50.0;
        let tilted = face.adjusted().unwrap();
        assert!(tilted[0].target[0][1] < tilted[0].source[0][1]);
        assert!(tilted[1].target[4][1] < tilted[1].source[4][1]);
        face.sliders.right_eye_size = Some(f64::NAN);
        assert!(face.adjusted().is_err());
    }
}
