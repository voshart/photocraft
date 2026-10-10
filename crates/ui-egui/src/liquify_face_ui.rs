//! Beginner face workflow: enable detection, then adjust absolute sliders.
use super::LiquifyDialog;
use crate::widgets;
use photocraft_algo::liquify::{
    LiquifyField, LiquifyStroke, LiquifyTool,
    face::{FaceSliders, FaceWarp},
};
use serde_json::{Value, json};

#[path = "liquify_face_handles_ui.rs"]
pub(super) mod handles;

#[cfg(not(target_arch = "wasm32"))]
type DetectionResult = Result<Vec<Vec<[f64; 3]>>, String>;

#[derive(Clone)]
struct DetectedFace {
    warp: FaceWarp,
    points: Vec<[f64; 2]>,
}

#[derive(Default)]
pub(super) struct FaceControls {
    handles: handles::HandleState,
    pub(super) enabled: bool,
    faces: Vec<DetectedFace>,
    selected: usize,
    sliders: FaceSliders,
    base_len: usize,
    base_field: Option<LiquifyField>,
    pub(super) show_points: bool,
    status: String,
    error: bool,
    #[cfg(not(target_arch = "wasm32"))]
    pending: Option<std::sync::mpsc::Receiver<DetectionResult>>,
}

impl FaceControls {
    pub(super) fn photo_changed(&mut self) {
        self.handles = Default::default();
        if !self.faces.is_empty() {
            self.faces.clear();
            self.sliders = FaceSliders::default();
            self.base_field = None;
            self.status = "Photo changed. Turn on Face adjustments to detect it again.".into();
        }
    }
    pub fn describe(&self) -> Value {
        json!({"enabled":self.enabled,"automaticDetection": true, "faces": self.faces.len(), "selectedFace": self.selected,
            "sliders": self.sliders, "showLandmarks": self.show_points, "status": self.status,
            "error": self.error, "detecting": self.detecting()})
    }
    fn detecting(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.pending.is_some()
        }
        #[cfg(target_arch = "wasm32")]
        {
            false
        }
    }
    pub(super) fn feedback(&mut self, result: Result<(), String>) {
        if let Err(e) = result {
            self.status = e;
            self.error = true;
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn model_path() -> Result<std::path::PathBuf, String> {
    if let Some(path) = std::env::var_os("PHOTOCRAFT_FACE_MODEL") {
        return Ok(path.into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("Cannot find PhotoCraft's folder")?;
    let bundled = dir.join("models/face_landmarker.task");
    if bundled.is_file() {
        return Ok(bundled);
    }
    Ok(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/windows/models/face_landmarker.task"))
}

pub(super) fn set_enabled(d: &mut LiquifyDialog, enabled: bool) -> Result<(), String> {
    d.end();
    d.face.enabled = enabled;
    d.face.handles = Default::default();
    if !enabled {
        return Ok(());
    }
    d.preview_tool = super::PreviewTool::Brush;
    d.rig.enabled = false;
    d.opts.view.before = false;
    if d.face.faces.is_empty() && !d.face.detecting() {
        detect(d)?;
    }
    Ok(())
}

fn detect(d: &mut LiquifyDialog) -> Result<(), String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = d;
        Err("Automatic face detection is available in the Windows desktop build".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if d.face.detecting() {
            return Err("Face detection is already running".into());
        }
        d.end();
        let model = model_path()?;
        if !model.is_file() {
            return Err("Face model is missing. Run scripts/setup-face-model.ps1 once.".into());
        }
        let mut pixels = vec![[0; 4]; d.proxy.px.len()];
        d.proxy.render(&d.field, [0, 0, d.proxy.w, d.proxy.h], &mut pixels);
        // The inference thumbnail is the same colour-converted preview the user sees.
        // Composite premultiplied pixels on white, rather than creating black edges.
        let rgb: Vec<u8> = pixels
            .iter()
            .flat_map(|p| {
                let a = f64::from(p[3]) / 255.0;
                std::array::from_fn::<_, 3, _>(|i| (f64::from(p[i]) + 255.0 * (1.0 - a)).clamp(0.0, 255.0).round() as u8)
            })
            .collect();
        let width = u32::try_from(d.proxy.w).map_err(|_| "Photo thumbnail is too wide")?;
        let height = u32::try_from(d.proxy.h).map_err(|_| "Photo thumbnail is too tall")?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("face-landmarks".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(|| photocraft_ml::face_landmarks(&model, width, height, &rgb))
                    .unwrap_or_else(|_| Err("Face detector stopped unexpectedly. Your photo has not changed.".into()));
                let _ = tx.send(result);
            })
            .map_err(|e| format!("Could not start face detection: {e}"))?;
        d.face.enabled = true;
        d.face.pending = Some(rx);
        d.face.base_len = d.strokes.len();
        d.face.base_field = Some(d.field.clone());
        d.face.faces.clear();
        d.face.sliders = FaceSliders::default();
        d.face.status = "Finding faces on your computer...".into();
        d.face.error = false;
        d.rig.enabled = false;
        d.opts.view.before = false;
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn accept_detection(d: &mut LiquifyDialog, faces: Vec<Vec<[f64; 3]>>) -> Result<(), String> {
    if d.strokes.len() != d.face.base_len {
        return Err("The photo changed during detection. Click Detect faces again.".into());
    }
    let mut detected = Vec::new();
    for points in faces {
        // Thumbnail width/height include a possible partial final proxy pixel.
        let points: Vec<_> = points
            .into_iter()
            .map(|p| {
                [
                    f64::from(d.canvas.x0) + p[0] * d.proxy.w as f64 * d.proxy.scale,
                    f64::from(d.canvas.y0) + p[1] * d.proxy.h as f64 * d.proxy.scale,
                    p[2] * d.proxy.w as f64 * d.proxy.scale,
                ]
            })
            .collect();
        if let Ok(warp) = FaceWarp::from_depth_landmarks(&points).and_then(|warp| {
            warp.validate_grid(d.canvas, d.cell)?;
            Ok(warp)
        }) {
            detected.push(DetectedFace { warp, points: points.into_iter().map(|p| [p[0], p[1]]).collect() });
        }
    }
    detected.sort_by(|a, b| {
        let area = |f: &DetectedFace| f.warp.regions.last().map(|r| (r.bounds[2] - r.bounds[0]) * (r.bounds[3] - r.bounds[1])).unwrap_or(0.0);
        area(b).total_cmp(&area(a))
    });
    if detected.is_empty() {
        return Err("No editable face found. Try a larger, well-lit face looking toward the camera.".into());
    }
    d.face.faces = detected;
    d.face.selected = 0;
    d.face.sliders = FaceSliders::default();
    d.face.handles = Default::default();
    d.face.status = "Face found. Hover over a feature, then drag a handle or use a slider.".into();
    d.face.error = false;
    Ok(())
}

pub(super) fn poll(d: &mut LiquifyDialog, ctx: &egui::Context) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(rx) = &d.face.pending else { return };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("Face detector stopped. Click Detect faces to retry.".into()),
        };
        d.face.pending = None;
        let result = result.and_then(|faces| accept_detection(d, faces));
        d.face.feedback(result);
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (d, ctx);
    }
}

fn apply_sliders(d: &mut LiquifyDialog, sliders: FaceSliders) -> Result<(), String> {
    if d.opts.view.before {
        return Err("Click Edited photo to continue adjusting the face.".into());
    }
    if !d.face.enabled {
        return Err("Turn on Face adjustments first.".into());
    }
    if d.cur.is_some() || d.rig.pointer_captured() {
        return Err("Finish the current stroke before adjusting face sliders.".into());
    }
    sliders.validate()?;
    let detected = d.face.faces.get(d.face.selected).ok_or("Click Detect faces first")?;
    let mut warp = detected.warp.clone();
    warp.sliders = sliders.clone();
    warp.validate_grid(d.canvas, d.cell)?;
    if d.strokes.len() < d.face.base_len
        || d.strokes.len() > d.face.base_len.saturating_add(1)
        || (d.strokes.len() > d.face.base_len && d.strokes.last().is_none_or(|s| s.tool != LiquifyTool::FaceWarpV1))
    {
        return Err("Brush editing changed the photo. Detect faces again before using the sliders.".into());
    }
    let prefix = d.strokes.get(..d.face.base_len).ok_or("Face edit history changed")?;
    let mut field = d.face.base_field.clone().unwrap_or_else(|| LiquifyField::from_strokes(d.canvas, d.cell, prefix));
    let changed = warp.apply(&mut field)?;
    d.end();
    d.strokes.truncate(d.face.base_len);
    d.redo.clear();
    if !changed.is_empty() {
        let mut stroke = LiquifyStroke::new(LiquifyTool::FaceWarpV1, 1.0);
        stroke.face = Some(Box::new(warp));
        d.strokes.push(stroke);
    }
    d.field = field;
    d.face.sliders = sliders;
    d.face.status = if changed.is_empty() {
        "Starting shape restored, or this face is protected by the freeze mask."
    } else {
        "Preview updated. Compare with Original photo."
    }
    .into();
    d.face.error = false;
    d.rig.enabled = false;
    d.dirty = Some([0, 0, d.proxy.w, d.proxy.h]);
    d.detail.invalidate();
    d.render_dirty();
    Ok(())
}

/// Undo/redo and reopened Smart Filters recover saved sliders without running a model.
pub(super) fn sync_history(d: &mut LiquifyDialog) {
    d.face.handles = Default::default();
    if let Some(face) = d.strokes.last().and_then(|s| s.face.as_deref()) {
        let mut original = face.clone();
        original.sliders = FaceSliders::default();
        d.face.faces = vec![DetectedFace { warp: original, points: Vec::new() }];
        d.face.selected = 0;
        d.face.base_len = d.strokes.len().saturating_sub(1);
        d.face.base_field = d.strokes.get(..d.face.base_len).map(|prefix| LiquifyField::from_strokes(d.canvas, d.cell, prefix));
        d.face.sliders = face.sliders.clone();
        d.rig.enabled = false;
    } else if d.strokes.len() == d.face.base_len {
        d.face.sliders = FaceSliders::default();
    } else if !d.face.faces.is_empty() {
        d.face.faces.clear();
        d.face.base_field = None;
        d.face.sliders = FaceSliders::default();
        d.face.status = "Photo changed. Click Detect faces again to use the sliders.".into();
    }
}

fn select_face(d: &mut LiquifyDialog, selected: usize) -> Result<(), String> {
    if selected >= d.face.faces.len() {
        return Err("No such detected face".into());
    }
    if selected != d.face.selected {
        // Keep the completed face edit as the next face's baseline.
        d.face.base_len = d.strokes.len();
        d.face.base_field = Some(d.field.clone());
        d.face.selected = selected;
        d.face.sliders = FaceSliders::default();
    }
    Ok(())
}

pub(super) fn control(d: &mut LiquifyDialog, v: &Value) -> Result<(), String> {
    if let Some(value) = v.get("faceEnabled") {
        let enabled = value.as_bool().ok_or("faceEnabled must be true or false")?;
        set_enabled(d, enabled)?;
    }
    if let Some(value) = v.get("detectFaces") {
        if value.as_bool() != Some(true) {
            return Err("detectFaces must be true".into());
        }
        detect(d)?;
    }
    if let Some(value) = v.get("selectedFace") {
        let selected = value.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or("selectedFace must be a face index")?;
        select_face(d, selected)?;
    }
    if let Some(value) = v.get("faceSliders") {
        let sliders: FaceSliders = serde_json::from_value(value.clone()).map_err(|e| format!("Invalid face sliders: {e}"))?;
        apply_sliders(d, sliders)?;
    }
    if let Some(value) = v.get("showFaceLandmarks") {
        d.face.show_points = value.as_bool().ok_or("showFaceLandmarks must be true or false")?;
    }
    Ok(())
}

fn slider(ui: &mut egui::Ui, label: &str, value: &mut f64, close: bool, help: &str) -> bool {
    let range = if close { 0.0..=100.0 } else { -100.0..=100.0 };
    ui.horizontal(|ui| {
        ui.add_sized([110.0, 20.0], egui::Label::new(label)).on_hover_text(help);
        ui.spacing_mut().slider_width = 120.0;
        ui.add(egui::Slider::new(value, range).clamping(egui::SliderClamping::Edits).step_by(1.0).show_value(true)).on_hover_text(help).changed()
    })
    .inner
}

fn eye_slider(ui: &mut egui::Ui, label: &str, left: &mut f64, right: &mut Option<f64>, linked: bool) -> bool {
    if linked {
        return slider(ui, label, left, false, "Adjusts both eyes. 0 restores the starting shape.");
    }
    ui.label(label);
    let mut r = right.unwrap_or(*left);
    let mut changed = false;
    ui.columns(2, |columns| {
        if let Some(column) = columns.first_mut() {
            column.small("Left");
            column.spacing_mut().slider_width = 75.0;
            changed |= column.add(egui::Slider::new(left, -100.0..=100.0).clamping(egui::SliderClamping::Edits).step_by(1.0)).changed();
        }
        if let Some(column) = columns.get_mut(1) {
            column.small("Right");
            column.spacing_mut().slider_width = 75.0;
            changed |= column.add(egui::Slider::new(&mut r, -100.0..=100.0).clamping(egui::SliderClamping::Edits).step_by(1.0)).changed();
        }
    });
    *right = Some(r);
    changed
}

pub(super) fn panel(d: &mut LiquifyDialog, ui: &mut egui::Ui) {
    widgets::section_label(ui, "Face adjustments");
    ui.small("Photos stay on this computer. Google's detector sends usage/performance metrics.");
    ui.hyperlink_to("MediaPipe privacy details", "https://github.com/google-ai-edge/mediapipe#privacy-notice");
    let mut enabled = d.face.enabled;
    if widgets::checkbox(ui, &mut enabled, "Enable face adjustments").changed() {
        let result = set_enabled(d, enabled);
        d.face.feedback(result);
    }
    if !d.face.enabled {
        ui.small("Turn on to find faces and adjust them with sliders.");
        return;
    }
    ui.small("Hover over a face feature to reveal handles. Drag a handle or move a slider, then Apply.");
    let busy = d.face.detecting();
    if ui
        .add_enabled(
            !busy,
            egui::Button::new(if busy {
                "Finding face..."
            } else if d.face.faces.is_empty() {
                "Detect face automatically"
            } else {
                "Detect again"
            })
            .min_size(egui::vec2(250.0, 30.0)),
        )
        .clicked()
    {
        let result = detect(d);
        d.face.feedback(result);
    }
    if busy {
        ui.spinner();
    }
    if !d.face.status.is_empty() {
        let tokens = crate::theme::Tokens::get(ui.ctx());
        ui.label(egui::RichText::new(&d.face.status).color(if d.face.error { tokens.danger } else { tokens.text }));
    }
    if d.face.faces.is_empty() {
        ui.small("No boxes to draw or dots to place. Start with Detect faces.");
        return;
    }
    ui.horizontal(|ui| {
        let tokens = crate::theme::Tokens::get(ui.ctx());
        let edited = egui::RichText::new("Edited photo").color(if d.opts.view.before { tokens.text } else { tokens.accent_text });
        let original = egui::RichText::new("Original photo").color(if d.opts.view.before { tokens.accent_text } else { tokens.text });
        ui.selectable_value(&mut d.opts.view.before, false, edited);
        ui.selectable_value(&mut d.opts.view.before, true, original);
    });
    if d.opts.view.before {
        ui.small("Showing the original. Click Edited photo to continue.");
    }
    widgets::checkbox(ui, &mut d.face.show_points, "Show detected face outline");
    let mut sliders = d.face.sliders.clone();
    let mut changed = false;
    ui.add_enabled_ui(!d.opts.view.before && !busy, |ui| {
        ui.small("0 = starting shape. Left/right mean the sides of this photo.");
        if widgets::secondary_button(ui, "Reset face", 120.0).on_hover_text("Restore this face's starting shape. Brush edits are kept.").clicked() {
            sliders = FaceSliders::default();
            changed = true;
        }
        egui::CollapsingHeader::new("Head rotation (2.5D)").default_open(true).show(ui, |ui| {
            ui.spacing_mut().slider_width = 100.0;
            ui.small("Small turns work best: try 5–15°. Total turn is limited to 30°.");
            let has_depth = d.face.faces.get(d.face.selected).is_some_and(|f| f.warp.depth.is_some());
            if !has_depth {
                ui.small("Detect again to enable depth rotation for this saved face.");
            }
            ui.add_enabled_ui(has_depth, |ui| {
                let yaw_limit = (900.0 - sliders.head_pitch.powi(2)).max(0.0).sqrt();
                ui.horizontal(|ui| {
                    ui.add_sized([110.0, 20.0], egui::Label::new("Turn left / right"));
                    changed |= ui
                        .add(
                            egui::Slider::new(&mut sliders.head_yaw, -yaw_limit..=yaw_limit)
                                .clamping(egui::SliderClamping::Edits)
                                .suffix("°")
                                .fixed_decimals(1),
                        )
                        .on_hover_text("Drag left or right to turn toward that side of the photo")
                        .changed();
                });
                ui.horizontal(|ui| {
                    ui.add_sized([110.0, 20.0], egui::Label::new("Look down / up"));
                    let pitch_limit = (900.0 - sliders.head_yaw.powi(2)).max(0.0).sqrt();
                    changed |= ui
                        .add(
                            egui::Slider::new(&mut sliders.head_pitch, -pitch_limit..=pitch_limit)
                                .clamping(egui::SliderClamping::Edits)
                                .suffix("°")
                                .fixed_decimals(1),
                        )
                        .on_hover_text("Negative looks down; positive looks up")
                        .changed();
                });
            });
            ui.small("Uses the existing photo: hidden skin, ears and hair cannot be reconstructed.");
        });
        egui::CollapsingHeader::new("Eyes").default_open(true).show(ui, |ui| {
            let mut linked = sliders.eyes_linked();
            if widgets::checkbox(ui, &mut linked, "Adjust both eyes together").changed() {
                sliders.right_eye_size = if linked { None } else { Some(sliders.eye_size) };
                sliders.right_eye_width = if linked { None } else { Some(sliders.eye_width) };
                sliders.right_eye_height = if linked { None } else { Some(sliders.eye_height) };
                sliders.right_eye_tilt = if linked { None } else { Some(sliders.eye_tilt) };
                sliders.right_eye_move_x = if linked { None } else { Some(sliders.eye_move_x) };
                sliders.right_eye_move_y = if linked { None } else { Some(sliders.eye_move_y) };
                changed = true;
            }
            changed |= eye_slider(ui, "Eye size", &mut sliders.eye_size, &mut sliders.right_eye_size, linked);
            changed |= eye_slider(ui, "Eye height", &mut sliders.eye_height, &mut sliders.right_eye_height, linked);
            egui::CollapsingHeader::new("More eye adjustments").show(ui, |ui| {
                changed |= eye_slider(ui, "Move left / right", &mut sliders.eye_move_x, &mut sliders.right_eye_move_x, linked);
                changed |= eye_slider(ui, "Move up / down", &mut sliders.eye_move_y, &mut sliders.right_eye_move_y, linked);
                changed |= eye_slider(ui, "Eye width", &mut sliders.eye_width, &mut sliders.right_eye_width, linked);
                changed |= eye_slider(ui, "Eye tilt", &mut sliders.eye_tilt, &mut sliders.right_eye_tilt, linked);
                changed |= slider(ui, "Eye distance", &mut sliders.eye_spacing, false, "Negative brings the eyes closer; positive moves them further apart");
                changed |= slider(ui, "Close left eye", &mut sliders.left_eye_close, true, "0 leaves the eye as it is; 100 nearly closes it");
                changed |= slider(ui, "Close right eye", &mut sliders.right_eye_close, true, "0 leaves the eye as it is; 100 nearly closes it");
            });
        });
        egui::CollapsingHeader::new("Mouth").default_open(true).show(ui, |ui| {
            changed |= slider(ui, "Smile", &mut sliders.smile, false, "Lower corners ← → Lift corners");
            changed |= slider(ui, "Mouth width", &mut sliders.mouth_width, false, "Narrower ← → Wider");
            changed |= slider(ui, "Mouth height", &mut sliders.mouth_height, false, "Shorter ← → Taller");
            egui::CollapsingHeader::new("Lip adjustments").show(ui, |ui| {
                changed |= slider(ui, "Upper lip", &mut sliders.upper_lip, false, "Lower ← → Raise the upper lip outline");
                changed |= slider(ui, "Lower lip", &mut sliders.lower_lip, false, "Raise ← → Lower the lower lip outline");
            });
            egui::CollapsingHeader::new("Mouth position").show(ui, |ui| {
                changed |= slider(ui, "Move left / right", &mut sliders.mouth_move_x, false, "Negative moves left; positive moves right");
                changed |= slider(ui, "Move up / down", &mut sliders.mouth_move_y, false, "Negative moves up; positive moves down");
            });
        });
        egui::CollapsingHeader::new("Nose").show(ui, |ui| {
            changed |= slider(ui, "Nose width", &mut sliders.nose_width, false, "Narrower ← → Wider");
            changed |= slider(ui, "Nose length", &mut sliders.nose_height, false, "Shorter ← → Longer");
            changed |= slider(ui, "Move left / right", &mut sliders.nose_move_x, false, "Negative moves left; positive moves right");
            changed |= slider(ui, "Move up / down", &mut sliders.nose_move_y, false, "Negative moves up; positive moves down");
        });
        egui::CollapsingHeader::new("Face shape").show(ui, |ui| {
            changed |= slider(ui, "Face width", &mut sliders.face_width, false, "Slimmer ← → Wider");
            changed |= slider(ui, "Chin length", &mut sliders.chin_height, false, "Shorter ← → Longer");
            changed |= slider(ui, "Forehead", &mut sliders.forehead, false, "Shorter ← → Taller");
            changed |= slider(ui, "Jaw width", &mut sliders.jaw_width, false, "Narrower ← → Wider");
        });
    });
    if changed {
        let turn = sliders.head_yaw.hypot(sliders.head_pitch);
        // Decimal entry may round a dynamic endpoint slightly beyond the circular limit.
        if turn > 30.0 {
            sliders.head_yaw *= 30.0 / turn;
            sliders.head_pitch *= 30.0 / turn;
        }
        let result = apply_sliders(d, sliders);
        d.face.feedback(result);
    }
    ui.small("Face sliders form one undoable edit. Apply saves it to the photo; Cancel discards this session.");
}

pub(super) fn draw(d: &LiquifyDialog, painter: &egui::Painter, area: egui::Rect) {
    if !d.face.enabled || !d.face.show_points || d.opts.view.before {
        return;
    }
    let Some(face) = d.face.faces.get(d.face.selected) else { return };
    let tokens = crate::theme::Tokens::get(painter.ctx());
    // This shows the original detected outline only at zero; after editing show transformed contours.
    let mut warp = face.warp.clone();
    warp.sliders = d.face.sliders.clone();
    if let Ok(regions) = warp.adjusted() {
        for r in regions {
            let mut line: Vec<_> = r.target.into_iter().map(|p| super::to_screen(d, area, warp.project_point(p))).collect();
            if let Some(first) = line.first().copied() {
                line.push(first);
            }
            painter.add(egui::Shape::line(line, egui::Stroke::new(1.0, tokens.accent)));
        }
    }
    if d.face.sliders == FaceSliders::default() {
        for point in &face.points {
            painter.circle_filled(super::to_screen(d, area, *point), 1.4, tokens.accent);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PhotocraftApp;
    #[test]
    fn face_panel_and_position_controls_preserve_saved_values_without_user_input() {
        let mut app = app();
        let d = app.distort.liquify.as_mut().unwrap();
        ready(d);
        d.face.faces.first_mut().unwrap().warp.depth =
            Some(Box::new(photocraft_algo::liquify::face::pose::FaceDepth { bounds: [0., 0., 300., 300.], samples: [[-30.; 25]; 25] }));
        let sliders = FaceSliders { head_yaw: 10., eye_size: 12.25, eye_move_x: 5.25, mouth_move_y: -6.75, ..Default::default() };
        apply_sliders(d, sliders.clone()).unwrap();
        let field = d.field.clone();
        let strokes = d.strokes.clone();
        let ctx = egui::Context::default();
        PhotocraftApp::setup_context(&ctx, Default::default());
        let mut position = 6.25;
        let mut left = 2.35;
        let mut right = Some(-3.75);
        for _ in 0..4 {
            let mut out = ctx.run_ui(
                egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280., 1600.))), ..Default::default() },
                |ui| {
                    panel(d, ui);
                    assert!(!slider(ui, "Move up / down", &mut position, false, "Position"));
                    assert!(!eye_slider(ui, "Move left / right", &mut left, &mut right, false));
                },
            );
            out.textures_delta.clear();
            assert_eq!(d.face.sliders, sliders);
            assert_eq!(d.field, field);
            assert_eq!(d.strokes, strokes);
            assert_eq!((position, left, right), (6.25, 2.35, Some(-3.75)));
        }
    }
    pub(super) fn app() -> PhotocraftApp {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
        app.session.execute("file.new", json!({"width":300,"height":300})).unwrap();
        super::super::open(&mut app, &egui::Context::default()).unwrap();
        app
    }
    pub(super) fn ready(d: &mut LiquifyDialog) {
        use photocraft_algo::liquify::rig::{RegionKind, RigWarp};
        let regions = [
            RigWarp::new(RegionKind::Eye, [0., 20., 130., 130.]).unwrap(),
            RigWarp::new(RegionKind::Eye, [170., 20., 300., 130.]).unwrap(),
            RigWarp::new(RegionKind::Mouth, [50., 170., 250., 280.]).unwrap(),
            RigWarp::new(RegionKind::Mouth, [80., 90., 220., 210.]).unwrap(),
            RigWarp::new(RegionKind::Mouth, [0., 0., 300., 300.]).unwrap(),
        ];
        d.face.faces = vec![DetectedFace { warp: FaceWarp { regions, sliders: FaceSliders::default(), depth: None }, points: Vec::new() }];
        d.face.base_len = d.strokes.len();
        d.face.enabled = true;
    }
    #[test]
    fn face_icon_returns_from_hand_and_zoom_without_redetecting_or_changing_pixels() {
        let mut app = app();
        ready(app.distort.liquify.as_mut().unwrap());
        let ctx = egui::Context::default();
        PhotocraftApp::setup_context(&ctx, Default::default());
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280., 900.));
        let frame = |app: &mut PhotocraftApp, events| {
            let mut out = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), events, ..Default::default() }, |ui| super::super::show(app, ui.ctx()));
            out.textures_delta.clear();
        };
        for _ in 0..4 {
            frame(&mut app, vec![]);
        }
        let original = app.distort.liquify.as_ref().unwrap().field.clone();
        let pos = egui::pos2(23., 539.);
        for tool in [super::super::PreviewTool::Hand, super::super::PreviewTool::Zoom] {
            app.distort.liquify.as_mut().unwrap().preview_tool = tool;
            frame(&mut app, vec![egui::Event::PointerMoved(pos)]);
            for pressed in [true, false] {
                frame(&mut app, vec![egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }]);
            }
            let d = app.distort.liquify.as_ref().unwrap();
            assert!(d.face.enabled);
            assert!(!d.face.detecting());
            assert!(d.preview_tool == super::super::PreviewTool::Brush);
            assert_eq!(d.field, original);
            assert!(d.strokes.is_empty());
        }
    }
    #[test]
    fn face_slider_preview_is_absolute_resettable_and_one_undoable_operation() {
        let mut app = app();
        let d = app.distort.liquify.as_mut().unwrap();
        ready(d);
        let original = d.field.clone();
        control(d, &json!({"faceSliders":{"eyeSize":45,"smile":30}})).unwrap();
        let edited = d.field.clone();
        assert_ne!(edited, original);
        assert_eq!(d.strokes.len(), 1);
        control(d, &json!({"faceSliders":{"eyeSize":70}})).unwrap();
        assert_eq!(d.strokes.len(), 1);
        control(d, &json!({"faceSliders":{"eyeSize":45,"smile":30}})).unwrap();
        assert_eq!(d.field, edited);
        d.undo();
        assert_eq!(d.field, original);
        assert_eq!(d.face.sliders, FaceSliders::default());
        d.redo();
        assert_eq!(d.field, edited);
        assert_eq!(d.face.sliders.eye_size, 45.0);
        control(d, &json!({"faceSliders":{}})).unwrap();
        assert_eq!(d.field, original);
        assert!(d.strokes.is_empty());
    }
    #[test]
    fn face_slider_before_and_invalid_inputs_leave_photo_unchanged() {
        let mut app = app();
        let d = app.distort.liquify.as_mut().unwrap();
        ready(d);
        let before = d.field.clone();
        d.opts.view.before = true;
        assert!(control(d, &json!({"faceSliders":{"smile":20}})).is_err());
        d.opts.view.before = false;
        for params in [json!({"faceSliders":{"smile":101}}), json!({"faceSliders":{"smile":"wide"}}), json!({"selectedFace":999})] {
            assert!(control(d, &params).is_err());
            assert_eq!(d.field, before);
        }
    }
    #[test]
    fn face_detection_is_opt_in_and_switching_off_preserves_edits() {
        let mut app = app();
        let d = app.distort.liquify.as_mut().unwrap();
        assert!(!d.face.enabled);
        assert!(!d.face.detecting());
        ready(d);
        control(d, &json!({"faceSliders":{"smile":50}})).unwrap();
        let field = d.field.clone();
        let strokes = d.strokes.clone();
        control(d, &json!({"faceEnabled":false})).unwrap();
        assert_eq!(d.field, field);
        assert_eq!(d.strokes, strokes);
        assert!(control(d, &json!({"faceSliders":{"smile":30}})).is_err());
        d.preview_tool = super::super::PreviewTool::Hand;
        control(d, &json!({"faceEnabled":true})).unwrap();
        assert!(d.preview_tool == super::super::PreviewTool::Brush);
        assert!(!d.face.detecting());
        assert_eq!(d.field, field);
    }
    #[test]
    #[ignore = "Requires the pinned MediaPipe runtime/model and PHOTOCRAFT_FACE_QA pointing to a public-domain portrait"]
    #[cfg(not(target_arch = "wasm32"))]
    fn face_native_detection_drives_real_sliders_and_reset_without_changing_the_document() {
        let path = std::env::var("PHOTOCRAFT_FACE_QA").unwrap();
        let bytes = std::fs::read(path).unwrap();
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
        photocraft_engine::file_cmds::open_bytes_as(&mut app.session, "portrait.png", &bytes, None, None).unwrap();
        let ctx = egui::Context::default();
        super::super::open(&mut app, &ctx).unwrap();
        let document = app.session.active().unwrap().doc.clone();
        let d = app.distort.liquify.as_mut().unwrap();
        assert!(!d.face.enabled);
        control(d, &json!({"faceEnabled":true,"showFaceLandmarks":true})).unwrap();
        let started = std::time::Instant::now();
        while d.face.detecting() && started.elapsed() < std::time::Duration::from_secs(10) {
            poll(d, &ctx);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!d.face.detecting());
        assert!(!d.face.faces.is_empty(), "{}", d.face.status);
        assert!(d.face.show_points, "Finishing detection must preserve the user's outline choice");
        assert_eq!(d.face.faces.first().unwrap().points.len(), 478);
        let original = d.field.clone();
        assert!(d.face.faces.first().unwrap().warp.depth.is_some());
        for key in ["headYaw", "headPitch"] {
            for value in [-15, 15] {
                control(d, &json!({"faceSliders":{key:value}})).unwrap_or_else(|e| panic!("{key}={value}: {e}"));
                assert_ne!(d.field, original);
                assert_eq!(d.strokes.len(), 1);
            }
            for value in [-30, 30] {
                let before = d.field.clone();
                let sliders = d.face.sliders.clone();
                let strokes = d.strokes.clone();
                match control(d, &json!({"faceSliders":{key:value}})) {
                    Ok(()) => {
                        assert_ne!(d.field, original);
                        assert_eq!(d.strokes.len(), 1);
                    }
                    Err(e) => {
                        assert!(e.contains("fold") || e.contains("inverted safely"), "{key}={value}: {e}");
                        assert_eq!(d.field, before);
                        assert_eq!(d.face.sliders, sliders);
                        assert_eq!(d.strokes, strokes);
                    }
                }
            }
        }
        for key in [
            "eyeSize",
            "eyeWidth",
            "eyeHeight",
            "eyeTilt",
            "eyeSpacing",
            "eyeMoveX",
            "eyeMoveY",
            "rightEyeMoveX",
            "rightEyeMoveY",
            "mouthMoveX",
            "mouthMoveY",
            "noseMoveX",
            "noseMoveY",
            "mouthWidth",
            "mouthHeight",
            "smile",
            "upperLip",
            "lowerLip",
            "noseWidth",
            "noseHeight",
            "faceWidth",
            "forehead",
            "jawWidth",
            "chinHeight",
        ] {
            for value in [-100, 100] {
                control(d, &json!({"faceSliders":{key:value}})).unwrap_or_else(|e| panic!("{key}={value}: {e}"));
                assert_ne!(d.field, original, "{key} must actually change the field");
                assert_eq!(d.strokes.len(), 1);
            }
        }
        control(d, &json!({"faceSliders":{}})).unwrap();
        assert_eq!(d.field, original);
        assert_eq!(app.session.active().unwrap().doc, document);
    }
}
