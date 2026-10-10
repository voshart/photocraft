// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reopen/edit/apply transaction for Liquify. The document is immutable until the
//! engine accepts a command. In particular, a reopened filter samples its INPUT,
//! never its already-deformed cache. Existing filter blend/mask state stays owned
//! by the host; this dialog explicitly shows the isolated filter operation.

use super::{LiquifyDialog, PhotocraftApp};
use egui::Context;
use photocraft_algo::liquify::{LiquifyStroke, rig::RigWarp};
use photocraft_doc::{Document, LayerContent, LayerId};
use photocraft_engine::jobs::JobId;
use photocraft_geom::Rect;
use serde_json::{Value, json};
use std::sync::Arc;

const EDITOR_KEY: &str = "precisionEditor";
const MAX_STROKES: usize = 10_000;
const MAX_POINTS: usize = 500_000;
const MAX_REOPEN_NODES: u64 = 4_000_000;

#[derive(Clone)]
enum Target {
    New,
    Existing { index: usize, params: Value, strokes: Vec<LiquifyStroke>, cell: f64, guide: Option<Box<RigWarp>> },
}

pub(super) struct Origin {
    document: Arc<Document>,
    target: Target,
    pending: Option<JobId>,
    /// An unresolvable submitted job must NOT be automatically retried: its
    /// outcome might already have been applied by the host.
    blocked: Option<String>,
}

impl Origin {
    pub fn new(document: Arc<Document>) -> Self {
        Self { document, target: Target::New, pending: None, blocked: None }
    }

    pub fn describe(&self) -> Value {
        let index = match &self.target {
            Target::New => None,
            Target::Existing { index, .. } => Some(*index),
        };
        json!({"filterIndex":index,"isolatedFilterPreview":index.is_some(),
            "pendingJob":self.pending.map(|j|j.0),"blocked":self.blocked})
    }

    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    pub fn isolated(&self) -> bool {
        matches!(self.target, Target::Existing { .. })
    }

    fn validate(&self, app: &PhotocraftApp, layer: LayerId) -> Result<(), String> {
        if let Some(reason) = &self.blocked {
            return Err(reason.clone());
        }
        let current = app.session.active().ok_or("The edited document is no longer open")?;
        if current.doc.id != self.document.id || !Arc::ptr_eq(&current.doc, &self.document) {
            return Err(
                "Document changed while Liquify was open. Local edits are retained; cancel and reopen on the new document state before applying.".into()
            );
        }
        if current.doc.layer(layer).is_none() {
            return Err("The edited layer no longer exists".into());
        }
        Ok(())
    }
}

fn read_guide(params: &Value, canvas: Rect, cell: f64) -> Result<Option<Option<RigWarp>>, String> {
    let Some(editor) = params.get(EDITOR_KEY) else { return Ok(None) };
    if editor.get("version").and_then(Value::as_u64) != Some(1) {
        return Err("Unsupported Liquify editor metadata version; the saved filter has not been changed".into());
    }
    let value = editor.get("guide").ok_or("Liquify editor metadata is missing guide")?;
    if value.is_null() {
        return Ok(Some(None));
    }
    let guide: RigWarp = serde_json::from_value(value.clone()).map_err(|e| format!("Invalid saved Liquify guides: {e}"))?;
    guide.validate_grid(canvas, cell)?;
    if !guide.is_identity() {
        return Err("Saved guides must describe current positions, not an unapplied deformation".into());
    }
    Ok(Some(Some(guide)))
}

/// Reject costly or malformed replay requests before allocating a replacement
/// field. This supplements (does not replace) the engine's stroke validation.
fn check_replay_budget(params: &Value, canvas: Rect, cell: f64) -> Result<(), String> {
    let strokes = params.get("strokes").and_then(Value::as_array);
    if let Some(strokes) = strokes {
        if strokes.len() > MAX_STROKES {
            return Err("Saved Liquify session exceeds the 10000-operation reopening budget".into());
        }
        let mut count = 0usize;
        for stroke in strokes {
            let n = stroke.get("points").and_then(Value::as_array).map_or(0, Vec::len);
            count = count.checked_add(n).ok_or("Saved Liquify point count overflow")?;
            if count > MAX_POINTS {
                return Err("Saved Liquify session exceeds the 500000-point reopening budget".into());
            }
        }
    }
    check_field_budget(canvas, cell)
}

/// Validate every session's field before allocating it, including new sessions.
/// Reopened sessions must allocate at their saved spacing, not at auto_cell first.
pub(super) fn check_field_budget(canvas: Rect, cell: f64) -> Result<(), String> {
    if canvas.is_empty() {
        return Err("Cannot open Liquify on an empty canvas".into());
    }
    if !cell.is_finite() || !(1.0..=64.0).contains(&cell) {
        return Err("Invalid saved Liquify field spacing".into());
    }
    let width = (f64::from(canvas.width()) / cell).ceil() as u64 + 2;
    let height = (f64::from(canvas.height()) / cell).ceil() as u64 + 2;
    if width.saturating_mul(height) > MAX_REOPEN_NODES {
        return Err("Liquify field exceeds the four-million-node session budget".into());
    }
    Ok(())
}

pub(super) fn open_existing(app: &mut PhotocraftApp, ctx: &Context, layer: LayerId, index: usize) -> Result<(), String> {
    if app.distort.liquify.is_some() {
        return Err("Finish or cancel the current Liquify session first".into());
    }
    let (source, params, strokes, cell, saved_guide) = {
        let st = app.session.active().ok_or("No document")?;
        let LayerContent::Smart(sm) = &st.doc.layer(layer).ok_or("No such layer")?.content else {
            return Err("Reopening Liquify requires a Smart Object layer".into());
        };
        let filter = sm.smart_filters.get(index).ok_or("No such Smart Filter")?;
        if filter.command != "filter.liquify" {
            return Err("The selected Smart Filter is not Liquify".into());
        }
        if !filter.params.is_object() {
            return Err("Saved Liquify parameters must be an object".into());
        }
        let canvas = st.doc.bounds();
        // Bound the JSON arrays before deserializing all the stroke data.
        let guess = filter.params.get("meshSize").and_then(Value::as_f64).unwrap_or_else(|| photocraft_algo::liquify::auto_cell(canvas));
        check_replay_budget(&filter.params, canvas, guess)?;
        let (strokes, cell) = photocraft_engine::distort_cmds::liquify_params("filter.liquify", &filter.params, canvas).map_err(|e| e.to_string())?;
        let guide = read_guide(&filter.params, canvas, cell)?;
        let source = photocraft_engine::smart_cmds::render_below_filter(&st.doc, sm, index)
            .map_err(|e| e.to_string())?
            .ok_or("The Smart Object source is unavailable; refusing to replay on its deformed cache")?;
        (source, filter.params.clone(), strokes, cell, guide)
    };
    // Reediting a stored filter must not capture an unrelated current selection.
    // This is deliberately an isolated, unmasked filter preview (labelled in UI).
    super::open_pixels(app, ctx, layer, source, false, Some(cell))?;
    let d = app.distort.liquify.as_mut().ok_or("Liquify dialog did not open")?;
    d.strokes = strokes.clone();
    d.cell = cell;
    d.rebuild();
    if let Some(guide) = saved_guide {
        d.rig.restore_saved(guide);
    }
    let guide = d.rig.saved_guide().map(Box::new);
    d.origin.target = Target::Existing { index, params, strokes, cell, guide };
    Ok(())
}

/// View settings are intentionally absent. Unknown root and editor metadata are
/// carried forward so editing one supported field does not discard other data.
fn parameters(old: &Value, strokes: &[LiquifyStroke], cell: f64, guide: Option<RigWarp>) -> Result<Value, String> {
    let mut params = old.clone();
    let object = params.as_object_mut().ok_or("Liquify parameters must be an object")?;
    object.insert("strokes".into(), serde_json::to_value(strokes).map_err(|e| e.to_string())?);
    object.insert("meshSize".into(), json!(cell));
    let mut editor = object.get(EDITOR_KEY).cloned().unwrap_or_else(|| json!({"version":1}));
    let metadata = editor.as_object_mut().ok_or("Liquify editor metadata must be an object")?;
    metadata.insert("guide".into(), serde_json::to_value(guide).map_err(|e| e.to_string())?);
    object.insert(EDITOR_KEY.into(), editor);
    Ok(params)
}

/// None means a true no-op, not an identity effect appended to the filter stack.
fn plan(d: &LiquifyDialog) -> Result<Option<(&'static str, Value)>, String> {
    let guide = d.rig.saved_guide();
    match &d.origin.target {
        Target::New => {
            if d.strokes.is_empty() {
                return Ok(None);
            }
            let mut params = parameters(&json!({}), &d.strokes, d.cell, guide)?;
            let object = params.as_object_mut().ok_or("Invalid Liquify command object")?;
            object.insert("layer".into(), json!(d.layer.0));
            Ok(Some(("filter.liquify", params)))
        }
        Target::Existing { index, params, strokes, cell, guide: original_guide } => {
            if &d.strokes == strokes && d.cell == *cell && guide.as_ref() == original_guide.as_deref() {
                return Ok(None);
            }
            let params = parameters(params, &d.strokes, d.cell, guide)?;
            Ok(Some(("layer.smartFilter.setParams", json!({"layer":d.layer.0,"index":index,"params":params}))))
        }
    }
}

pub(super) fn has_pending(app: &PhotocraftApp) -> bool {
    app.distort.liquify.as_ref().is_some_and(|d| d.origin.pending.is_some())
}

pub(super) fn request_cancel(app: &mut PhotocraftApp) -> bool {
    let id = app.distort.liquify.as_ref().and_then(|d| d.origin.pending);
    if let Some(id) = id {
        app.session.cancel_job(id);
        true
    } else {
        false
    }
}

pub(super) fn commit(app: &mut PhotocraftApp) {
    if has_pending(app) {
        return;
    }
    let Some(mut d) = app.distort.liquify.take() else { return };
    super::remember(app, &d.opts);
    d.end();
    let result = plan(&d).and_then(|command| {
        let Some((id, params)) = command else {
            return Ok(json!({"changed":false}));
        };
        d.origin.validate(app, d.layer)?;
        app.run(id, params)
    });
    match result {
        Ok(value) if value.get("pending").and_then(Value::as_bool) == Some(true) => {
            if let Some(id) = value.get("job").and_then(Value::as_u64) {
                d.origin.pending = Some(JobId(id));
            } else {
                let message = "Apply was submitted without a job id. Verify document history before retrying; this local session has been retained.";
                d.origin.blocked = Some(message.into());
                app.ui.status = message.into();
                app.ui.status_error = true;
            }
            app.distort.liquify = Some(d);
        }
        Ok(_) => {} // synchronous completion (or unchanged): close only now
        Err(error) => {
            app.ui.status = error;
            app.ui.status_error = true;
            app.distort.liquify = Some(d);
        }
    }
}

pub(super) fn commit_reply(app: &PhotocraftApp) -> Result<Value, String> {
    if let Some(d) = &app.distort.liquify {
        if let Some(id) = d.origin.pending {
            return Ok(json!({"pending":true,"job":id.0,"committed":false}));
        }
        return Err(app.ui.status.clone());
    }
    Ok(json!({"committed":true}))
}

/// The host polls jobs before drawing the dialog. A submitted request is not a
/// successful commit. On failed/cancelled work, restore local interactivity.
pub(super) fn poll_pending(app: &mut PhotocraftApp, ctx: &Context) -> bool {
    let Some(id) = app.distort.liquify.as_ref().and_then(|d| d.origin.pending) else { return false };
    let job = app.session.job(id);
    match job.as_ref().map(|j| j.state) {
        Some("done") => {
            app.distort.liquify = None;
            return true;
        }
        Some("failed") | Some("cancelled") | None => {
            let missing = job.is_none();
            let message = job.as_ref().and_then(|j| j.error.clone()).unwrap_or_else(|| {
                if missing {
                    "Apply outcome is unavailable. Verify document history; automatic retry is disabled.".into()
                } else {
                    "Apply cancelled; local Liquify edits are retained.".into()
                }
            });
            if let Some(d) = &mut app.distort.liquify {
                d.origin.pending = None;
                if missing {
                    d.origin.blocked = Some(message.clone());
                }
            }
            app.ui.status = message;
            app.ui.status_error = true;
            return false;
        }
        _ => {}
    }
    egui::Window::new(tl!("Applying Liquify")).collapsible(false).resizable(false).show(ctx, |ui| {
        ui.label(tl!("Local edits are retained until the operation finishes."));
        if let Some(job) = &job {
            ui.add(egui::ProgressBar::new(job.progress).show_percentage());
        }
        if ui.button(tl!("Cancel operation")).clicked() {
            app.session.cancel_job(id);
        }
    });
    ctx.request_repaint_after(std::time::Duration::from_millis(25));
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use photocraft_algo::liquify::rig::RegionKind;

    #[test]
    fn future_editor_metadata_is_rejected_not_reinterpreted() {
        assert!(read_guide(&json!({"precisionEditor":{"version":2,"guide":null}}), Rect::new(0, 0, 100, 100), 2.0).is_err());
    }

    #[test]
    fn metadata_absent_and_explicitly_cleared_are_distinct() {
        let canvas = Rect::new(0, 0, 100, 100);
        assert_eq!(read_guide(&json!({}), canvas, 2.0).unwrap(), None);
        assert_eq!(read_guide(&json!({"precisionEditor":{"version":1,"guide":null}}), canvas, 2.0).unwrap(), Some(None));
    }

    #[test]
    fn saved_current_guides_round_trip_and_unknown_metadata_survives() {
        let rig = RigWarp::new(RegionKind::Eye, [0.0, 0.0, 100.0, 80.0]).unwrap();
        let old = json!({"custom":17,"precisionEditor":{"version":1,"other":"preserve"}});
        let params = parameters(&old, &[], 2.0, Some(rig.clone())).unwrap();
        assert_eq!(params["custom"], 17);
        assert_eq!(params["precisionEditor"]["other"], "preserve");
        assert_eq!(read_guide(&params, Rect::new(0, 0, 100, 80), 2.0).unwrap(), Some(Some(rig)));
        assert!(params.get("workingOpacity").is_none());
    }

    #[test]
    fn saved_guides_cannot_smuggle_an_unapplied_warp() {
        let rig = RigWarp::new(RegionKind::Eye, [0.0, 0.0, 100.0, 80.0]).unwrap().close_eye(0.25).unwrap();
        let params = json!({"precisionEditor":{"version":1,"guide":rig}});
        assert!(read_guide(&params, Rect::new(0, 0, 100, 80), 2.0).is_err());
    }

    #[test]
    fn enormous_reopen_field_is_rejected_before_allocation() {
        assert!(check_replay_budget(&json!({}), Rect::new(0, 0, 100_000, 100_000), 1.0).is_err());
    }
}

#[cfg(test)]
mod allocation_tests {
    use super::*;

    #[test]
    fn field_budget_rejects_bad_spacing_and_empty_canvas() {
        for cell in [0.0, f64::NAN, f64::INFINITY, 65.0] {
            assert!(check_field_budget(Rect::new(0, 0, 100, 100), cell).is_err());
        }
        assert!(check_field_budget(Rect::EMPTY, 2.0).is_err());
        assert!(check_field_budget(Rect::new(0, 0, 3840, 2160), 4.0).is_ok());
    }
}
