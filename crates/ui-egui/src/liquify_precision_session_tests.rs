// SPDX-License-Identifier: MIT OR Apache-2.0
//! End-to-end session regressions for the precision extension (no native window).
use super::*;
use photocraft_algo::liquify::apply_liquify;

fn app() -> PhotocraftApp {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.session.execute("file.new", json!({"width": 120, "height": 80, "depth": 8})).unwrap();
    app.session.execute("layer.new.layer", json!({})).unwrap();
    app.session
        .edit("fixture", |doc, active| {
            let s = doc.layer_mut(active.unwrap()).unwrap().surface_mut().unwrap();
            for y in 0..80 {
                for x in 0..120 {
                    s.write_pixel(x, y, &[x as f32 / 120.0, ((x / 4 + y / 4) % 2) as f32, y as f32 / 80.0, 1.0]);
                }
            }
            Ok(())
        })
        .unwrap();
    app.sync_views();
    app
}

fn draw(app: &mut PhotocraftApp) {
    for event in [ToolEvent::Down { x: 40.0, y: 40.0, pressure: 1.0 }, ToolEvent::Move { x: 54.0, y: 42.0, pressure: 1.0 }, ToolEvent::Up { x: 54.0, y: 42.0 }]
    {
        pointer(app, event, egui::Modifiers::NONE);
    }
}

#[test]
fn precision_stroke_undo_redo_replays_exactly() {
    let ctx = egui::Context::default();
    let mut app = app();
    open(&mut app, &ctx).unwrap();
    control(&mut app, &json!({"tool":"precisionWarp", "pressure":35, "density":65})).unwrap();
    draw(&mut app);
    let expected = app.distort.liquify.as_ref().unwrap().field.clone();
    control(&mut app, &json!({"undo":true})).unwrap();
    assert!(app.distort.liquify.as_ref().unwrap().field.is_identity());
    control(&mut app, &json!({"redo":true})).unwrap();
    let d = app.distort.liquify.as_ref().unwrap();
    assert_eq!(d.field, expected);
    assert_eq!(LiquifyField::from_strokes(d.canvas, d.cell, &d.strokes), expected);
}

#[test]
fn all_preview_options_leave_document_and_deformation_unchanged() {
    let ctx = egui::Context::default();
    let mut app = app();
    open(&mut app, &ctx).unwrap();
    control(&mut app, &json!({"tool":"precisionWarp"})).unwrap();
    draw(&mut app);
    let document = app.session.active().unwrap().doc.clone();
    let history = app.session.active().unwrap().history.past_len();
    let field = app.distort.liquify.as_ref().unwrap().field.clone();
    let strokes = app.distort.liquify.as_ref().unwrap().strokes.clone();
    let target = app.distort.liquify.as_ref().unwrap().layer;
    for q in ["fast", "balanced", "precise"] {
        control(&mut app, &json!({"previewQuality":q, "workingOpacity":32, "referenceOnTop":true, "referenceSource":"before", "showBefore":true})).unwrap();
        let d = app.distort.liquify.as_ref().unwrap();
        assert_eq!(d.field, field);
        assert_eq!(d.strokes, strokes);
        assert_eq!(d.layer, target);
        assert_eq!(app.session.active().unwrap().doc, document);
        assert_eq!(app.session.active().unwrap().history.past_len(), history);
    }
}

#[test]
fn apply_ignores_preview_opacity_and_before_view() {
    let ctx = egui::Context::default();
    let mut app = app();
    open(&mut app, &ctx).unwrap();
    control(&mut app, &json!({"tool":"precisionWarp"})).unwrap();
    draw(&mut app);
    let (target, expected) = {
        let d = app.distort.liquify.as_ref().unwrap();
        let source = app.session.active().unwrap().doc.layer(d.layer).unwrap().surface().unwrap();
        (d.layer, apply_liquify(source, &d.field))
    };
    control(&mut app, &json!({"previewQuality":"fast", "workingOpacity":0, "showBefore":true})).unwrap();
    let history = app.session.active().unwrap().history.past_len();
    commit(&mut app);
    assert!(app.distort.liquify.is_none());
    assert_eq!(app.session.active().unwrap().history.past_len(), history + 1);
    assert_eq!(app.session.active().unwrap().doc.layer(target).unwrap().surface().unwrap(), &expected);
}

#[test]
fn cancel_does_not_touch_any_pixels() {
    let ctx = egui::Context::default();
    let mut app = app();
    let original = app.session.active().unwrap().doc.clone();
    let history = app.session.active().unwrap().history.past_len();
    open(&mut app, &ctx).unwrap();
    control(&mut app, &json!({"tool":"precisionWarp"})).unwrap();
    draw(&mut app);
    cancel(&mut app);
    assert_eq!(app.session.active().unwrap().doc, original);
    assert_eq!(app.session.active().unwrap().history.past_len(), history);
}
