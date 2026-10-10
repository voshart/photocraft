// SPDX-License-Identifier: MIT OR Apache-2.0
//! Source-backed Liquify detail preview and view-only reference controls.
//! Kept inside liquify_ui: it cannot change the target document or recorded strokes.
//! Native-only asynchronous refinement in this first pass; WASM retains its working proxy.

use egui::{Color32, ColorImage, Context, Rect as ScreenRect, TextureHandle, TextureOptions, Ui, pos2, vec2};
use photocraft_algo::liquify::LiquifyField;
#[cfg(not(target_arch = "wasm32"))]
use photocraft_algo::liquify::apply_liquify_region;
use photocraft_doc::{Document, LayerContent, LayerId};
use photocraft_geom::Rect;
use photocraft_raster::Surface;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, TryRecvError},
};

const MAX_DETAIL_PIXELS: u64 = 4_000_000;
const MAX_FIELD_NODES: usize = 4_000_000;
const MAX_REFERENCES: usize = 256;
const SETTLE_MS: f64 = 120.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Quality {
    Fast,
    #[default]
    Balanced,
    Precise,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Reference {
    #[default]
    Before,
    OtherVisible,
    Layer(u64),
}

#[derive(Clone, Debug)]
pub(crate) struct ViewOptions {
    pub quality: Quality,
    pub working_opacity: f32,
    pub reference: Reference,
    pub on_top: bool,
    pub before: bool,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self { quality: Quality::Balanced, working_opacity: 100.0, reference: Reference::Before, on_top: false, before: false }
    }
}

impl ViewOptions {
    pub fn apply(&mut self, v: &Value) -> Result<(), String> {
        // Validate before applying: malformed view JSON cannot partially change the view.
        let quality = v.get("previewQuality").map(|v| serde_json::from_value::<Quality>(v.clone())).transpose().map_err(|e| e.to_string())?;
        let reference = v.get("referenceSource").map(|v| serde_json::from_value::<Reference>(v.clone())).transpose().map_err(|e| e.to_string())?;
        if let Some(q) = quality {
            self.quality = q;
        }
        if let Some(r) = reference {
            self.reference = r;
        }
        if let Some(n) = v.get("workingOpacity").and_then(Value::as_f64).filter(|n| n.is_finite()) {
            self.working_opacity = n.clamp(0.0, 100.0) as f32;
        }
        if let Some(b) = v.get("referenceOnTop").and_then(Value::as_bool) {
            self.on_top = b;
        }
        if let Some(b) = v.get("showBefore").and_then(Value::as_bool) {
            self.before = b;
        }
        Ok(())
    }

    /// Session-specific layer ids and the temporary Before view must not leak into
    /// preferences and accidentally reference another document on the next launch.
    pub fn remembered(&self) -> Value {
        json!({"previewQuality": self.quality, "workingOpacity": self.working_opacity, "referenceOnTop": self.on_top})
    }
}

struct ReferenceLayer {
    id: LayerId,
    name: String,
    surface: Surface,
}

/// Copy-on-write pixel sources, not full-resolution GPU textures. Groups, effects,
/// clipping stacks and vector masks are intentionally not advertised as isolated
/// layer composites in this first pass. The UI explicitly labels these as pixels.
fn references(doc: &Document, edited: LayerId) -> Vec<ReferenceLayer> {
    let mut result = Vec::new();
    let mut todo: Vec<_> = doc.layers.iter().collect();
    while let Some(layer) = todo.pop() {
        if result.len() >= MAX_REFERENCES {
            break;
        }
        if let Some(children) = layer.children() {
            todo.extend(children.iter());
        }
        if layer.id == edited {
            continue;
        }
        let surface = match &layer.content {
            LayerContent::Raster(s) => Some(s),
            LayerContent::Smart(s) => s.cache.as_ref(),
            _ => None,
        };
        if let Some(surface) = surface {
            result.push(ReferenceLayer { id: layer.id, name: layer.name.clone(), surface: surface.clone() });
        }
    }
    result
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    revision: u64,
    region: Rect,
    before: bool,
}

type DetailResult = Result<(Key, ColorImage), String>;

pub(super) struct DetailPreview {
    source: Surface,
    isolated: bool,
    selection: Option<Surface>,
    bounds: Rect,
    references: Vec<ReferenceLayer>,
    reference_texture: Option<(Reference, TextureHandle)>,
    revision: u64,
    changed_ms: f64,
    pending: Option<Receiver<DetailResult>>,
    pending_key: Option<Key>,
    cancel: Arc<AtomicBool>,
    ready: Option<(Key, TextureHandle)>,
    failed: Option<Key>,
    pub status: String,
}

impl Drop for DetailPreview {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl DetailPreview {
    pub fn new(source: Surface, bounds: Rect, doc: &Document, edited: LayerId) -> Self {
        Self {
            source,
            isolated: false,
            selection: doc.selection.clone(),
            bounds,
            references: references(doc, edited),
            reference_texture: None,
            revision: 0,
            changed_ms: 0.0,
            pending: None,
            pending_key: None,
            cancel: Arc::new(AtomicBool::new(false)),
            ready: None,
            failed: None,
            status: "Reduced preview".into(),
        }
    }

    /// Reopening an existing filter shows its isolated operation; it must not
    /// adopt a fresh document selection as a new filter constraint.
    pub fn set_isolated(&mut self) {
        self.isolated = true;
        self.selection = None;
        self.invalidate();
    }

    /// The reduced renderer writes premultiplied RGBA8. Mix its effect with the
    /// immutable proxy, never the previous output. Coverage is sampled in document
    /// coordinates; precise rendering still uses the full-resolution selection.
    pub fn mix_proxy(&self, proxy: &photocraft_algo::liquify::ProxyImage, out: &mut [[u8; 4]], rect: [usize; 4]) {
        let Some(mask) = &self.selection else { return };
        let [x0, y0, x1, y1] = [rect[0].min(proxy.w), rect[1].min(proxy.h), rect[2].min(proxy.w), rect[3].min(proxy.h)];
        for y in y0..y1 {
            for x in x0..x1 {
                let Some(i) = y.checked_mul(proxy.w).and_then(|row| row.checked_add(x)) else { continue };
                let (Some(original), Some(pixel)) = (proxy.px.get(i), out.get_mut(i)) else { continue };
                let px = (f64::from(proxy.bounds.x0) + (x as f64 + 0.5) * proxy.scale).floor() as i32;
                let py = (f64::from(proxy.bounds.y0) + (y as f64 + 0.5) * proxy.scale).floor() as i32;
                let coverage = mask.sample_channel(px, py, 0).clamp(0.0, 1.0);
                for (value, source) in pixel.iter_mut().zip(original) {
                    *value = ((*source + (f32::from(*value) / 255.0 - *source) * coverage).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                }
            }
        }
    }

    pub fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.changed_ms = crate::gpu_canvas::now_ms();
        self.ready = None;
        self.failed = None;
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn describe(&self) -> Value {
        json!({"status": self.status, "revision": self.revision, "pending": self.pending.is_some(), "nativeDetail": !cfg!(target_arch = "wasm32")})
    }

    pub fn panel(&self, ui: &mut Ui, view: &mut ViewOptions, proxy_scale: f64, show_reference: &mut bool, reference_opacity: &mut f32) {
        crate::widgets::section_label(ui, tl!("Precision preview"));
        if cfg!(target_arch = "wasm32") {
            ui.label(tl!("This web build uses the reduced preview."));
        } else if proxy_scale <= 1.0 {
            // A small source gets final-quality interpolation automatically, not
            // a meaningless upgrade selector that promises additional detail.
            ui.label(tl!("Full-detail refinement: automatic"));
        } else {
            let label = match view.quality {
                Quality::Fast => tl!("Fast"),
                Quality::Balanced => tl!("Balanced"),
                Quality::Precise => tl!("Precise"),
            };
            egui::ComboBox::from_id_salt("liquify-preview-quality").selected_text(label).show_ui(ui, |ui| {
                ui.selectable_value(&mut view.quality, Quality::Fast, tl!("Fast"));
                ui.selectable_value(&mut view.quality, Quality::Balanced, tl!("Balanced"));
                ui.selectable_value(&mut view.quality, Quality::Precise, tl!("Precise"));
            });
            ui.small(tl!("Balanced refines after a drag. Precise also refines while dragging."));
        }
        ui.small(&self.status);
        let before_label = if self.isolated { tl!("Filter input (view only)") } else { tl!("Before (view only)") };
        crate::widgets::checkbox(ui, &mut view.before, before_label);
        crate::widgets::hairline(ui);
        crate::widgets::section_label(ui, tl!("Reference view"));
        crate::widgets::checkbox(ui, show_reference, tl!("Show reference"));
        if *show_reference {
            let before_name = if self.isolated { "Input to existing Liquify filter" } else { "Before this Liquify session" };
            let name = match view.reference {
                Reference::Before => tl!(before_name).to_string(),
                Reference::OtherVisible => tl!("Other visible layers (composite)").to_string(),
                Reference::Layer(id) => {
                    self.references.iter().find(|r| r.id.0 == id).map(|r| r.name.clone()).unwrap_or_else(|| tl!("Unavailable layer").into())
                }
            };
            egui::ComboBox::from_id_salt("liquify-reference-source").selected_text(name).show_ui(ui, |ui| {
                ui.selectable_value(&mut view.reference, Reference::Before, tl!(before_name));
                ui.selectable_value(&mut view.reference, Reference::OtherVisible, tl!("Other visible layers (composite)"));
                for layer in &self.references {
                    ui.selectable_value(&mut view.reference, Reference::Layer(layer.id.0), format!("{} — {}", layer.name, tl!("pixels")));
                }
            });
            crate::widgets::checkbox(ui, &mut view.on_top, tl!("Reference in front"));
            crate::widgets::slider_row(ui, tl!("Reference opacity"), reference_opacity, 0.0..=100.0, "%", None);
        }
        crate::widgets::slider_row(ui, tl!("Working preview opacity"), &mut view.working_opacity, 0.0..=100.0, "%", None);
        ui.small(tl!("View only: Apply never changes layer opacity or reference layers."));
        ui.small(tl!("Selected references show pixel data, without layer effects or masks."));
    }

    pub fn reference(&mut self, ctx: &Context, selected: Reference) -> Option<TextureHandle> {
        if let Some((key, texture)) = &self.reference_texture
            && *key == selected
        {
            return Some(texture.clone());
        }
        let source = match selected {
            Reference::Before => &self.source,
            Reference::Layer(id) => &self.references.iter().find(|r| r.id.0 == id)?.surface,
            Reference::OtherVisible => return None, // existing compositor path in liquify_ui
        };
        let image = crate::distort_ui::surface_image(source, self.bounds, 1600);
        let texture = ctx.load_texture("liquify-reference-pixels", image, TextureOptions::LINEAR);
        self.reference_texture = Some((selected, texture.clone()));
        Some(texture)
    }

    fn poll(&mut self, ctx: &Context, wanted: Key) {
        let Some(rx) = &self.pending else { return };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("Detail worker stopped; reduced preview remains available".into()),
        };
        let requested = self.pending_key.take();
        self.pending = None;
        match result {
            Ok((key, image)) if key == wanted => {
                // Nearest magnification exposes actual pixels when zoomed past 100%.
                // This display setting is NOT the image/field warp interpolation.
                let options = TextureOptions { magnification: egui::TextureFilter::Nearest, ..TextureOptions::LINEAR };
                self.ready = Some((key, ctx.load_texture("liquify-source-detail", image, options)));
                self.failed = None;
            }
            Ok(_) => {} // late result from another field revision, crop, or before/after view
            Err(e) if requested == Some(wanted) => {
                self.status = e;
                self.failed = requested;
            }
            Err(_) => {}
        }
    }

    /// True only when the full visible image region was drawn. The caller must
    /// then skip the proxy, not alpha-composite the detail over that proxy twice.
    #[allow(clippy::too_many_arguments)] // Render inputs are view-only; avoid borrowing the parent dialog.
    pub fn paint(
        &mut self,
        ui: &Ui,
        image: ScreenRect,
        viewport: ScreenRect,
        field: &LiquifyField,
        view: &ViewOptions,
        proxy_scale: f64,
        dragging: bool,
    ) -> bool {
        let quality = if proxy_scale <= 1.0 { Quality::Precise } else { view.quality };
        if quality == Quality::Fast {
            self.status = "Reduced preview · final Apply is unchanged".into();
            return false;
        }
        if cfg!(target_arch = "wasm32") {
            self.status = "Native detail worker unavailable · reduced web preview".into();
            return false;
        }
        let region = match visible_region(self.bounds, image, viewport, ui.input(|i| i.max_texture_side)) {
            Ok(Some(r)) => r,
            Ok(None) => return false,
            Err(e) => {
                self.status = e;
                return false;
            }
        };
        let wanted = Key { revision: self.revision, region, before: view.before };
        self.poll(ui.ctx(), wanted);
        let settled = crate::gpu_canvas::now_ms() - self.changed_ms >= SETTLE_MS;
        let may_request = quality == Quality::Precise || (!dragging && settled);
        if self.ready.as_ref().is_none_or(|(key, _)| *key != wanted) && may_request && self.pending.is_none() && self.failed != Some(wanted) {
            self.request(ui.ctx(), wanted, field);
        }
        if let Some((key, texture)) = &self.ready
            && *key == wanted
        {
            let scale = image.size() / vec2(self.bounds.width() as f32, self.bounds.height() as f32);
            let rect = ScreenRect::from_min_size(
                image.min + vec2((region.x0 - self.bounds.x0) as f32, (region.y0 - self.bounds.y0) as f32) * scale,
                vec2(region.width() as f32, region.height() as f32) * scale,
            );
            let tint = Color32::from_white_alpha((view.working_opacity.clamp(0.0, 100.0) * 2.55).round() as u8);
            ui.painter().with_clip_rect(viewport).image(texture.id(), rect, ScreenRect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
            self.status = "Source-detail crop · final-render interpolation".into();
            return true;
        }
        if self.failed != Some(wanted) {
            self.status = if self.pending.is_some() { "Refining source detail…" } else { "Reduced preview · waiting for gesture to settle" }.into();
        }
        if self.pending.is_some() || (!settled && !dragging) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(25));
        }
        false
    }

    fn request(&mut self, ctx: &Context, key: Key, field: &LiquifyField) {
        if field.d.len() > MAX_FIELD_NODES {
            self.status = "Detail field exceeds the first-pass memory budget".into();
            self.failed = Some(key);
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let source = self.source.clone();
            let selection = self.selection.clone();
            let field = field.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            self.cancel = Arc::new(AtomicBool::new(false));
            let cancelled = self.cancel.clone();
            let ctx = ctx.clone();
            let spawn = std::thread::Builder::new().name("liquify-detail".into()).spawn(move || {
                // Old dialogs can finish at most one bounded region; cancellation prevents
                // stale uploads. The existing renderer is not interruptible within a tile.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if cancelled.load(Ordering::Relaxed) {
                        return Err("Superseded".to_string());
                    }
                    let mut rendered = if key.before { source.clone() } else { apply_liquify_region(&source, &field, key.region) };
                    if !key.before
                        && let Some(mask) = selection.as_ref()
                    {
                        photocraft_algo::liquify::precision::mix_selection_region(&source, &mut rendered, mask, key.region);
                    }
                    if cancelled.load(Ordering::Relaxed) {
                        return Err("Superseded".to_string());
                    }
                    let image = crate::distort_ui::surface_image(&rendered, key.region, key.region.width().max(key.region.height()) as usize);
                    Ok((key, image))
                }))
                .unwrap_or_else(|_| Err("Detail preview failed; the document is unchanged".into()));
                let _ = tx.send(result);
                ctx.request_repaint();
            });
            match spawn {
                Ok(_) => {
                    self.pending = Some(rx);
                    self.pending_key = Some(key);
                }
                Err(e) => {
                    self.status = format!("Cannot start detail preview: {e}");
                    self.failed = Some(key);
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (ctx, key);
        }
    }
}

fn visible_region(bounds: Rect, image: ScreenRect, viewport: ScreenRect, max_side: usize) -> Result<Option<Rect>, String> {
    if bounds.is_empty() || !image.is_finite() || image.width() <= 0.0 || image.height() <= 0.0 {
        return Ok(None);
    }
    let visible = image.intersect(viewport);
    if visible.width() <= 0.0 || visible.height() <= 0.0 {
        return Ok(None);
    }
    let sx = f64::from(bounds.width()) / f64::from(image.width());
    let sy = f64::from(bounds.height()) / f64::from(image.height());
    let region = Rect::new(
        (f64::from(bounds.x0) + f64::from(visible.left() - image.left()) * sx).floor().max(f64::from(bounds.x0)) as i32,
        (f64::from(bounds.y0) + f64::from(visible.top() - image.top()) * sy).floor().max(f64::from(bounds.y0)) as i32,
        (f64::from(bounds.x0) + f64::from(visible.right() - image.left()) * sx).ceil().min(f64::from(bounds.x1)) as i32,
        (f64::from(bounds.y0) + f64::from(visible.bottom() - image.top()) * sy).ceil().min(f64::from(bounds.y1)) as i32,
    )
    .intersect(&bounds);
    let pixels = u64::from(region.width()) * u64::from(region.height());
    if pixels > MAX_DETAIL_PIXELS || region.width() as usize > max_side || region.height() as usize > max_side {
        return Err("Zoom in for source-detail preview (4 MP visible-region budget)".into());
    }
    Ok((!region.is_empty()).then_some(region))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_crop_keeps_document_coordinates() {
        let image = ScreenRect::from_min_size(pos2(-100.0, -50.0), vec2(1000.0, 800.0));
        let view = ScreenRect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0));
        assert_eq!(visible_region(Rect::new(10, 20, 1010, 820), image, view, 8192).unwrap(), Some(Rect::new(110, 70, 510, 370)));
    }

    #[test]
    fn enormous_fit_view_is_not_an_unbounded_detail_allocation() {
        let view = ScreenRect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        assert!(visible_region(Rect::new(0, 0, 30_000, 20_000), view, view, 8192).is_err());
    }

    #[test]
    fn remembered_view_has_no_document_layer_reference() {
        let view = ViewOptions { reference: Reference::Layer(900), before: true, ..Default::default() };
        let v = view.remembered();
        assert!(v.get("referenceSource").is_none());
        assert!(v.get("showBefore").is_none());
    }

    #[test]
    fn reference_opacity_is_not_stored_in_geometry() {
        let mut view = ViewOptions::default();
        view.apply(&json!({"workingOpacity": -10.0, "previewQuality": "precise"})).unwrap();
        assert_eq!(view.working_opacity, 0.0);
        assert_eq!(view.quality, Quality::Precise);
        assert!(view.apply(&json!({"previewQuality": "not-a-mode"})).is_err());
    }

    #[test]
    fn completed_preview_keys_include_region_and_before_state() {
        let a = Key { revision: 3, region: Rect::new(0, 0, 10, 10), before: false };
        assert_ne!(a, Key { revision: 4, ..a });
        assert_ne!(a, Key { before: true, ..a });
        assert_ne!(a, Key { region: Rect::new(1, 0, 11, 10), ..a });
    }
}
