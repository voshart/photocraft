// SPDX-License-Identifier: MIT OR Apache-2.0
//! Authored Rust session tests. Run under the full host; not Python emulations.
use super::*;

fn app() -> PhotocraftApp {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.session.execute("file.new", json!({"width":300,"height":180,"depth":8})).unwrap();
    app.session.execute("layer.new.layer", json!({})).unwrap();
    app.session
        .edit("fixture", |doc, active| {
            let surf = doc.layer_mut(active.unwrap()).unwrap().surface_mut().unwrap();
            for y in 0..180 {
                for x in 0..300 {
                    surf.write_pixel(x, y, &[x as f32 / 300.0, y as f32 / 180.0, ((x / 8 + y / 8) % 2) as f32, 1.0]);
                }
            }
            Ok(())
        })
        .unwrap();
    app.sync_views();
    app
}
fn eye(app: &mut PhotocraftApp) {
    control(app, &json!({"placeRig":{"kind":"eye","bounds":[0,0,300,180]},"adjustGuides":false})).unwrap();
}

#[test]
fn place_guides_never_changes_pixels_or_field() {
    let ctx = egui::Context::default();
    let mut app = app();
    open(&mut app, &ctx).unwrap();
    let doc = app.session.active().unwrap().doc.clone();
    eye(&mut app);
    assert_eq!(doc, app.session.active().unwrap().doc);
    assert!(app.distort.liquify.as_ref().unwrap().field.is_identity());
    assert!(app.distort.liquify.as_ref().unwrap().strokes.is_empty());
}
#[test]
fn wink_is_one_replayable_undoable_operation() {
    let ctx = egui::Context::default();
    let mut app = app();
    open(&mut app, &ctx).unwrap();
    eye(&mut app);
    control(&mut app, &json!({"closeEye":0.85})).unwrap();
    let d = app.distort.liquify.as_ref().unwrap();
    let expected = d.field.clone();
    assert_eq!(d.strokes.len(), 1);
    assert!(d.strokes[0].rig.is_some());
    assert_eq!(LiquifyField::from_strokes(d.canvas, d.cell, &d.strokes), expected);
    control(&mut app, &json!({"undo":true})).unwrap();
    assert!(app.distort.liquify.as_ref().unwrap().field.is_identity());
    control(&mut app, &json!({"redo":true})).unwrap();
    assert_eq!(app.distort.liquify.as_ref().unwrap().field, expected);
    assert!(app.distort.liquify.as_ref().unwrap().rig.enabled);
}
#[test]
fn restore_all_is_undoable_including_protection() {
    let ctx = egui::Context::default();
    let mut app = app();
    open(&mut app, &ctx).unwrap();
    eye(&mut app);
    control(&mut app, &json!({"closeEye":0.5})).unwrap();
    control(&mut app, &json!({"maskAll":true})).unwrap();
    let original = app.distort.liquify.as_ref().unwrap().field.clone();
    control(&mut app, &json!({"restoreAll":true})).unwrap();
    let d = app.distort.liquify.as_ref().unwrap();
    assert!(d.field.is_identity());
    assert!(d.field.freeze.iter().all(|v| *v == 0.0));
    control(&mut app, &json!({"undo":true})).unwrap();
    assert_eq!(app.distort.liquify.as_ref().unwrap().field, original);
    control(&mut app, &json!({"redo":true})).unwrap();
    assert!(app.distort.liquify.as_ref().unwrap().field.is_identity());
}
#[test]
fn direct_drag_commits_once_and_replays_the_final_preview() {
    let ctx = egui::Context::default();
    let mut app = app();
    open(&mut app, &ctx).unwrap();
    eye(&mut app);
    // Upper middle of the default guide at (150,69.3).
    for event in [
        ToolEvent::Down { x: 150.0, y: 69.3, pressure: 1.0 },
        ToolEvent::Move { x: 151.0, y: 73.0, pressure: 1.0 },
        ToolEvent::Move { x: 150.0, y: 76.0, pressure: 1.0 },
        ToolEvent::Up { x: 150.0, y: 76.0 },
    ] {
        pointer(&mut app, event, egui::Modifiers::NONE);
    }
    let d = app.distort.liquify.as_ref().unwrap();
    assert_eq!(d.strokes.len(), 1);
    assert_eq!(LiquifyField::from_strokes(d.canvas, d.cell, &d.strokes), d.field);
}
#[test]
fn invalid_rig_command_leaves_document_unchanged() {
    let mut app = app();
    let original = app.session.active().unwrap().doc.clone();
    let result = app
        .session
        .execute("filter.liquify", json!({"strokes":[{"tool":"rigWarpV1","rig":{"kind":"eye","bounds":[0,0,20,20],"source":[],"target":[],"pinned":[]}}]}));
    assert!(result.is_err());
    assert_eq!(app.session.active().unwrap().doc, original);
}
#[test]
fn failed_commit_retains_the_local_edit() {
    let ctx = egui::Context::default();
    let mut app = app();
    open(&mut app, &ctx).unwrap();
    eye(&mut app);
    control(&mut app, &json!({"closeEye":0.5})).unwrap();
    let expected = app.distort.liquify.as_ref().unwrap().field.clone();
    let layer = app.distort.liquify.as_ref().unwrap().layer;
    app.session
        .edit("lock fixture", |doc, _| {
            doc.layer_mut(layer).unwrap().locks.all = true;
            Ok(())
        })
        .unwrap();
    assert!(control(&mut app, &json!({"commit":true})).is_err());
    let d = app.distort.liquify.as_ref().unwrap();
    assert_eq!(d.field, expected);
    assert_eq!(d.strokes.len(), 1);
}
#[test]
fn switching_preview_quality_does_not_change_wink_commit() {
    let ctx = egui::Context::default();
    let mut app = app();
    open(&mut app, &ctx).unwrap();
    eye(&mut app);
    control(&mut app, &json!({"closeEye":0.5})).unwrap();
    let d = app.distort.liquify.as_ref().unwrap();
    let expected = d.field.clone();
    let layer = d.layer;
    let source = app.session.active().unwrap().doc.layer(layer).unwrap().surface().unwrap();
    let final_image = photocraft_algo::liquify::apply_liquify(source, &expected);
    for q in ["fast", "balanced", "precise"] {
        control(&mut app, &json!({"previewQuality":q})).unwrap();
        assert_eq!(app.distort.liquify.as_ref().unwrap().field, expected);
    }
    control(&mut app, &json!({"commit":true})).unwrap();
    assert_eq!(app.session.active().unwrap().doc.layer(layer).unwrap().surface().unwrap(), &final_image);
}
