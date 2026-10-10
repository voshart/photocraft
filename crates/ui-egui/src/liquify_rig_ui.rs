// SPDX-License-Identifier: MIT OR Apache-2.0
//! Manual eye/mouth guides and a brush over those guides. Automatic detection is
//! intentionally absent. Placement/correction changes guides only; Sculpt changes
//! the same Liquify field as ordinary strokes. Each accepted gesture is one stroke.

use super::{LiquifyDialog, ToolEvent};
use egui::{Modifiers, Pos2, Rect as ScreenRect, Stroke, Ui};
use photocraft_algo::liquify::rig::{HANDLE_COUNT, RegionKind, RigWarp, prepare_steps};
use photocraft_algo::liquify::{LiquifyField, LiquifyStroke, LiquifyTool};
use serde_json::{Value, json};

const MAX_SNAPSHOT_NODES: usize = 4_000_000;

pub(super) fn changes_geometry(tool: LiquifyTool) -> bool {
    !matches!(
        tool,
        LiquifyTool::Freeze | LiquifyTool::Thaw | LiquifyTool::LassoMask | LiquifyTool::FreezeAll | LiquifyTool::ThawAll | LiquifyTool::InvertFreeze
    )
}

#[derive(Default)]
pub(super) struct ManualRig {
    pub enabled: bool,
    pub adjust: bool,
    pub brush: bool,
    placing: Option<RegionKind>,
    guide: Option<RigWarp>,
    drag: Option<Drag>,
    // Pin clicks and empty-space presses also own the pointer until release.
    // A gesture with no deformation must not be mistaken for a new mouse-down
    // by the dialog's per-frame fallback input routing.
    press_active: bool,
    last_pointer: Option<[f64; 2]>,
    pub status: String,
}

pub(super) struct GuideState {
    guide: Option<RigWarp>,
    enabled: bool,
    adjust: bool,
    brush: bool,
}

enum Drag {
    Place { kind: RegionKind, start: [f64; 2], end: [f64; 2] },
    Adjust { index: usize },
    Sculpt { base: LiquifyField, before: Box<RigWarp>, start: [f64; 2], weights: [f64; HANDLE_COUNT], applied: Option<Box<RigWarp>> },
}

enum Action {
    Place(RegionKind),
    Close(f64),
    Smirk(bool),
    Hide,
}

impl ManualRig {
    pub fn describe(&self) -> Value {
        json!({"enabled":self.enabled,"adjustGuides":self.adjust,"rigBrush":self.brush,
               "guide": self.guide, "dragging": self.drag.is_some(), "pointerCaptured": self.press_active, "status": self.status,
               "automaticDetection": false})
    }
    /// Persist only current geometry and pins, not temporary view/mode flags.
    pub fn saved_guide(&self) -> Option<RigWarp> {
        self.guide.as_ref().map(RigWarp::next_gesture)
    }

    pub fn preserve_for(&self, tool: LiquifyTool) -> Option<GuideState> {
        (!changes_geometry(tool)).then(|| GuideState { guide: self.guide.clone(), enabled: self.enabled, adjust: self.adjust, brush: self.brush })
    }

    pub fn restore_preserved(&mut self, state: Option<GuideState>) {
        if let Some(state) = state {
            self.restore_saved(state.guide);
            self.enabled = state.enabled;
            self.adjust = state.adjust;
            self.brush = state.brush;
        }
    }

    pub fn restore_saved(&mut self, guide: Option<RigWarp>) {
        self.drag = None;
        self.press_active = false;
        self.placing = None;
        self.guide = guide;
        self.enabled = self.guide.is_some();
        self.adjust = false;
        self.last_pointer = None;
    }

    /// Undoing the first guided edit must keep its pre-gesture guide, rather
    /// than leaving the user with no controls until they redo it.
    pub fn recover_next(&mut self, next: Option<&LiquifyStroke>) {
        if let Some(rig) = next.and_then(|stroke| stroke.rig.as_ref()) {
            let mut before = rig.clone();
            before.target = before.source;
            self.restore_saved(Some(before));
        }
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }
    pub fn pointer_captured(&self) -> bool {
        self.press_active || self.drag.is_some()
    }
    pub fn moved(&self, p: [f64; 2]) -> bool {
        self.drag.is_some() && self.last_pointer != Some(p)
    }

    /// A following raw stroke changes the coordinate reference; avoid showing old
    /// guides over new geometry. Re-place or undo that stroke to recover a guide.
    pub fn raw_started(&mut self) {
        if self.guide.take().is_some() {
            self.status = "Raw stroke started: re-place guides before further guided edits".into();
        }
        self.drag = None;
        self.press_active = false;
        self.enabled = false;
    }

    pub fn recover(&mut self, strokes: &[LiquifyStroke]) {
        self.drag = None;
        self.press_active = false;
        self.placing = None;
        self.guide = strokes
            .iter()
            .rev()
            .find(|s| {
                !matches!(
                    s.tool,
                    LiquifyTool::Freeze
                        | LiquifyTool::Thaw
                        | LiquifyTool::LassoMask
                        | LiquifyTool::FreezeAll
                        | LiquifyTool::ThawAll
                        | LiquifyTool::InvertFreeze
                )
            })
            .and_then(|s| s.rig.as_ref())
            .map(RigWarp::next_gesture);
        self.enabled = self.guide.is_some();
        self.adjust = false;
    }
}

pub(super) fn panel(d: &mut LiquifyDialog, ui: &mut Ui) {
    crate::widgets::hairline(ui);
    crate::widgets::section_label(ui, tl!("Manual face controls"));
    let mut action = None;
    ui.horizontal(|ui| {
        if ui.button(tl!("Place eye")).clicked() {
            action = Some(Action::Place(RegionKind::Eye));
        }
        if ui.button(tl!("Place mouth")).clicked() {
            action = Some(Action::Place(RegionKind::Mouth));
        }
    });
    crate::widgets::checkbox(ui, &mut d.rig.enabled, tl!("Use manual rig"));
    if d.rig.guide.is_some() {
        crate::widgets::checkbox(ui, &mut d.rig.adjust, tl!("Adjust guides only (no pixel movement)"));
        crate::widgets::checkbox(ui, &mut d.rig.brush, tl!("Rig brush: radius recruits nearby handles"));
        ui.small(tl!("Shift-click a handle to pin it. Uncheck Use manual rig for ordinary Liquify."));
        ui.small(tl!("Handles move 1:1. Rig brush uses Size, Density and Pressure."));
        if !d.rig.adjust {
            let kind = d.rig.guide.as_ref().map(|r| r.kind);
            ui.horizontal(|ui| {
                if kind == Some(RegionKind::Eye) {
                    if ui.button(tl!("Squint 25%")).clicked() {
                        action = Some(Action::Close(0.25));
                    }
                    if ui.button(tl!("Wink 85%")).clicked() {
                        action = Some(Action::Close(0.85));
                    }
                } else {
                    if ui.button(tl!("Smirk image-left")).clicked() {
                        action = Some(Action::Smirk(false));
                    }
                    if ui.button(tl!("Smirk image-right")).clicked() {
                        action = Some(Action::Smirk(true));
                    }
                }
            });
        }
        if ui.button(tl!("Hide guides (keep edit)")).clicked() {
            action = Some(Action::Hide);
        }
    }
    ui.small(tl!("No model required. Draw around ONE eye or mouth, including surrounding skin."));
    if !d.rig.status.is_empty() {
        ui.small(&d.rig.status);
    }
    if let Some(a) = action {
        act(d, a);
    }
}

fn act(d: &mut LiquifyDialog, action: Action) {
    d.end();
    match action {
        Action::Place(kind) => {
            d.rig.placing = Some(kind);
            d.rig.enabled = true;
            d.rig.guide = None;
            d.rig.status = "Drag a box around the feature and its surrounding skin".into();
        }
        Action::Hide => {
            d.rig.enabled = false;
            d.rig.drag = None;
        }
        Action::Close(f) => {
            let r = d.rig.guide.as_ref().ok_or_else(|| "Place guides first".to_string()).and_then(|r| r.close_eye(f));
            let _ = accept_macro(d, r);
        }
        Action::Smirk(side) => {
            let r = d.rig.guide.as_ref().ok_or_else(|| "Place guides first".to_string()).and_then(|r| r.smirk(side));
            let _ = accept_macro(d, r);
        }
    }
}

fn operation(rig: RigWarp) -> LiquifyStroke {
    let mut stroke = LiquifyStroke::new(LiquifyTool::RigWarpV1, 1.0);
    stroke.rig = Some(rig);
    stroke
}
fn accept_macro(d: &mut LiquifyDialog, rig: Result<RigWarp, String>) -> Result<(), String> {
    let result = rig.and_then(|r| {
        let area = photocraft_algo::liquify::rig::apply(&mut d.field, &r)?;
        if !area.is_empty() {
            d.redo.clear();
            d.strokes.push(operation(r.clone()));
            d.mark(area, false);
            d.render_dirty();
            d.rig.guide = Some(r.next_gesture());
        }
        // Fully protected or numerically unchanged gestures leave both the
        // history and guide positions as they were. No invisible "movement".
        Ok(!area.is_empty())
    });
    d.rig.status = match &result {
        Ok(true) => "Applied as one undoable geometric edit; texture repair may be needed".into(),
        Ok(false) => "No pixel movement: guides and history were kept unchanged".into(),
        Err(e) => e.clone(),
    };
    result.map(|_| ())
}

fn hit(rig: &RigWarp, p: [f64; 2], tolerance: f64) -> Option<usize> {
    rig.target
        .iter()
        .enumerate()
        .map(|(i, q)| (i, (q[0] - p[0]).hypot(q[1] - p[1])))
        .filter(|(_, d)| *d <= tolerance)
        .min_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(i, _)| i)
}
fn weights(rig: &RigWarp, p: [f64; 2], radius: f64, selected: Option<usize>, brush: bool, softness: f64, strength: f64) -> [f64; HANDLE_COUNT] {
    std::array::from_fn(|i| {
        if rig.pinned[i] {
            return 0.0;
        }
        if !brush {
            return if Some(i) == selected { 1.0 } else { 0.0 };
        }
        let distance = (rig.target[i][0] - p[0]).hypot(rig.target[i][1] - p[1]);
        photocraft_algo::liquify::precision::smooth_falloff(distance / radius.max(0.5), softness) * strength.clamp(0.0, 1.0)
    })
}

/// Consumes input only while manual mode is enabled. Input is in document space;
/// screen-space hit tolerance is converted once, not stored in command data.
pub(super) fn pointer(d: &mut LiquifyDialog, ev: &ToolEvent, mods: Modifiers) -> bool {
    if !d.rig.enabled || d.opts.view.before {
        return false;
    }
    let p = match *ev {
        ToolEvent::Down { x, y, .. } | ToolEvent::Move { x, y, .. } | ToolEvent::Up { x, y } => [x, y],
    };
    if p.iter().any(|v| !v.is_finite()) {
        return true;
    }
    let moved = d.rig.last_pointer != Some(p);
    d.rig.last_pointer = Some(p);
    match *ev {
        ToolEvent::Down { .. } => {
            // The UI normally suppresses duplicate Down events; preserve that
            // contract for control-channel callers too.
            if d.rig.press_active {
                return true;
            }
            finish(d);
            d.rig.press_active = true;
            if let Some(kind) = d.rig.placing {
                d.rig.drag = Some(Drag::Place { kind, start: p, end: p });
                return true;
            }
            let Some(r) = &mut d.rig.guide else {
                return true;
            };
            let selected = hit(r, p, 9.0 / f64::from(d.zoom.max(0.01)));
            if mods.shift
                && let Some(i) = selected
            {
                r.pinned[i] = !r.pinned[i];
                return true;
            }
            if d.rig.adjust {
                if let Some(index) = selected {
                    d.rig.drag = Some(Drag::Adjust { index });
                }
                return true;
            }
            let w = weights(r, p, f64::from(d.opts.size) * 0.5, selected, d.rig.brush, f64::from(d.opts.density) / 100.0, f64::from(d.opts.pressure) / 100.0);
            if w.iter().all(|v| *v == 0.0) {
                return true;
            }
            if d.field.d.len() > MAX_SNAPSHOT_NODES {
                d.rig.status = "Field exceeds manual-drag snapshot budget; use a smaller document".into();
                return true;
            }
            d.rig.drag = Some(Drag::Sculpt { base: d.field.clone(), before: Box::new(r.next_gesture()), start: p, weights: w, applied: None });
        }
        ToolEvent::Move { .. } => {
            if moved {
                update_drag(d, p);
            }
        }
        ToolEvent::Up { .. } => {
            if moved {
                update_drag(d, p);
            }
            finish(d);
        }
    }
    true
}

fn update_drag(d: &mut LiquifyDialog, p: [f64; 2]) {
    let Some(mut drag) = d.rig.drag.take() else {
        return;
    };
    match &mut drag {
        Drag::Place { end, .. } => *end = p,
        Drag::Adjust { index } => {
            if let Some(r) = &d.rig.guide {
                let mut new = r.clone();
                new.source[*index] = p;
                new.target[*index] = p;
                match new.validate() {
                    Ok(()) => {
                        d.rig.guide = Some(new);
                        d.rig.status.clear();
                    }
                    Err(e) => d.rig.status = e,
                }
            }
        }
        Drag::Sculpt { base, before, start, weights, applied } => {
            let mut r = before.as_ref().clone();
            for (i, w) in weights.iter().enumerate() {
                r.target[i] = [r.source[i][0] + (p[0] - start[0]) * w, r.source[i][1] + (p[1] - start[1]) * w];
            }
            // Always rebuild THIS gesture from the mouse-down field. This prevents
            // repeated mouse-move previews from accumulating interpolation error.
            let result = r.validate_grid(base.bounds, base.cell).and_then(|()| prepare_steps(&r).map(|_| ()));
            match result {
                Err(e) => d.rig.status = e,
                Ok(()) => {
                    let mut field = base.clone();
                    match photocraft_algo::liquify::rig::apply(&mut field, &r) {
                        Ok(area) => {
                            // Include the preceding valid preview area when returning to zero.
                            let halo = (base.cell * 2.0).ceil() as i32 + 2;
                            let damage = area.union(&r.rect().inflate(halo)).intersect(&base.bounds);
                            d.field = field;
                            if area.is_empty() {
                                d.rig.guide = Some(before.next_gesture());
                                *applied = None;
                            } else {
                                d.rig.guide = Some(r.next_gesture());
                                *applied = Some(Box::new(r));
                            }
                            d.mark(damage, false);
                            d.render_dirty();
                            d.rig.status.clear();
                        }
                        Err(e) => d.rig.status = e,
                    }
                }
            }
        }
    }
    d.rig.drag = Some(drag);
}

pub(super) fn finish(d: &mut LiquifyDialog) {
    d.rig.press_active = false;
    match d.rig.drag.take() {
        Some(Drag::Place { kind, start, end }) => {
            let b = [start[0].min(end[0]), start[1].min(end[1]), start[0].max(end[0]), start[1].max(end[1])];
            match RigWarp::new(kind, b).and_then(|r| {
                r.validate_grid(d.canvas, d.cell)?;
                Ok(r)
            }) {
                Ok(r) => {
                    d.rig.guide = Some(r);
                    d.rig.adjust = true;
                    d.rig.placing = None;
                    d.rig.status = "Correct the guide positions, then uncheck Adjust guides only to sculpt".into();
                }
                Err(e) => d.rig.status = e,
            }
        }
        Some(Drag::Sculpt { applied: Some(r), .. }) if !r.is_identity() => {
            d.redo.clear();
            d.strokes.push(operation(*r));
        }
        _ => {}
    }
}

/// Readable control protocol for placement and macros, independent of a mouse.
pub(super) fn control(d: &mut LiquifyDialog, v: &Value) -> Result<(), String> {
    if d.opts.view.before && ["closeEye", "smirk"].iter().any(|key| v.get(*key).is_some()) {
        return Err("Leave Before view before applying expression edits".into());
    }
    if let Some(value) = v.get("placeRig") {
        let kind: RegionKind = serde_json::from_value(value.get("kind").cloned().ok_or("placeRig needs kind")?).map_err(|e| e.to_string())?;
        let bounds: [f64; 4] = serde_json::from_value(value.get("bounds").cloned().ok_or("placeRig needs bounds")?).map_err(|e| e.to_string())?;
        let r = RigWarp::new(kind, bounds)?;
        r.validate_grid(d.canvas, d.cell)?;
        d.end();
        d.rig.guide = Some(r);
        d.rig.enabled = true;
        d.rig.adjust = true;
        d.rig.placing = None;
    }
    if let Some(b) = v.get("adjustGuides").and_then(Value::as_bool) {
        d.end();
        d.rig.adjust = b;
    }
    if let Some(b) = v.get("rigBrush").and_then(Value::as_bool) {
        d.rig.brush = b;
    }
    if let Some(b) = v.get("rigEnabled").and_then(Value::as_bool) {
        d.end();
        d.rig.enabled = b;
    }
    if let Some(value) = v.get("pinHandle") {
        let index = value.get("index").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()).ok_or("pinHandle needs index")?;
        let pinned = value.get("pinned").and_then(Value::as_bool).ok_or("pinHandle needs pinned")?;
        d.end();
        let r = d.rig.guide.as_mut().ok_or("Place a rig first")?;
        let slot = r.pinned.get_mut(index).ok_or("No such handle")?;
        *slot = pinned;
    }
    if let Some(v) = v.get("closeEye") {
        let f = v.as_f64().filter(|f| f.is_finite()).ok_or("closeEye must be finite")?;
        let rig = d.rig.guide.as_ref().ok_or("Place an eye first")?.close_eye(f)?;
        prepare_steps(&rig)?;
        d.end();
        accept_macro(d, Ok(rig))?;
    }
    if let Some(v) = v.get("smirk") {
        let right = match v.as_str() {
            Some("imageLeft") => false,
            Some("imageRight") => true,
            _ => return Err("smirk is imageLeft or imageRight".into()),
        };
        let rig = d.rig.guide.as_ref().ok_or("Place a mouth first")?.smirk(right)?;
        prepare_steps(&rig)?;
        d.end();
        accept_macro(d, Ok(rig))?;
    }
    Ok(())
}

/// Interpolated guide outline is an editing aid, not a claim that every curve
/// sample is a hard constraint. Only the eight marked handles are constraints.
fn contour(points: [[f64; 2]; HANDLE_COUNT]) -> Vec<[f64; 2]> {
    let mut out = Vec::with_capacity(HANDLE_COUNT * 12 + 1);
    for i in 0..HANDLE_COUNT {
        let [a, b, c, e] = [points[(i + HANDLE_COUNT - 1) % HANDLE_COUNT], points[i], points[(i + 1) % HANDLE_COUNT], points[(i + 2) % HANDLE_COUNT]];
        for k in 0..12 {
            let t = f64::from(k) / 12.0;
            out.push(std::array::from_fn(|j| {
                0.5 * ((2.0 * b[j])
                    + (-a[j] + c[j]) * t
                    + (2.0 * a[j] - 5.0 * b[j] + 4.0 * c[j] - e[j]) * t * t
                    + (-a[j] + 3.0 * b[j] - 3.0 * c[j] + e[j]) * t * t * t)
            }));
        }
    }
    out.push(points[0]);
    out
}

pub(super) fn draw(d: &LiquifyDialog, painter: &egui::Painter, area: ScreenRect) {
    if !d.rig.enabled || d.opts.view.before {
        return;
    }
    let tokens = crate::theme::Tokens::get(painter.ctx());
    let screen = |p| super::to_screen(d, area, p);
    if let Some(Drag::Place { start, end, .. }) = &d.rig.drag {
        painter.rect_stroke(ScreenRect::from_two_pos(screen(*start), screen(*end)), 0.0, Stroke::new(1.0, tokens.accent), egui::StrokeKind::Inside);
    }
    let Some(r) = &d.rig.guide else {
        return;
    };
    let line: Vec<Pos2> = contour(r.target).into_iter().map(screen).collect();
    painter.add(egui::Shape::line(line, Stroke::new(1.2, tokens.accent)));
    let hover = match &d.rig.drag {
        Some(Drag::Sculpt { weights, .. }) => *weights,
        _ => d
            .hover
            .map(|p| {
                weights(
                    r,
                    p,
                    f64::from(d.opts.size) * 0.5,
                    hit(r, p, 9.0 / f64::from(d.zoom.max(0.01))),
                    d.rig.brush,
                    f64::from(d.opts.density) / 100.0,
                    f64::from(d.opts.pressure) / 100.0,
                )
            })
            .unwrap_or([0.0; HANDLE_COUNT]),
    };
    for (i, p) in r.target.iter().enumerate() {
        let pos = screen(*p);
        painter.circle_filled(pos, if hover[i] > 0.0 { 5.0 } else { 3.5 }, if r.pinned[i] { tokens.danger } else { tokens.accent });
        painter.circle_stroke(pos, 5.5, Stroke::new(0.8, tokens.canvas));
        if r.pinned[i] {
            painter.line_segment([pos + egui::vec2(-3.0, -3.0), pos + egui::vec2(3.0, 3.0)], Stroke::new(1.0, tokens.text));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn larger_rig_brush_recruits_more_handles_without_moving_pins() {
        let mut r = RigWarp::new(RegionKind::Eye, [0.0, 0.0, 300.0, 180.0]).unwrap();
        r.pinned[0] = true;
        let p = r.target[2];
        let a = weights(&r, p, 8.0, Some(2), true, 0.65, 1.0);
        let b = weights(&r, p, 200.0, Some(2), true, 0.65, 1.0);
        assert!(a.iter().filter(|v| **v > 0.0).count() < b.iter().filter(|v| **v > 0.0).count());
        assert_eq!(b[0], 0.0);
    }
    #[test]
    fn contours_close_and_interpolate_marked_handles() {
        let r = RigWarp::new(RegionKind::Mouth, [0.0, 0.0, 300.0, 180.0]).unwrap();
        let c = contour(r.target);
        assert_eq!(c.first(), c.last());
        for i in 0..HANDLE_COUNT {
            assert_eq!(c[i * 12], r.target[i]);
        }
    }
}
