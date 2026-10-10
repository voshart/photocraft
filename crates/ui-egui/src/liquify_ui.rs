//! Filter › Liquify…: a full-window dialog like Photoshop's (tool strip on the left, properties on
//! the right, preview in the middle, OK / Cancel).
//!
//! The preview is a CPU proxy: the layer box-downsampled once to at most 1600 px
//! (`ProxyImage`), re-rendered through the displacement field only where a dab changed it
//! (bilinear field and source), and uploaded with a partial texture update. A dab plus its
//! update costs well under a millisecond on a 24 MP layer (see `bench_liquify`). Strokes are
//! recorded exactly as the engine replays them (`stroke_begin` + `stroke_segment` per point), so
//! OK runs `filter.liquify` with the same strokes and gets the same field.

use egui::{Align2, Color32, FontId, Pos2, Rect as ERect, Sense, Stroke, TextureHandle, pos2, vec2};
use photocraft_algo::liquify::{LiquifyField, LiquifyStroke, LiquifyTool, ProxyImage, auto_cell};
use photocraft_doc::LayerId;
use photocraft_geom::Rect;
use serde_json::{Value, json};

use crate::PhotocraftApp;
use crate::canvas::ToolEvent;
use crate::theme::Tokens;
use crate::widgets;

#[path = "liquify_face_ui.rs"]
mod face_ui;
#[path = "liquify_rig_ui.rs"]
mod manual_rig;
#[path = "liquify_precision_ui.rs"]
mod precision;
#[cfg(test)]
#[path = "liquify_precision_session_tests.rs"]
mod precision_session_tests;
#[cfg(test)]
#[path = "liquify_rig_session_tests.rs"]
mod rig_session_tests;
#[path = "liquify_session_ui.rs"]
mod session;
#[cfg(test)]
#[path = "liquify_smart_session_tests.rs"]
mod smart_session_tests;

/// Longest side of the preview proxy (px).
const PROXY_SIDE: usize = 1600;
const LEFT_W: f32 = 48.0;
const RIGHT_W: f32 = 340.0;
const REDO_STACK_LIMIT: usize = 100;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PreviewTool {
    Brush,
    Hand,
    Zoom,
}

fn preview_zoom_factor(ctx: &egui::Context) -> f32 {
    let notch = ctx.options(|o| o.input_options.line_scroll_speed).max(1.0);
    ctx.input(|i| {
        let pinch = i.zoom_delta();
        if pinch.is_finite() && pinch > 0.0 && pinch != 1.0 {
            return pinch;
        }
        // Main-canvas navigation folds Ctrl+wheel into horizontal smoothing.
        // Read original events so Liquify's wheel steps are immediate and independent of that policy.
        let points: f32 = i
            .events
            .iter()
            .filter_map(|event| match event {
                egui::Event::MouseWheel { unit, delta, .. } => {
                    let scale = match unit {
                        egui::MouseWheelUnit::Point => 1.0,
                        egui::MouseWheelUnit::Line => notch,
                        egui::MouseWheelUnit::Page => i.viewport_rect().height(),
                    };
                    Some((delta.x + delta.y) * scale)
                }
                _ => None,
            })
            .sum();
        if points.is_finite() { crate::wheel_nav::NOTCH.powf(points / notch) } else { 1.0 }
    })
}

fn zoom_at(d: &mut LiquifyDialog, area: ERect, anchor: Pos2, zoom: f32) {
    if !zoom.is_finite() || !anchor.x.is_finite() || !anchor.y.is_finite() {
        return;
    }
    let before = to_doc(d, area, anchor);
    d.zoom = zoom.clamp(0.01 / d.pixels_per_point, 32.0 / d.pixels_per_point);
    d.zoom_fit = false;
    let after = to_doc(d, area, anchor);
    d.center = [d.center[0] + before[0] - after[0], d.center[1] + before[1] - after[1]];
}

fn view_control(d: &mut LiquifyDialog, value: &Value) -> Result<(), String> {
    if let Some(v) = value.get("zoomPercent") {
        let percent = v.as_f64().filter(|v| v.is_finite() && (1.0..=3200.0).contains(v)).ok_or("zoomPercent must be 1..3200")?;
        d.zoom = (percent / 100.0) as f32 / d.pixels_per_point;
        d.zoom_fit = false;
    }
    if let Some(v) = value.get("fitView") {
        d.zoom_fit = v.as_bool().ok_or("fitView must be true or false")?;
    }
    if let Some(v) = value.get("previewTool") {
        let tool = match v.as_str() {
            Some("brush") => PreviewTool::Brush,
            Some("hand") => PreviewTool::Hand,
            Some("zoom") => PreviewTool::Zoom,
            _ => return Err("previewTool is brush, hand or zoom".into()),
        };
        d.end();
        d.preview_tool = tool;
    }
    Ok(())
}

#[cfg(test)]
mod view_tests {
    use super::*;
    #[test]
    fn liquify_view_accepts_pinch_and_ctrl_wheel_without_editing_pixels() {
        let ctx = egui::Context::default();
        PhotocraftApp::setup_context(&ctx, Default::default());
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
        app.session.execute("file.new", json!({"width":512,"height":384})).unwrap();
        open(&mut app, &ctx).unwrap();
        let screen = ERect::from_min_size(Pos2::ZERO, vec2(1280.0, 900.0));
        let position = pos2(370.0, 240.0);
        let frame = |app: &mut PhotocraftApp, events| {
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), events, ..Default::default() }, |ui| show(app, ui.ctx()));
            output.textures_delta.clear();
        };
        frame(&mut app, vec![egui::Event::PointerMoved(position)]);
        // Areas need an initial sizing/hover pass before they receive pointer input.
        for _ in 0..3 {
            frame(&mut app, Vec::new());
        }
        let before = app.distort.liquify.as_ref().unwrap().zoom;
        frame(&mut app, vec![egui::Event::Zoom(1.5)]);
        let pinch = app.distort.liquify.as_ref().unwrap().zoom;
        assert!((pinch - before * 1.5).abs() < 0.001, "before={before}, pinch={pinch}");
        frame(
            &mut app,
            vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, 60.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
            }],
        );
        // Wheel deltas are smoothed across frames by egui, unlike a pinch event.
        for _ in 0..4 {
            frame(&mut app, Vec::new());
        }
        let d = app.distort.liquify.as_ref().unwrap();
        assert!(d.zoom > pinch, "pinch={pinch}, ctrl-wheel={}", d.zoom);
        assert!(d.strokes.is_empty());
        assert!(d.field.is_identity());
    }
    #[test]
    fn liquify_zoom_keeps_cursor_point_fixed_and_rejects_invalid_zoom() {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
        app.session.execute("file.new", json!({"width":512,"height":384})).unwrap();
        open(&mut app, &egui::Context::default()).unwrap();
        let d = app.distort.liquify.as_mut().unwrap();
        d.zoom = 0.8;
        let area = ERect::from_min_size(pos2(48.0, 30.0), vec2(800.0, 600.0));
        let anchor = pos2(231.0, 179.0);
        let original = to_doc(d, area, anchor);
        for zoom in [2.0, 0.25, 32.0, 0.01, 0.8] {
            zoom_at(d, area, anchor, zoom);
            let after = to_doc(d, area, anchor);
            assert!((after[0] - original[0]).abs() < 0.001);
            assert!((after[1] - original[1]).abs() < 0.001);
        }
        let previous = d.zoom;
        zoom_at(d, area, anchor, f32::NAN);
        assert_eq!(d.zoom, previous);
        assert!(view_control(d, &json!({"zoomPercent":-1})).is_err());
        assert!(view_control(d, &json!({"zoomPercent":"big"})).is_err());
        view_control(d, &json!({"zoomPercent":100,"previewTool":"hand"})).unwrap();
        assert_eq!(d.zoom, 1.0);
        assert!(d.preview_tool == PreviewTool::Hand);
        assert!(!d.zoom_fit);
        assert!(d.strokes.is_empty());
        d.pixels_per_point = 2.0;
        view_control(d, &json!({"zoomPercent":100})).unwrap();
        assert_eq!(d.zoom, 0.5);
        assert_eq!(d.describe()["zoomPercent"], 100.0);
        let start = to_screen(d, area, [100.0, 100.0]);
        let end = to_screen(d, area, [110.0, 100.0]);
        assert_eq!((end.x - start.x) * d.pixels_per_point, 10.0);
    }
}

/// Options shown in the properties panel (and settable over the control channel).
#[derive(Clone, Debug)]
pub struct LiquifyOpts {
    pub tool: LiquifyTool,
    pub size: f32,
    pub density: f32,
    pub pressure: f32,
    pub rate: f32,
    pub show_mesh: bool,
    /// 0 small, 1 medium, 2 large.
    pub mesh_size: usize,
    pub show_mask: bool,
    pub show_backdrop: bool,
    pub backdrop_opacity: f32,
    pub reconstruct_amount: f32,
    pub(crate) view: precision::ViewOptions,
}

impl Default for LiquifyOpts {
    fn default() -> Self {
        LiquifyOpts {
            tool: LiquifyTool::ForwardWarp,
            size: 100.0,
            density: 50.0,
            pressure: 100.0,
            rate: 80.0,
            show_mesh: false,
            mesh_size: 1,
            show_mask: true,
            show_backdrop: false,
            backdrop_opacity: 50.0,
            reconstruct_amount: 100.0,
            view: precision::ViewOptions::default(),
        }
    }
}

/// The `prefs.dialogs` key under which the brush settings are remembered between uses and
/// launches (#418).
const REMEMBERED: &str = "filter.liquify";

impl LiquifyOpts {
    /// Applies the settings named in `v` (the control channel's and the remembered keys),
    /// clamped to their ranges; the others are left as they are. A bad `tool` is an error, after
    /// everything else has been applied.
    pub fn apply(&mut self, v: &Value) -> Result<(), String> {
        self.view.apply(v.get("precisionView").unwrap_or(v))?;
        let num = |k: &str| v.get(k).and_then(Value::as_f64).filter(|n| n.is_finite()).map(|n| n as f32);
        let flag = |k: &str| v.get(k).and_then(Value::as_bool);
        if let Some(n) = num("size") {
            self.size = n.clamp(1.0, 15000.0);
        }
        for (k, slot) in
            [("density", &mut self.density), ("pressure", &mut self.pressure), ("rate", &mut self.rate), ("backdropOpacity", &mut self.backdrop_opacity)]
        {
            if let Some(n) = num(k) {
                *slot = n.clamp(0.0, 100.0);
            }
        }
        for (k, slot) in [("showMesh", &mut self.show_mesh), ("showMask", &mut self.show_mask), ("showBackdrop", &mut self.show_backdrop)] {
            if let Some(b) = flag(k) {
                *slot = b;
            }
        }
        if let Some(m) = v.get("meshSize").and_then(Value::as_str) {
            self.mesh_size = match m {
                "small" => 0,
                "large" => 2,
                _ => 1,
            };
        }
        if let Some(t) = v.get("tool") {
            let tool: LiquifyTool = serde_json::from_value(t.clone()).map_err(|e| format!("bad tool: {e}"))?;
            if matches!(tool, LiquifyTool::RigWarpV1 | LiquifyTool::FaceWarpV1 | LiquifyTool::ResetSessionV1) {
                return Err("Use manual-rig controls or Restore All, not a non-brush tool selection".into());
            }
            self.tool = tool;
        }
        Ok(())
    }

    /// The settings as [`LiquifyOpts::apply`] reads them.
    pub fn to_json(&self) -> Value {
        let mesh = ["small", "medium", "large"].get(self.mesh_size).copied().unwrap_or("medium");
        json!({
            "tool": self.tool,
            "size": self.size,
            "density": self.density,
            "pressure": self.pressure,
            "rate": self.rate,
            "showMesh": self.show_mesh,
            "meshSize": mesh,
            "showMask": self.show_mask,
            "showBackdrop": self.show_backdrop,
            "backdropOpacity": self.backdrop_opacity,
            "precisionView": self.view.remembered(),
        })
    }
}

pub struct LiquifyDialog {
    pub layer: LayerId,
    layer_name: String,
    canvas: Rect,
    cell: f64,
    pub field: LiquifyField,
    pub strokes: Vec<LiquifyStroke>,
    redo: Vec<LiquifyStroke>,
    /// The stroke being drawn and its last point.
    cur: Option<(LiquifyStroke, [f64; 3])>,
    /// The lasso polygon being drawn (document px) and whether it thaws (Alt held at pointer-down).
    lasso: Option<(bool, Vec<[f64; 2]>)>,
    proxy: ProxyImage,
    detail: precision::DetailPreview,
    rig: manual_rig::ManualRig,
    face: face_ui::FaceControls,
    origin: session::Origin,
    out: Vec<[u8; 4]>,
    tex: Option<TextureHandle>,
    /// Proxy pixels to re-render and upload (x0, y0, x1, y1).
    dirty: Option<[usize; 4]>,
    backdrop: Option<TextureHandle>,
    mask_tex: Option<TextureHandle>,
    mask_dirty: bool,
    pub opts: LiquifyOpts,
    /// Screen px per document px (0 = fit on next frame) and the view centre (document px).
    zoom: f32,
    pixels_per_point: f32,
    zoom_fit: bool,
    preview_tool: PreviewTool,
    center: [f64; 2],
    hover: Option<[f64; 2]>,
    last_dab: f64,
    /// Last dab + proxy update time (ms), shown in the footer.
    pub dab_ms: f64,
}

fn union(a: Option<[usize; 4]>, b: [usize; 4]) -> Option<[usize; 4]> {
    if b[0] >= b[2] || b[1] >= b[3] {
        return a;
    }
    Some(match a {
        Some(a) => [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])],
        None => b,
    })
}

impl LiquifyDialog {
    pub fn describe(&self) -> Value {
        json!({
            "layer": self.layer.0,
            "tool": self.opts.tool,
            "size": self.opts.size,
            "zoomPercent": self.zoom * self.pixels_per_point * 100.0,
            "fitView":self.zoom_fit,
            "strokes": self.strokes.len() + usize::from(self.cur.is_some()),
            "redo": self.redo.len(),
            "meshSize": self.cell,
            "maxDisplacement": self.field.max_displacement(),
            "proxy": [self.proxy.w, self.proxy.h],
            "dabMs": self.dab_ms,
            "detailPreview": self.detail.describe(),
            "manualRig": self.rig.describe(),
            "faceControls": self.face.describe(),
            "session": self.origin.describe(),
            "precisionView": self.opts.view.remembered(),
            "referenceSource": self.opts.view.reference,
            "showBefore": self.opts.view.before,
        })
    }

    fn template(&self, tool: LiquifyTool) -> LiquifyStroke {
        let o = &self.opts;
        LiquifyStroke {
            tool,
            size: f64::from(o.size),
            density: f64::from(o.density),
            pressure: f64::from(o.pressure),
            rate: f64::from(o.rate),
            points: Vec::new(),
            amount: None,
            rig: None,
            face: None,
        }
    }

    fn mark(&mut self, doc_rect: Rect, mask: bool) {
        if doc_rect.is_empty() {
            return;
        }
        self.detail.invalidate();
        self.dirty = union(self.dirty, self.proxy.proxy_rect(doc_rect));
        self.mask_dirty |= mask;
    }

    /// Starts a stroke; `alt` draws with the selected tool's opposite (see [`alt_tool`]).
    fn begin(&mut self, p: [f64; 3], now: f64, alt: bool) {
        self.face.photo_changed();
        self.redo.clear();
        let tool = if alt { alt_tool(self.opts.tool) } else { self.opts.tool };
        if manual_rig::changes_geometry(tool) {
            self.rig.raw_started();
        }
        let mut s = self.template(tool);
        s.points.push(p.to_vec());
        let t0 = crate::gpu_canvas::now_ms();
        let d = self.field.stroke_begin(&s, p);
        self.mark(d, matches!(s.tool, LiquifyTool::Freeze | LiquifyTool::Thaw));
        self.render_dirty();
        self.dab_ms = crate::gpu_canvas::now_ms() - t0;
        self.cur = Some((s, p));
        self.last_dab = now;
    }

    fn extend(&mut self, p: [f64; 3], now: f64) {
        let Some((mut s, last)) = self.cur.take() else { return };
        let t0 = crate::gpu_canvas::now_ms();
        let d = self.field.stroke_segment(&s, last, p);
        s.points.push(p.to_vec());
        self.mark(d, matches!(s.tool, LiquifyTool::Freeze | LiquifyTool::Thaw));
        self.render_dirty();
        self.dab_ms = crate::gpu_canvas::now_ms() - t0;
        self.cur = Some((s, p));
        self.last_dab = now;
    }

    fn end(&mut self) {
        manual_rig::finish(self);
        if let Some((s, _)) = self.cur.take() {
            self.strokes.push(s);
        }
    }

    /// Closes the lasso polygon into the freeze mask, recorded like any stroke (so undo, replay
    /// and OK all treat it the same as Freeze/Thaw brush work).
    fn close_lasso(&mut self, subtract: bool, mut pts: Vec<[f64; 2]>) {
        if pts.len() < 3 {
            return;
        }
        if let (Some(&first), Some(&last)) = (pts.first(), pts.last())
            && ((first[0] - last[0]).abs() > 0.5 || (first[1] - last[1]).abs() > 0.5)
        {
            pts.push(first);
        }
        self.redo.clear();
        let mut s = self.template(LiquifyTool::LassoMask);
        s.amount = Some(if subtract { 0.0 } else { 1.0 });
        s.points = pts.into_iter().map(|p| p.to_vec()).collect();
        let d = self.field.apply_stroke(&s);
        self.strokes.push(s);
        self.mark(d, true);
        self.render_dirty();
    }

    /// Applies a whole-field operation (Reconstruct…, mask buttons) as a stroke.
    fn global(&mut self, tool: LiquifyTool, amount: Option<f64>) {
        self.end();
        self.redo.clear();
        let mut s = LiquifyStroke::new(tool, 1.0);
        s.amount = amount;
        let d = self.field.apply_stroke(&s);
        self.strokes.push(s);
        self.mark(d, true);
        self.render_dirty();
    }

    fn rebuild(&mut self) {
        self.detail.invalidate();
        self.field = LiquifyField::from_strokes(self.canvas, self.cell, &self.strokes);
        self.rig.recover(&self.strokes);
        self.rig.recover_next(self.redo.last());
        face_ui::sync_history(self);
        self.dirty = Some([0, 0, self.proxy.w, self.proxy.h]);
        self.mask_dirty = true;
        self.render_dirty();
    }

    fn undo(&mut self) {
        self.end();
        if let Some(stroke) = self.strokes.pop() {
            let guides = self.rig.preserve_for(stroke.tool);
            if self.redo.len() == REDO_STACK_LIMIT {
                self.redo.remove(0);
            }
            self.redo.push(stroke);
            self.rebuild();
            self.rig.restore_preserved(guides);
        }
    }

    fn redo(&mut self) {
        self.end();
        if let Some(stroke) = self.redo.pop() {
            let guides = self.rig.preserve_for(stroke.tool);
            let d = self.field.apply_stroke(&stroke);
            self.strokes.push(stroke);
            self.rig.recover(&self.strokes);
            self.rig.recover_next(self.redo.last());
            self.rig.restore_preserved(guides);
            face_ui::sync_history(self);
            self.mark(d, true);
            self.render_dirty();
        }
    }

    fn restore_all(&mut self) {
        self.end();
        self.lasso = None;
        if self.field.is_identity() && self.field.freeze.iter().all(|f| *f == 0.0) {
            return;
        }
        self.redo.clear();
        self.strokes.push(LiquifyStroke::new(LiquifyTool::ResetSessionV1, 1.0));
        self.rebuild();
    }

    /// Re-renders the dirty proxy pixels into `out` (the texture upload happens in `show`).
    fn render_dirty(&mut self) {
        if let Some(r) = self.dirty {
            self.proxy.render(&self.field, r, &mut self.out);
            self.detail.mix_proxy(&self.proxy, &mut self.out, r);
        }
    }

    fn upload(&mut self, ctx: &egui::Context) {
        let (w, h) = (self.proxy.w, self.proxy.h);
        match (&mut self.tex, self.dirty.take()) {
            (None, _) => {
                self.proxy.render(&self.field, [0, 0, w, h], &mut self.out);
                self.detail.mix_proxy(&self.proxy, &mut self.out, [0, 0, w, h]);
                let img = egui::ColorImage::new([w, h], self.out.iter().map(|p| Color32::from_rgba_premultiplied(p[0], p[1], p[2], p[3])).collect());
                self.tex = Some(ctx.load_texture("liquify-proxy", img, egui::TextureOptions::LINEAR));
            }
            (Some(tex), Some(r)) => {
                let (x0, y0, x1, y1) = (r[0].min(w), r[1].min(h), r[2].min(w), r[3].min(h));
                if x1 > x0 && y1 > y0 {
                    let mut px = Vec::with_capacity((x1 - x0) * (y1 - y0));
                    for y in y0..y1 {
                        px.extend(self.out[y * w + x0..y * w + x1].iter().map(|p| Color32::from_rgba_premultiplied(p[0], p[1], p[2], p[3])));
                    }
                    tex.set_partial([x0, y0], egui::ColorImage::new([x1 - x0, y1 - y0], px), egui::TextureOptions::LINEAR);
                }
            }
            _ => {}
        }
        if self.mask_dirty || self.mask_tex.is_none() {
            self.mask_dirty = false;
            let f = &self.field;
            let img = egui::ColorImage::new(
                [f.w, f.h],
                f.freeze.iter().map(|v| Color32::from_rgba_unmultiplied(230, 40, 40, (v.clamp(0.0, 1.0) * 140.0) as u8)).collect(),
            );
            match &mut self.mask_tex {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => self.mask_tex = Some(ctx.load_texture("liquify-mask", img, egui::TextureOptions::LINEAR)),
            }
        }
    }
}

/// Opens the dialog on the active layer.
pub fn open(app: &mut PhotocraftApp, ctx: &egui::Context) -> Result<(), String> {
    let (layer, surf, _) = crate::distort_ui::active_pixels(app)?;
    open_pixels(app, ctx, layer, surf, true, None)
}

/// Reopen the existing operation, not another filter on top of its output.
pub fn open_smart_filter(app: &mut PhotocraftApp, ctx: &egui::Context, layer: LayerId, index: usize) -> Result<(), String> {
    session::open_existing(app, ctx, layer, index)
}

fn open_pixels(
    app: &mut PhotocraftApp,
    ctx: &egui::Context,
    layer: LayerId,
    surf: photocraft_raster::Surface,
    selection_preview: bool,
    field_cell: Option<f64>,
) -> Result<(), String> {
    if app.distort.liquify.is_some() {
        return Err("Finish or cancel the existing Liquify session first".into());
    }
    let st = app.session.active().ok_or("no document")?;
    let canvas = st.doc.bounds();
    let name = st.doc.layer(layer).map(|l| l.name.clone()).unwrap_or_default();
    let cell = field_cell.unwrap_or_else(|| auto_cell(canvas));
    session::check_field_budget(canvas, cell)?;
    let proxy = ProxyImage::new(&surf, canvas, PROXY_SIDE);
    let out = vec![[0u8; 4]; proxy.w * proxy.h];
    let center = [f64::from(canvas.x0 + canvas.x1) / 2.0, f64::from(canvas.y0 + canvas.y1) / 2.0];
    let mut d = LiquifyDialog {
        layer,
        layer_name: name,
        canvas,
        cell,
        field: LiquifyField::new(canvas, cell),
        strokes: Vec::new(),
        redo: Vec::new(),
        cur: None,
        lasso: None,
        proxy,
        detail: precision::DetailPreview::new(surf.clone(), canvas, &st.doc, layer),
        rig: manual_rig::ManualRig::default(),
        face: face_ui::FaceControls::default(),
        origin: session::Origin::new(st.doc.clone()),
        out,
        tex: None,
        dirty: None,
        backdrop: None,
        mask_tex: None,
        mask_dirty: true,
        opts: LiquifyOpts::default(),
        zoom: 0.0,
        pixels_per_point: ctx.pixels_per_point().max(0.01),
        zoom_fit: true,
        preview_tool: PreviewTool::Brush,
        center,
        hover: None,
        last_dab: 0.0,
        dab_ms: 0.0,
    };
    // A sensible starting brush (about a tenth of the image), then the settings last used, so
    // Liquify opens as it was left (#418). Saved values are clamped; a bad one is skipped.
    d.opts.size = ((canvas.width().max(canvas.height()) as f32) / 10.0).round().clamp(10.0, 1500.0);
    if let Some(saved) = app.session.prefs().dialogs.get(REMEMBERED) {
        let _ = d.opts.apply(saved);
    }
    if !selection_preview {
        d.detail.set_isolated();
    }
    d.upload(ctx);
    app.distort.liquify = Some(d);
    Ok(())
}

/// Keeps the brush settings for the next time Liquify opens, also after a restart.
fn remember(app: &mut PhotocraftApp, opts: &LiquifyOpts) {
    let v = opts.to_json();
    app.session.prefs.edit(|p| p.dialogs.insert(REMEMBERED.into(), v));
}

/// Cancel (button, Esc or the control channel): the document is untouched; the brush settings
/// are kept.
pub fn cancel(app: &mut PhotocraftApp) {
    if session::request_cancel(app) {
        return;
    }
    if let Some(d) = app.distort.liquify.take() {
        remember(app, &d.opts);
    }
}

/// OK: runs `filter.liquify` with the recorded strokes (one history step).
pub fn commit(app: &mut PhotocraftApp) {
    session::commit(app);
}

/// Control channel: `filter.liquify {"ui": {...}}` while the dialog is open.
pub fn control(app: &mut PhotocraftApp, ui: &Value) -> Result<Value, String> {
    if ui.get("commit").and_then(Value::as_bool) == Some(true) {
        commit(app);
        return session::commit_reply(app);
    }
    if ui.get("cancel").and_then(Value::as_bool) == Some(true) {
        cancel(app);
        return Ok(json!({"cancelled": app.distort.liquify.is_none(), "pending": session::has_pending(app)}));
    }
    if session::has_pending(app) {
        return Err("Liquify Apply is still running; cancel it before editing".into());
    }
    let d = app.distort.liquify.as_mut().ok_or(tl!("Liquify is not open"))?;
    if ui.get("tool").is_some() {
        d.end();
        d.rig.enabled = false;
        d.preview_tool = PreviewTool::Brush;
        d.face.enabled = false;
    }
    view_control(d, ui)?;
    d.opts.apply(ui)?;
    manual_rig::control(d, ui)?;
    face_ui::control(d, ui)?;
    let num = |k: &str| ui.get(k).and_then(Value::as_f64).map(|v| v as f32);
    let flag = |k: &str| ui.get(k).and_then(Value::as_bool);
    if flag("undo") == Some(true) {
        d.undo();
    }
    if flag("redo") == Some(true) {
        d.redo();
    }
    if flag("restoreAll") == Some(true) {
        d.restore_all();
    }
    if let Some(a) = num("reconstruct") {
        d.global(LiquifyTool::ReconstructAll, Some(f64::from(a)));
    }
    for (k, t) in [("maskAll", LiquifyTool::FreezeAll), ("maskNone", LiquifyTool::ThawAll), ("invertMask", LiquifyTool::InvertFreeze)] {
        if flag(k) == Some(true) {
            d.global(t, None);
        }
    }
    Ok(d.describe())
}

/// Photoshop's Alt (Option) variant of a Liquify brush: the twirls reverse direction and Pucker
/// and Bloat swap. Other tools are unchanged.
fn alt_tool(t: LiquifyTool) -> LiquifyTool {
    match t {
        LiquifyTool::TwirlCw => LiquifyTool::TwirlCcw,
        LiquifyTool::TwirlCcw => LiquifyTool::TwirlCw,
        LiquifyTool::Pucker => LiquifyTool::Bloat,
        LiquifyTool::Bloat => LiquifyTool::Pucker,
        t => t,
    }
}

/// Pointer in document coordinates (from the preview or the control channel). Alt at
/// pointer-down subtracts from the freeze mask (lasso), reverses a twirl and swaps Pucker/Bloat;
/// Shift adds to the mask, same as no modifier.
pub fn pointer(app: &mut PhotocraftApp, ev: ToolEvent, mods: egui::Modifiers) {
    let now = crate::gpu_canvas::now_ms();
    let Some(d) = app.distort.liquify.as_mut() else { return };
    if d.origin.busy() {
        return;
    }
    if d.opts.view.before || d.face.enabled || d.preview_tool != PreviewTool::Brush {
        d.end();
        return;
    }
    if manual_rig::pointer(d, &ev, mods) {
        return;
    }
    match ev {
        ToolEvent::Down { x, y, pressure } => {
            if d.opts.tool == LiquifyTool::LassoMask {
                d.lasso = Some((mods.alt, vec![[x, y]]));
            } else {
                d.begin([x, y, f64::from(pressure)], now, mods.alt);
            }
        }
        ToolEvent::Move { x, y, pressure } => {
            d.hover = Some([x, y]);
            if let Some((_, pts)) = &mut d.lasso {
                if pts.last().is_none_or(|&l| l[0] != x || l[1] != y) {
                    pts.push([x, y]);
                }
            } else if d.cur.is_some() {
                d.extend([x, y, f64::from(pressure)], now);
            }
        }
        ToolEvent::Up { x, y } => {
            if let Some((subtract, pts)) = d.lasso.take() {
                d.close_lasso(subtract, pts);
                return;
            }
            if let Some((_, last)) = d.cur
                && (last[0] != x || last[1] != y)
            {
                d.extend([x, y, last[2]], now);
            }
            d.end();
        }
    }
}

pub fn keys(app: &mut PhotocraftApp, ctx: &egui::Context) {
    if session::has_pending(app) {
        return;
    }
    let Some(d) = app.distort.liquify.as_mut() else { return };
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Num0)) {
        d.zoom_fit = true;
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Num1)) {
        d.zoom = 1.0 / d.pixels_per_point;
        d.zoom_fit = false;
    }
    let zoom_in = ctx.input_mut(|i| {
        i.consume_key(egui::Modifiers::COMMAND, egui::Key::Plus)
            || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Equals)
            || i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Plus)
    });
    let zoom_out = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Minus));
    if zoom_in || zoom_out {
        let zoom = if d.zoom > 0.0 { d.zoom } else { 1.0 / d.pixels_per_point };
        d.zoom = (zoom * if zoom_in { 1.25 } else { 0.8 }).clamp(0.01 / d.pixels_per_point, 32.0 / d.pixels_per_point);
        d.zoom_fit = false;
    }
    let cmd_shift = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
    if ctx.input_mut(|i| i.consume_key(cmd_shift, egui::Key::Z)) {
        d.redo();
    } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z)) {
        d.undo();
    }
    // Ctrl+H (Hide Extras) toggles the red freeze-mask overlay; the mask itself stays active.
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::H)) {
        d.opts.show_mask = !d.opts.show_mask;
    }
    // Ctrl+I inverts the freeze mask (the Invert All button); Ctrl+D clears it (None).
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::I)) {
        d.global(LiquifyTool::InvertFreeze, None);
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::D)) {
        d.global(LiquifyTool::ThawAll, None);
    }
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::H)) {
        d.end();
        d.preview_tool = PreviewTool::Hand;
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Z)) {
        d.end();
        d.preview_tool = PreviewTool::Zoom;
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::OpenBracket)) {
        d.opts.size = (d.opts.size * 0.9).max(1.0);
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::CloseBracket)) {
        d.opts.size = (d.opts.size * 1.1).min(15000.0);
    }
    let tools = [
        (egui::Key::W, LiquifyTool::ForwardWarp),
        (egui::Key::P, LiquifyTool::PrecisionWarp),
        (egui::Key::R, LiquifyTool::Reconstruct),
        (egui::Key::E, LiquifyTool::Smooth),
        (egui::Key::C, LiquifyTool::TwirlCw),
        (egui::Key::S, LiquifyTool::Pucker),
        (egui::Key::B, LiquifyTool::Bloat),
        (egui::Key::O, LiquifyTool::PushLeft),
        (egui::Key::F, LiquifyTool::Freeze),
        (egui::Key::D, LiquifyTool::Thaw),
        (egui::Key::L, LiquifyTool::LassoMask),
    ];
    for (k, t) in tools {
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, k)) {
            d.end();
            d.rig.enabled = false;
            d.opts.tool = t;
            d.preview_tool = PreviewTool::Brush;
            d.face.enabled = false;
        }
    }
}

fn tool_icon(t: LiquifyTool) -> &'static str {
    match t {
        LiquifyTool::ForwardWarp | LiquifyTool::PrecisionWarp => "pointer",
        LiquifyTool::Reconstruct => "brush-cleaning",
        LiquifyTool::Smooth => "droplet",
        LiquifyTool::TwirlCw => "rotate-cw",
        LiquifyTool::TwirlCcw => "undo-2",
        LiquifyTool::Pucker => "scan",
        LiquifyTool::Bloat => "maximize-2",
        LiquifyTool::PushLeft => "chevrons-left",
        LiquifyTool::Freeze => "lock",
        LiquifyTool::Thaw => "lock-open",
        LiquifyTool::LassoMask => "lasso",
        _ => "circle",
    }
}

fn shortcut(t: LiquifyTool) -> &'static str {
    match t {
        LiquifyTool::ForwardWarp => "W",
        LiquifyTool::PrecisionWarp => "P",
        LiquifyTool::Reconstruct => "R",
        LiquifyTool::Smooth => "E",
        LiquifyTool::TwirlCw => "C",
        LiquifyTool::Pucker => "S",
        LiquifyTool::Bloat => "B",
        LiquifyTool::PushLeft => "O",
        LiquifyTool::Freeze => "F",
        LiquifyTool::Thaw => "D",
        LiquifyTool::LassoMask => "L",
        _ => "",
    }
}

fn load_backdrop(app: &PhotocraftApp, ctx: &egui::Context, layer: LayerId, canvas: Rect) -> Option<TextureHandle> {
    let st = app.session.active()?;
    let doc = crate::distort_ui::without_layer(&st.doc, layer);
    let buf = photocraft_compose::render(&doc, canvas);
    let mut s = photocraft_raster::Surface::new(photocraft_color::PixelFormat::RGBA8);
    let flat: Vec<f32> = buf.px.iter().flat_map(|p| *p).collect();
    s.write_region(canvas, &flat);
    Some(ctx.load_texture("liquify-backdrop", crate::distort_ui::surface_image(&s, canvas, PROXY_SIDE), egui::TextureOptions::LINEAR))
}

/// Draws the dialog (a full-window layer over the app).
pub fn show(app: &mut PhotocraftApp, ctx: &egui::Context) {
    if session::poll_pending(app, ctx) {
        return;
    }
    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    if app.distort.liquify.as_ref().is_some_and(|d| d.opts.show_backdrop && d.opts.view.reference == precision::Reference::OtherVisible && d.backdrop.is_none())
    {
        let tex = app.distort.liquify.as_ref().map(|d| (d.layer, d.canvas)).and_then(|(layer, canvas)| load_backdrop(app, ctx, layer, canvas));
        if let Some(d) = app.distort.liquify.as_mut() {
            d.backdrop = tex;
        }
    }
    let mut action: Option<&str> = None;
    let mut events: Vec<ToolEvent> = Vec::new();
    let mut mods = egui::Modifiers::NONE;
    egui::Area::new(egui::Id::new("liquify-dialog")).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
        let Some(d) = app.distort.liquify.as_mut() else { return };
        let density = ctx.pixels_per_point().max(0.01);
        if density != d.pixels_per_point {
            d.zoom *= d.pixels_per_point / density;
            d.pixels_per_point = density;
        }
        face_ui::poll(d, ctx);
        d.upload(ctx);
        let (full, _) = ui.allocate_exact_size(screen.size(), Sense::hover());
        let painter = ui.painter().clone();
        painter.rect_filled(full, 0.0, t.chrome);
        // Title strip.
        let title = ERect::from_min_size(full.min, vec2(full.width(), 30.0));
        painter.rect_filled(title, 0.0, t.dock);
        painter.line_segment([title.left_bottom(), title.right_bottom()], Stroke::new(1.0, t.separator));
        let pct = if d.zoom > 0.0 { d.zoom * d.pixels_per_point * 100.0 } else { 100.0 };
        painter.text(
            title.center(),
            Align2::CENTER_CENTER,
            format!("{} ({}, {:.0}%)", tl!("Liquify…").trim_end_matches('…'), d.layer_name, pct),
            FontId::proportional(13.0),
            t.text,
        );
        let body = ERect::from_min_max(pos2(full.left(), title.bottom()), full.max);
        // Left tool strip.
        let left = ERect::from_min_size(body.min, vec2(LEFT_W, body.height()));
        painter.rect_filled(left, 0.0, t.dock);
        painter.line_segment([left.right_top(), left.right_bottom()], Stroke::new(1.0, t.separator));
        let mut strip = ui.new_child(egui::UiBuilder::new().max_rect(left.shrink2(vec2(6.0, 8.0))));
        strip.spacing_mut().item_spacing.y = 4.0;
        for tool in LiquifyTool::ALL {
            let tip = match tool {
                // The lasso works on the same freeze mask as Freeze/Thaw.
                LiquifyTool::LassoMask => tl!("Freeze Lasso: drag to freeze an area, Alt-drag to thaw it (L)").to_string(),
                _ => format!("{} ({})", tl!(tool.label()), shortcut(tool)),
            };
            if crate::icons::button(&mut strip, tool_icon(tool), 34.0, d.preview_tool == PreviewTool::Brush && !d.face.enabled && d.opts.tool == tool, &tip)
                .clicked()
            {
                d.end();
                d.rig.enabled = false;
                d.opts.tool = tool;
                d.preview_tool = PreviewTool::Brush;
                d.face.enabled = false;
            }
            if matches!(tool, LiquifyTool::Smooth | LiquifyTool::PushLeft | LiquifyTool::Thaw) {
                strip.add_space(6.0);
            }
        }
        strip.add_space(10.0);
        let face_editing = d.face.enabled && d.preview_tool == PreviewTool::Brush;
        if crate::icons::button(&mut strip, "scan", 34.0, face_editing, "Face adjustments: turn on automatic detection and sliders").clicked() {
            let enabled = !face_editing;
            let result = face_ui::set_enabled(d, enabled);
            d.face.feedback(result);
        }
        for (tool, icon, tip) in [
            (PreviewTool::Hand, "hand", "Move view (H). You can also hold Space or drag with the middle mouse button."),
            (PreviewTool::Zoom, "zoom-in", "Zoom (Z). Click to zoom in; Alt-click to zoom out."),
        ] {
            if crate::icons::button(&mut strip, icon, 34.0, d.preview_tool == tool, tip).clicked() {
                d.end();
                d.preview_tool = tool;
            }
        }
        // Right properties panel.
        let right = ERect::from_min_size(pos2(body.right() - RIGHT_W, body.top()), vec2(RIGHT_W, body.height()));
        painter.rect_filled(right, 0.0, t.dock);
        painter.line_segment([right.left_top(), right.left_bottom()], Stroke::new(1.0, t.separator));
        let property_rect = ERect::from_min_max(right.min + vec2(14.0, 12.0), pos2(right.right() - 14.0, (right.bottom() - 68.0).max(right.top() + 12.0)));
        let mut props = ui.new_child(egui::UiBuilder::new().max_rect(property_rect));
        egui::ScrollArea::vertical().id_salt("liquify-props").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible).show(
            &mut props,
            |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.small(format!("{}: {}", tl!("Editing"), d.layer_name));
                if d.origin.isolated() {
                    ui.small(tl!("Editing existing Smart Filter: isolated effect preview."));
                    ui.small(tl!("Layer masks, filter blending, and later filters remain in the document."));
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(!d.strokes.is_empty() || d.cur.is_some(), egui::Button::new(tl!("Undo"))).clicked() {
                        d.undo();
                    }
                    if ui.add_enabled(!d.redo.is_empty(), egui::Button::new(tl!("Redo"))).clicked() {
                        d.redo();
                    }
                });
                face_ui::panel(d, ui);
                widgets::hairline(ui);
                egui::CollapsingHeader::new("Brush tools and advanced options").id_salt("liquify-advanced").show(ui, |ui| {
                    if widgets::secondary_button(ui, tl!("Precision Push preset"), 230.0).clicked() {
                        d.end();
                        d.rig.enabled = false;
                        d.opts.tool = LiquifyTool::PrecisionWarp;
                        d.preview_tool = PreviewTool::Brush;
                        d.face.enabled = false;
                        d.opts.density = 65.0;
                        d.opts.pressure = 35.0;
                    }
                    widgets::section_label(ui, tl!("Brush Tool Options"));
                    widgets::slider_row(ui, tl!("Size"), &mut d.opts.size, 1.0..=3000.0, "", None);
                    widgets::slider_row(ui, tl!("Density"), &mut d.opts.density, 0.0..=100.0, "", None);
                    widgets::slider_row(ui, tl!("Pressure"), &mut d.opts.pressure, 0.0..=100.0, "", None);
                    widgets::slider_row(ui, tl!("Rate"), &mut d.opts.rate, 0.0..=100.0, "", None);
                    widgets::hairline(ui);
                    widgets::section_label(ui, tl!("Brush Reconstruct Options"));
                    widgets::slider_row(ui, tl!("Amount"), &mut d.opts.reconstruct_amount, 0.0..=100.0, "%", None);
                    ui.horizontal(|ui| {
                        if widgets::secondary_button(ui, tl!("Reconstruct"), 110.0).clicked() {
                            let a = f64::from(d.opts.reconstruct_amount);
                            d.global(LiquifyTool::ReconstructAll, Some(a));
                        }
                        if widgets::secondary_button(ui, tl!("Restore All"), 110.0).clicked() {
                            d.restore_all();
                        }
                    });
                    widgets::hairline(ui);
                    widgets::section_label(ui, tl!("Mask Options"));
                    ui.horizontal(|ui| {
                        if widgets::secondary_button(ui, tl!("None"), 70.0).clicked() {
                            d.global(LiquifyTool::ThawAll, None);
                        }
                        if widgets::secondary_button(ui, tl!("Mask All"), 80.0).clicked() {
                            d.global(LiquifyTool::FreezeAll, None);
                        }
                        if widgets::secondary_button(ui, tl!("Invert All"), 80.0).clicked() {
                            d.global(LiquifyTool::InvertFreeze, None);
                        }
                    });
                    widgets::hairline(ui);
                    widgets::section_label(ui, tl!("View Options"));
                    widgets::checkbox(ui, &mut d.opts.show_mesh, tl!("Show Mesh"));
                    ui.horizontal(|ui| {
                        ui.label(tl!("Mesh Size"));
                        widgets::dropdown(ui, "liquify-mesh-size", &mut d.opts.mesh_size, &[(0, tl!("Small")), (1, tl!("Medium")), (2, tl!("Large"))], 110.0);
                    });
                    widgets::checkbox(ui, &mut d.opts.show_mask, tl!("Show Mask"));
                    d.detail.panel(ui, &mut d.opts.view, d.proxy.scale, &mut d.opts.show_backdrop, &mut d.opts.backdrop_opacity);
                    egui::CollapsingHeader::new("Manual outline placement").show(ui, |ui| {
                        ui.add_enabled_ui(!d.opts.view.before, |ui| manual_rig::panel(d, ui));
                    });
                    widgets::hairline(ui);
                    ui.label(
                        egui::RichText::new(format!("{} stroke(s) · field {} px/node · dab {:.2} ms", d.strokes.len(), d.cell, d.dab_ms))
                            .color(t.text_faint)
                            .size(11.0),
                    );
                });
            },
        );
        // Footer buttons.
        let foot = ERect::from_min_size(pos2(right.left() + 14.0, right.bottom() - 48.0), vec2(RIGHT_W - 28.0, 32.0));
        let mut fb = ui.new_child(egui::UiBuilder::new().max_rect(foot).layout(egui::Layout::right_to_left(egui::Align::Center)));
        if let Some(role) = widgets::dialog_buttons(
            &mut fb,
            &[
                widgets::DialogButton::new(widgets::ButtonRole::Default, tl!("Apply"), 84.0),
                widgets::DialogButton::new(widgets::ButtonRole::Cancel, tl!("Cancel"), 84.0),
            ],
        ) {
            action = Some(if role == widgets::ButtonRole::Default { "ok" } else { "cancel" });
        }
        // Preview.
        let area = ERect::from_min_max(pos2(left.right(), body.top()), pos2(right.left(), body.bottom() - 34.0));
        painter.rect_filled(area, 0.0, t.canvas);
        let (cw, ch) = (d.canvas.width() as f32, d.canvas.height() as f32);
        if d.zoom_fit || d.zoom <= 0.0 {
            d.zoom = ((area.width() - 40.0) / cw).min((area.height() - 40.0) / ch).clamp(0.01 / d.pixels_per_point, 32.0 / d.pixels_per_point);
            d.center = [f64::from(d.canvas.x0) + f64::from(cw) / 2.0, f64::from(d.canvas.y0) + f64::from(ch) / 2.0];
        }
        let resp = ui.interact(area, egui::Id::new("liquify-preview"), Sense::click_and_drag());
        // Handle wheel, Windows Ctrl+wheel, and pinch, preserving the point under the pointer.
        if resp.hovered() && d.cur.is_none() && !d.rig.pointer_captured() && !face_ui::handles::captured(d) {
            let factor = preview_zoom_factor(ctx);
            if factor != 1.0
                && let Some(hp) = resp.hover_pos()
            {
                zoom_at(d, area, hp, d.zoom * factor);
            }
        }
        if d.preview_tool == PreviewTool::Zoom && resp.clicked() {
            let factor = if ui.input(|i| i.modifiers.alt) { 0.5 } else { 2.0 };
            if let Some(hp) = resp.interact_pointer_pos() {
                zoom_at(d, area, hp, d.zoom * factor);
            }
        }
        let viewbar = ERect::from_min_max(pos2(left.right(), area.bottom()), pos2(right.left(), body.bottom()));
        painter.rect_filled(viewbar, 0.0, t.dock);
        let mut view_ui =
            ui.new_child(egui::UiBuilder::new().max_rect(viewbar.shrink2(vec2(8.0, 3.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        if view_ui.button("-").on_hover_text("Zoom out").clicked() {
            zoom_at(d, area, area.center(), d.zoom / 1.25);
        }
        let mut percent = d.zoom * d.pixels_per_point * 100.0;
        if view_ui.add(egui::DragValue::new(&mut percent).range(1.0..=3200.0).suffix("%").speed(1.0)).changed() {
            zoom_at(d, area, area.center(), percent / (100.0 * d.pixels_per_point));
        }
        if view_ui.button("+").on_hover_text("Zoom in").clicked() {
            zoom_at(d, area, area.center(), d.zoom * 1.25);
        }
        if view_ui.button("Fit").clicked() {
            d.zoom_fit = true;
        }
        if view_ui.button("100%").on_hover_text("Actual image pixels").clicked() {
            zoom_at(d, area, area.center(), 1.0 / d.pixels_per_point);
        }
        view_ui.label("Wheel: zoom | Space-drag: pan");
        face_ui::handles::interact(d, ui, area, &resp);
        let img = ERect::from_min_max(
            to_screen(d, area, [f64::from(d.canvas.x0), f64::from(d.canvas.y0)]),
            to_screen(d, area, [f64::from(d.canvas.x1), f64::from(d.canvas.y1)]),
        );
        let clip = painter.with_clip_rect(area);
        widgets::checker(&clip, img.intersect(area), 8.0);
        let uv = ERect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        // References and preview opacity never modify the document or target layer.
        let reference = if d.opts.show_backdrop {
            if d.opts.view.reference == precision::Reference::OtherVisible { d.backdrop.clone() } else { d.detail.reference(ctx, d.opts.view.reference) }
        } else {
            None
        };
        let reference_tint = Color32::from_white_alpha((d.opts.backdrop_opacity.clamp(0.0, 100.0) * 2.55).round() as u8);
        if !d.opts.view.on_top
            && let Some(texture) = &reference
        {
            clip.image(texture.id(), img, uv, reference_tint);
        }
        let detailed = d.detail.paint(
            ui,
            img,
            area,
            &d.field,
            &d.opts.view,
            d.proxy.scale,
            d.cur.is_some() || d.lasso.is_some() || d.rig.dragging() || face_ui::handles::captured(d),
        );
        if !detailed {
            let tint = Color32::from_white_alpha((d.opts.view.working_opacity.clamp(0.0, 100.0) * 2.55).round() as u8);
            if d.opts.view.before {
                if let Some(texture) = d.detail.reference(ctx, precision::Reference::Before) {
                    clip.image(texture.id(), img, uv, tint);
                }
            } else if let Some(texture) = &d.tex {
                // A reduced proxy covers whole proxy pixels, sometimes beyond the last document pixel.
                let proxy_rect = ERect::from_min_max(
                    img.min,
                    img.min + vec2(d.proxy.w as f32 * d.proxy.scale as f32 * d.zoom, d.proxy.h as f32 * d.proxy.scale as f32 * d.zoom),
                );
                clip.image(texture.id(), proxy_rect, uv, tint);
            }
        }
        if d.opts.view.on_top
            && let Some(texture) = &reference
        {
            clip.image(texture.id(), img, uv, reference_tint);
        }
        if d.opts.show_mask
            && let Some(m) = &d.mask_tex
        {
            let f = &d.field;
            let mr = ERect::from_min_size(
                img.min - vec2(0.5, 0.5) * d.zoom * f.cell as f32 + vec2(0.5, 0.5) * d.zoom,
                vec2(f.w as f32, f.h as f32) * f.cell as f32 * d.zoom,
            );
            clip.image(m.id(), mr, uv, Color32::WHITE);
        }
        if d.opts.show_mesh {
            draw_mesh(d, &clip, area);
        }
        manual_rig::draw(d, &clip, area);
        face_ui::draw(d, &clip, area);
        face_ui::handles::draw(d, &clip, area);
        // The lasso polygon being drawn (danger colour freezes, accent thaws; the mask overlay paints the
        // applied area red like the Freeze brush).
        if let Some((subtract, pts)) = &d.lasso
            && pts.len() > 1
        {
            let color = if *subtract { t.accent } else { t.danger };
            let line: Vec<Pos2> = pts.iter().map(|p| to_screen(d, area, *p)).collect();
            clip.add(egui::Shape::line(line, Stroke::new(1.0, color)));
        }
        // Brush outline (the lasso has none).
        if let Some(hp) = resp.hover_pos() {
            if d.preview_tool == PreviewTool::Brush && !d.face.enabled && d.opts.tool != LiquifyTool::LassoMask {
                let r = d.opts.size * 0.5 * d.zoom;
                clip.circle_stroke(hp, r, Stroke::new(1.5, Color32::BLACK));
                clip.circle_stroke(hp, r, Stroke::new(0.75, Color32::WHITE));
            }
            d.hover = Some(to_doc(d, area, hp));
        }
        clip.rect_stroke(img, 0.0, Stroke::new(1.0, t.separator), egui::StrokeKind::Outside);
        // Mouse → strokes (Space or middle button pans).
        let pan = d.preview_tool == PreviewTool::Hand || ui.input(|i| i.key_down(egui::Key::Space) || i.pointer.middle_down());
        if pan {
            resp.clone().on_hover_cursor(egui::CursorIcon::Grab);
        } else if d.preview_tool == PreviewTool::Zoom {
            resp.clone().on_hover_cursor(egui::CursorIcon::ZoomIn);
        }
        if pan && resp.dragged() {
            let dl = resp.drag_delta();
            d.center = [d.center[0] - f64::from(dl.x / d.zoom), d.center[1] - f64::from(dl.y / d.zoom)];
        } else if !pan
            && !d.face.enabled
            && d.preview_tool == PreviewTool::Brush
            && !d.opts.view.before
            && let Some(pp) = resp.interact_pointer_pos()
        {
            let q = to_doc(d, area, pp);
            let pr = 1.0;
            mods = ui.input(|i| i.modifiers);
            if d.lasso.is_some() {
                // A lasso in progress: collect the polygon, no brush dabs.
                if resp.dragged() {
                    events.push(ToolEvent::Move { x: q[0], y: q[1], pressure: pr });
                    ctx.request_repaint();
                }
            } else if resp.drag_started() || (resp.is_pointer_button_down_on() && d.cur.is_none() && !d.rig.pointer_captured()) {
                events.push(ToolEvent::Down { x: q[0], y: q[1], pressure: pr });
            } else if resp.dragged() || resp.is_pointer_button_down_on() {
                let moved = d.rig.moved(q) || d.cur.as_ref().is_some_and(|(_, l)| l[0] != q[0] || l[1] != q[1]);
                // Stationary tools keep working while the brush is held still (30 dabs/s).
                let held = d.opts.tool.is_stationary() && crate::gpu_canvas::now_ms() - d.last_dab > 33.0;
                if moved || held {
                    events.push(ToolEvent::Move { x: q[0], y: q[1], pressure: pr });
                }
                ctx.request_repaint();
            }
        }
        let primary_down = ui.input(|i| i.pointer.primary_down());
        let lasso_up = d.lasso.is_some() && !primary_down;
        if resp.drag_stopped() || lasso_up || ((d.cur.is_some() || d.rig.pointer_captured()) && !primary_down) {
            let q = d.lasso.as_ref().and_then(|(_, pts)| pts.last().copied()).or_else(|| d.cur.as_ref().map(|(_, l)| [l[0], l[1]])).unwrap_or_default();
            let q = if d.rig.pointer_captured() { d.hover.unwrap_or(q) } else { q };
            events.push(ToolEvent::Up { x: q[0], y: q[1] });
        }
    });
    for ev in events {
        pointer(app, ev, mods);
    }
    match action {
        Some("ok") => commit(app),
        Some("cancel") => cancel(app),
        _ => {}
    }
}

fn to_screen(d: &LiquifyDialog, area: ERect, p: [f64; 2]) -> Pos2 {
    area.center() + vec2(((p[0] - d.center[0]) as f32) * d.zoom, ((p[1] - d.center[1]) as f32) * d.zoom)
}

fn to_doc(d: &LiquifyDialog, area: ERect, p: Pos2) -> [f64; 2] {
    let v = (p - area.center()) / d.zoom;
    [d.center[0] + f64::from(v.x), d.center[1] + f64::from(v.y)]
}

/// The deformed mesh: where the source grid lines land. Each output sample knows its source
/// position `p + d(p)`, so a source line `x = c` is the iso-line of that function, traced with
/// marching squares on a sample grid four times finer than the mesh (exact for any field, unlike
/// inverting the field point by point, which fails where it folds).
fn draw_mesh(d: &LiquifyDialog, painter: &egui::Painter, area: ERect) {
    let c = d.canvas;
    let cells = [64.0, 40.0, 24.0][d.opts.mesh_size.min(2)];
    let step = f64::from(c.width().max(c.height())) / cells;
    let h = step / 4.0;
    let nx = (f64::from(c.width()) / h).ceil() as usize + 1;
    let ny = (f64::from(c.height()) / h).ceil() as usize + 1;
    let (x0, y0) = (f64::from(c.x0), f64::from(c.y0));
    let mut src = Vec::with_capacity(nx * ny);
    for j in 0..ny {
        for i in 0..nx {
            let p = [x0 + i as f64 * h, y0 + j as f64 * h];
            let v = d.field.sample(p[0], p[1]);
            src.push([p[0] + v[0], p[1] + v[1]]);
        }
    }
    let stroke = Stroke::new(0.75, Color32::from_rgba_unmultiplied(90, 90, 90, 210));
    let pos = |i: usize, j: usize| [x0 + i as f64 * h, y0 + j as f64 * h];
    for axis in [0usize, 1] {
        let origin = if axis == 0 { x0 } else { y0 };
        for jj in 0..ny - 1 {
            for ii in 0..nx - 1 {
                let corners = [(ii, jj), (ii + 1, jj), (ii + 1, jj + 1), (ii, jj + 1)];
                let vals = corners.map(|(i, j)| src[j * nx + i][axis]);
                let (lo, hi) = (vals.iter().copied().fold(f64::MAX, f64::min), vals.iter().copied().fold(f64::MIN, f64::max));
                let mut k = ((lo - origin) / step).ceil() as i64;
                while origin + k as f64 * step <= hi {
                    let level = origin + k as f64 * step;
                    let mut pts: Vec<[f64; 2]> = Vec::with_capacity(4);
                    for e in 0..4 {
                        let (a, b) = (vals[e] - level, vals[(e + 1) % 4] - level);
                        if (a <= 0.0) != (b <= 0.0) {
                            let t = a / (a - b);
                            let (pa, pb) = (pos(corners[e].0, corners[e].1), pos(corners[(e + 1) % 4].0, corners[(e + 1) % 4].1));
                            pts.push([pa[0] + (pb[0] - pa[0]) * t, pa[1] + (pb[1] - pa[1]) * t]);
                        }
                    }
                    for seg in pts.as_chunks::<2>().0 {
                        painter.line_segment([to_screen(d, area, seg[0]), to_screen(d, area, seg[1])], stroke);
                    }
                    k += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_layer() -> PhotocraftApp {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
        app.session.execute("file.new", json!({"width": 120, "height": 80, "depth": 8})).unwrap();
        app.session.execute("layer.new.layer", json!({})).unwrap();
        app.session
            .edit("paint", |doc, active| {
                let s = doc.layer_mut(active.unwrap()).unwrap().surface_mut().unwrap();
                for y in 10..70 {
                    for x in 10..110 {
                        let v = ((x / 5 + y / 5) % 2) as f32;
                        s.write_pixel(x, y, &[v, 0.3, 1.0 - v, 1.0]);
                    }
                }
                Ok(())
            })
            .unwrap();
        app.sync_views();
        app
    }

    /// #418: the brush settings survive Cancel, OK and Esc, and come back the next time Liquify
    /// opens (also after a restart: they live in the preferences).
    #[test]
    fn brush_settings_are_remembered_between_uses() {
        let ctx = egui::Context::default();
        let mut app = app_with_layer();
        open(&mut app, &ctx).unwrap();
        let fresh = app.distort.liquify.as_ref().unwrap().opts.clone();
        assert_eq!((fresh.density, fresh.tool), (50.0, LiquifyTool::ForwardWarp), "first use: the defaults");
        let set = json!({"tool": "twirlCw", "size": 37, "density": 85, "pressure": 60, "rate": 30, "showMesh": true, "meshSize": "large", "showBackdrop": true, "backdropOpacity": 25});
        control(&mut app, &set).unwrap();
        control(&mut app, &json!({"cancel": true})).unwrap();
        assert!(app.distort.liquify.is_none());
        open(&mut app, &ctx).unwrap();
        assert_eq!(app.distort.liquify.as_ref().unwrap().opts.to_json(), {
            let mut o = LiquifyOpts::default();
            o.apply(&set).unwrap();
            o.to_json()
        });
        // OK and Esc keep the latest settings too.
        control(&mut app, &json!({"density": 90})).unwrap();
        commit(&mut app);
        open(&mut app, &ctx).unwrap();
        assert_eq!(app.distort.liquify.as_ref().unwrap().opts.density, 90.0);
        control(&mut app, &json!({"density": 70})).unwrap();
        cancel(&mut app);
        assert_eq!(app.session.prefs().dialogs[REMEMBERED]["density"], json!(70.0));
        // A damaged preference keeps what it can and never fails to open.
        app.session.prefs.edit(|p| p.dialogs.insert(REMEMBERED.into(), json!({"tool": "nope", "density": "lots", "size": 1e12, "rate": 20})));
        open(&mut app, &ctx).unwrap();
        let o = &app.distort.liquify.as_ref().unwrap().opts;
        assert_eq!((o.tool, o.density, o.size, o.rate), (LiquifyTool::ForwardWarp, 50.0, 15000.0, 20.0));
        app.session.prefs.edit(|p| p.dialogs.insert(REMEMBERED.into(), json!("not an object")));
        cancel(&mut app);
        assert!(open(&mut app, &ctx).is_ok());
    }

    /// Follow-up to #1494 / #1484: Alt is captured at stroke-down and stays recorded
    /// through undo, redo and deterministic Liquify field reconstruction.
    #[test]
    fn alt_reversed_twirl_survives_replay_undo_and_redo() {
        let ctx = egui::Context::default();
        let mut app = app_with_layer();
        open(&mut app, &ctx).unwrap();
        control(&mut app, &json!({"tool": "twirlCw", "size": 36, "rate": 80})).unwrap();

        // Releasing Alt while dragging must not change the tool recorded at pointer-down.
        pointer(&mut app, ToolEvent::Down { x: 53.0, y: 40.0, pressure: 1.0 }, egui::Modifiers::ALT);
        pointer(&mut app, ToolEvent::Move { x: 58.0, y: 41.0, pressure: 1.0 }, egui::Modifiers::NONE);
        pointer(&mut app, ToolEvent::Up { x: 58.0, y: 41.0 }, egui::Modifiers::NONE);
        let d = app.distort.liquify.as_ref().unwrap();
        assert_eq!(d.opts.tool, LiquifyTool::TwirlCw, "Alt must not change the selected tool");
        assert_eq!(d.strokes.len(), 1);
        assert_eq!(d.strokes[0].tool, LiquifyTool::TwirlCcw);
        assert_eq!(LiquifyField::from_strokes(d.canvas, d.cell, &d.strokes), d.field);

        // A following stroke without Alt records the original tool.
        pointer(&mut app, ToolEvent::Down { x: 70.0, y: 40.0, pressure: 1.0 }, egui::Modifiers::NONE);
        pointer(&mut app, ToolEvent::Move { x: 75.0, y: 42.0, pressure: 1.0 }, egui::Modifiers::ALT);
        pointer(&mut app, ToolEvent::Up { x: 75.0, y: 42.0 }, egui::Modifiers::ALT);
        let d = app.distort.liquify.as_ref().unwrap();
        assert_eq!(d.strokes.len(), 2);
        assert_eq!(d.strokes[1].tool, LiquifyTool::TwirlCw);
        assert_eq!(LiquifyField::from_strokes(d.canvas, d.cell, &d.strokes), d.field);

        control(&mut app, &json!({"undo": true})).unwrap();
        let d = app.distort.liquify.as_ref().unwrap();
        assert_eq!(d.strokes.len(), 1);
        assert_eq!(d.strokes[0].tool, LiquifyTool::TwirlCcw);
        assert_eq!(d.redo[0].tool, LiquifyTool::TwirlCw);
        assert_eq!(LiquifyField::from_strokes(d.canvas, d.cell, &d.strokes), d.field);

        control(&mut app, &json!({"redo": true})).unwrap();
        let d = app.distort.liquify.as_ref().unwrap();
        assert_eq!(d.strokes.len(), 2);
        assert!(d.redo.is_empty());
        assert_eq!(d.strokes[0].tool, LiquifyTool::TwirlCcw);
        assert_eq!(d.strokes[1].tool, LiquifyTool::TwirlCw);
        assert_eq!(LiquifyField::from_strokes(d.canvas, d.cell, &d.strokes), d.field);
    }

    #[test]
    fn dialog_strokes_match_the_engine_and_commit_once() {
        let ctx = egui::Context::default();
        let mut app = app_with_layer();
        open(&mut app, &ctx).unwrap();
        control(&mut app, &json!({"tool": "forwardWarp", "size": 40})).unwrap();
        for ev in [
            ToolEvent::Down { x: 40.0, y: 40.0, pressure: 1.0 },
            ToolEvent::Move { x: 48.0, y: 41.0, pressure: 1.0 },
            ToolEvent::Move { x: 56.0, y: 40.0, pressure: 1.0 },
            ToolEvent::Up { x: 56.0, y: 40.0 },
        ] {
            pointer(&mut app, ev, egui::Modifiers::NONE);
        }
        control(&mut app, &json!({"tool": "bloat"})).unwrap();
        for ev in [ToolEvent::Down { x: 80.0, y: 40.0, pressure: 1.0 }, ToolEvent::Move { x: 80.0, y: 40.0, pressure: 1.0 }, ToolEvent::Up { x: 80.0, y: 40.0 }]
        {
            pointer(&mut app, ev, egui::Modifiers::NONE);
        }
        let d = app.distort.liquify.as_ref().unwrap();
        assert_eq!(d.strokes.len(), 2);
        assert_eq!(d.redo.len(), 0);
        // Replaying the recorded strokes gives exactly the interactive field.
        let replay = LiquifyField::from_strokes(d.canvas, d.cell, &d.strokes);
        assert_eq!(replay, d.field);
        assert!(d.field.max_displacement() > 3.0);
        // Undo inside the dialog drops the last stroke.
        control(&mut app, &json!({"undo": true})).unwrap();
        assert_eq!(app.distort.liquify.as_ref().unwrap().strokes.len(), 1);
        assert_eq!(app.distort.liquify.as_ref().unwrap().redo.len(), 1);
        control(&mut app, &json!({"redo": true})).unwrap();
        assert_eq!(app.distort.liquify.as_ref().unwrap().strokes.len(), 2);
        assert_eq!(app.distort.liquify.as_ref().unwrap().redo.len(), 0);
        control(&mut app, &json!({"undo": true})).unwrap();
        let before = app.session.active().unwrap().history.past_len();
        control(&mut app, &json!({"commit": true})).unwrap();
        assert!(app.distort.liquify.is_none());
        assert_eq!(app.session.active().unwrap().history.past_len(), before + 1);
    }

    #[test]
    fn undo_rerenders_the_proxy_before_the_next_texture_upload() {
        let ctx = egui::Context::default();
        let mut app = app_with_layer();
        open(&mut app, &ctx).unwrap();
        control(&mut app, &json!({"tool": "forwardWarp", "size": 40})).unwrap();
        for ev in [
            ToolEvent::Down { x: 35.0, y: 40.0, pressure: 1.0 },
            ToolEvent::Move { x: 55.0, y: 40.0, pressure: 1.0 },
            ToolEvent::Up { x: 55.0, y: 40.0 },
            ToolEvent::Down { x: 70.0, y: 40.0, pressure: 1.0 },
            ToolEvent::Move { x: 90.0, y: 40.0, pressure: 1.0 },
            ToolEvent::Up { x: 90.0, y: 40.0 },
        ] {
            pointer(&mut app, ev, egui::Modifiers::NONE);
        }

        let before = app.distort.liquify.as_ref().unwrap().out.clone();
        control(&mut app, &json!({"undo": true})).unwrap();
        let d = app.distort.liquify.as_ref().unwrap();
        assert_eq!(d.strokes.len(), 1);
        assert_ne!(d.out, before, "undo must update the pixels shown by the existing texture");

        let mut expected = vec![[0u8; 4]; d.proxy.w * d.proxy.h];
        d.proxy.render(&d.field, [0, 0, d.proxy.w, d.proxy.h], &mut expected);
        assert_eq!(d.out, expected, "dirty upload source must match the rebuilt field");
    }

    #[test]
    fn liquify_redo_restores_undone_stroke_and_a_new_stroke_clears_redo() {
        let ctx = egui::Context::default();
        let mut app = app_with_layer();
        open(&mut app, &ctx).unwrap();
        control(&mut app, &json!({"tool": "forwardWarp", "size": 40})).unwrap();
        let draw = |app: &mut PhotocraftApp, x: f64| {
            for ev in
                [ToolEvent::Down { x, y: 40.0, pressure: 1.0 }, ToolEvent::Move { x: x + 14.0, y: 40.0, pressure: 1.0 }, ToolEvent::Up { x: x + 14.0, y: 40.0 }]
            {
                pointer(app, ev, egui::Modifiers::NONE);
            }
        };

        draw(&mut app, 30.0);
        draw(&mut app, 70.0);
        control(&mut app, &json!({"undo": true})).unwrap();
        let d = app.distort.liquify.as_ref().unwrap();
        assert_eq!((d.strokes.len(), d.redo.len()), (1, 1));

        control(&mut app, &json!({"redo": true})).unwrap();
        let d = app.distort.liquify.as_ref().unwrap();
        assert_eq!((d.strokes.len(), d.redo.len()), (2, 0));
        assert_eq!(LiquifyField::from_strokes(d.canvas, d.cell, &d.strokes), d.field);

        control(&mut app, &json!({"undo": true})).unwrap();
        draw(&mut app, 50.0);
        let d = app.distort.liquify.as_ref().unwrap();
        assert_eq!((d.strokes.len(), d.redo.len()), (2, 0));
        control(&mut app, &json!({"redo": true})).unwrap();
        assert_eq!(app.distort.liquify.as_ref().unwrap().strokes.len(), 2, "redo after a new stroke is a no-op");
    }

    /// The Freeze Lasso freezes the dragged polygon (⌥ thaws it) as one recorded stroke, and a
    /// new lasso clears Redo like any other stroke.
    #[test]
    fn freeze_lasso_records_a_stroke_and_clears_redo() {
        let ctx = egui::Context::default();
        let mut app = app_with_layer();
        open(&mut app, &ctx).unwrap();
        control(&mut app, &json!({"tool": "lassoMask"})).unwrap();
        let lasso = |app: &mut PhotocraftApp, m: egui::Modifiers, r: [f64; 4]| {
            pointer(app, ToolEvent::Down { x: r[0], y: r[1], pressure: 1.0 }, m);
            for (x, y) in [(r[2], r[1]), (r[2], r[3]), (r[0], r[3])] {
                pointer(app, ToolEvent::Move { x, y, pressure: 1.0 }, m);
            }
            pointer(app, ToolEvent::Up { x: r[0], y: r[3] }, m);
        };
        lasso(&mut app, egui::Modifiers::NONE, [20.0, 20.0, 80.0, 60.0]);
        let d = app.distort.liquify.as_ref().unwrap();
        assert_eq!(d.strokes.len(), 1);
        assert_eq!(d.field.freeze_at(50.0, 40.0), 1.0, "frozen inside");
        assert_eq!(d.field.freeze_at(5.0, 5.0), 0.0, "not outside");
        lasso(&mut app, egui::Modifiers::ALT, [40.0, 30.0, 60.0, 50.0]);
        let d = app.distort.liquify.as_ref().unwrap();
        assert_eq!(d.field.freeze_at(50.0, 40.0), 0.0, "⌥ thaws");
        assert_eq!(d.field.freeze_at(25.0, 25.0), 1.0);
        control(&mut app, &json!({"undo": true})).unwrap();
        assert_eq!(app.distort.liquify.as_ref().unwrap().field.freeze_at(50.0, 40.0), 1.0, "undo replays the first lasso");
        assert_eq!(app.distort.liquify.as_ref().unwrap().redo.len(), 1);
        lasso(&mut app, egui::Modifiers::NONE, [0.0, 0.0, 10.0, 10.0]);
        assert_eq!(app.distort.liquify.as_ref().unwrap().redo.len(), 0, "a new lasso clears redo");
    }

    #[test]
    fn alt_reverses_twirl_and_swaps_pucker_and_bloat() {
        let ctx = egui::Context::default();
        let mut app = app_with_layer();
        open(&mut app, &ctx).unwrap();
        let dab = |app: &mut PhotocraftApp, m: egui::Modifiers| {
            pointer(app, ToolEvent::Down { x: 50.0, y: 40.0, pressure: 1.0 }, m);
            pointer(app, ToolEvent::Up { x: 50.0, y: 40.0 }, m);
            app.distort.liquify.as_ref().unwrap().strokes.last().unwrap().tool
        };
        for (tool, plain, alt) in [
            ("twirlCw", LiquifyTool::TwirlCw, LiquifyTool::TwirlCcw),
            ("twirlCcw", LiquifyTool::TwirlCcw, LiquifyTool::TwirlCw),
            ("pucker", LiquifyTool::Pucker, LiquifyTool::Bloat),
            ("bloat", LiquifyTool::Bloat, LiquifyTool::Pucker),
            ("forwardWarp", LiquifyTool::ForwardWarp, LiquifyTool::ForwardWarp),
        ] {
            control(&mut app, &json!({"tool": tool})).unwrap();
            assert_eq!(dab(&mut app, egui::Modifiers::NONE), plain, "{tool}");
            assert_eq!(dab(&mut app, egui::Modifiers::ALT), alt, "⌥ {tool}");
            assert_eq!(app.distort.liquify.as_ref().unwrap().opts.tool, plain, "⌥ leaves the selected tool alone");
        }
    }

    #[test]
    fn liquify_redo_stack_is_bounded() {
        let ctx = egui::Context::default();
        let mut app = app_with_layer();
        open(&mut app, &ctx).unwrap();
        let d = app.distort.liquify.as_mut().unwrap();
        d.strokes = (0..=REDO_STACK_LIMIT).map(|_| LiquifyStroke::new(LiquifyTool::ForwardWarp, 1.0)).collect();
        for _ in 0..=REDO_STACK_LIMIT {
            d.undo();
        }
        assert_eq!(d.redo.len(), REDO_STACK_LIMIT);
    }

    #[test]
    fn shortcut_handler_undoes_liquify_even_when_egui_owns_keyboard_focus() {
        let ctx = egui::Context::default();
        let mut app = app_with_layer();
        open(&mut app, &ctx).unwrap();
        for ev in [ToolEvent::Down { x: 40.0, y: 40.0, pressure: 1.0 }, ToolEvent::Move { x: 52.0, y: 40.0, pressure: 1.0 }, ToolEvent::Up { x: 52.0, y: 40.0 }]
        {
            pointer(&mut app, ev, egui::Modifiers::NONE);
        }
        assert_eq!(app.distort.liquify.as_ref().unwrap().strokes.len(), 1);

        let raw = egui::RawInput {
            events: vec![egui::Event::Key { key: egui::Key::Z, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND }],
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ui| {
            let ctx = ui.ctx();
            ctx.memory_mut(|m| m.request_focus(egui::Id::new("liquify-slider-focus")));
            assert!(ctx.egui_wants_keyboard_input());
            crate::shortcuts::handle(&mut app, ctx);
        });
        out.textures_delta.clear();
        assert_eq!(app.distort.liquify.as_ref().unwrap().strokes.len(), 0);
        let raw = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Z,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            }],
            ..Default::default()
        };
        let _ = ctx.run_ui(raw, |ui| {
            crate::shortcuts::handle(&mut app, ui.ctx());
        });
        assert_eq!(app.distort.liquify.as_ref().unwrap().strokes.len(), 1, "Cmd+Shift+Z redoes the last stroke");
    }

    #[test]
    fn menu_opens_the_dialog_and_routes_pointer_events() {
        let ctx = egui::Context::default();
        let mut app = app_with_layer();
        let r = crate::distort_ui::menu(&mut app, &ctx, "filter.liquify", &json!({})).unwrap().unwrap();
        assert!(r["liquify"]["proxy"].is_array());
        assert!(crate::distort_ui::pointer(&mut app, ToolEvent::Down { x: 50.0, y: 40.0, pressure: 1.0 }, egui::Modifiers::NONE));
        crate::distort_ui::pointer(&mut app, ToolEvent::Up { x: 60.0, y: 40.0 }, egui::Modifiers::NONE);
        assert_eq!(app.distort.liquify.as_ref().unwrap().strokes.len(), 1);
        control(&mut app, &json!({"cancel": true})).unwrap();
        assert!(app.distort.liquify.is_none());
        assert_eq!(app.session.active().unwrap().history.past_len(), 2, "cancel records nothing");
    }
}
