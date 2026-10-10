// SPDX-License-Identifier: MIT OR Apache-2.0
//! Native/engine integration regressions. Authored here; run with Cargo before
//! treating the source bundle as a verified build.
use super::*;
use photocraft_algo::liquify::apply_liquify;
use photocraft_doc::{LayerContent, SmartObject};

fn fixture(smart: bool) -> PhotocraftApp {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.session.execute("file.new", json!({"width":120,"height":80,"depth":8})).unwrap();
    app.session.execute("layer.new.layer", json!({})).unwrap();
    app.session
        .edit("fixture", |doc, active| {
            let surface = doc.layer_mut(active.unwrap()).unwrap().surface_mut().unwrap();
            for y in 0..80 {
                for x in 0..120 {
                    surface.write_pixel(x, y, &[x as f32 / 120.0, ((x / 4 + y / 4) % 2) as f32, y as f32 / 80.0, 1.0]);
                }
            }
            Ok(())
        })
        .unwrap();
    if smart {
        app.session.execute("filter.convertForSmartFilters", json!({})).unwrap();
    }
    app.sync_views();
    app
}

fn target(app: &PhotocraftApp) -> LayerId {
    app.session.active().unwrap().active_layer.unwrap()
}
fn smart(app: &PhotocraftApp, layer: LayerId) -> &SmartObject {
    let LayerContent::Smart(sm) = &app.session.active().unwrap().doc.layer(layer).unwrap().content else {
        panic!("fixture smart object");
    };
    sm
}
fn stroke(app: &mut PhotocraftApp, x: f64) {
    control(app, &json!({"tool":"precisionWarp","size":40,"pressure":35})).unwrap();
    for event in
        [ToolEvent::Down { x, y: 40.0, pressure: 1.0 }, ToolEvent::Move { x: x + 12.0, y: 42.0, pressure: 1.0 }, ToolEvent::Up { x: x + 12.0, y: 42.0 }]
    {
        pointer(app, event, egui::Modifiers::NONE);
    }
}
fn first_filter(app: &mut PhotocraftApp, ctx: &egui::Context) -> LayerId {
    let layer = target(app);
    open(app, ctx).unwrap();
    stroke(app, 35.0);
    commit(app);
    assert!(app.distort.liquify.is_none());
    assert_eq!(smart(app, layer).smart_filters.len(), 1);
    layer
}

#[test]
fn reopening_without_changes_does_not_resample_or_add_history() {
    let ctx = egui::Context::default();
    let mut app = fixture(true);
    let layer = first_filter(&mut app, &ctx);
    let original = app.session.active().unwrap().doc.clone();
    let history = app.session.active().unwrap().history.past_len();
    open_smart_filter(&mut app, &ctx, layer, 0).unwrap();
    control(&mut app, &json!({"previewQuality":"precise","workingOpacity":25,"showBefore":true})).unwrap();
    commit(&mut app);
    assert!(app.distort.liquify.is_none());
    assert_eq!(app.session.active().unwrap().doc, original);
    assert_eq!(app.session.active().unwrap().history.past_len(), history);
}

#[test]
fn reopening_uses_input_pixels_not_the_already_deformed_cache() {
    let ctx = egui::Context::default();
    let mut app = fixture(true);
    let layer = first_filter(&mut app, &ctx);
    let before = smart(&app, layer).cache.clone().unwrap();
    open_smart_filter(&mut app, &ctx, layer, 0).unwrap();
    let doc = &app.session.active().unwrap().doc;
    let source = photocraft_engine::smart_cmds::render_below_filter(doc, smart(&app, layer), 0).unwrap().unwrap();
    let d = app.distort.liquify.as_ref().unwrap();
    assert_eq!(apply_liquify(&source, &d.field), before);
    assert_eq!(d.origin.describe()["filterIndex"], 0);
}

#[test]
fn editing_an_existing_filter_replaces_its_parameters_in_place() {
    let ctx = egui::Context::default();
    let mut app = fixture(true);
    let layer = first_filter(&mut app, &ctx);
    let history = app.session.active().unwrap().history.past_len();
    open_smart_filter(&mut app, &ctx, layer, 0).unwrap();
    stroke(&mut app, 70.0);
    commit(&mut app);
    assert!(app.distort.liquify.is_none());
    assert_eq!(smart(&app, layer).smart_filters.len(), 1);
    assert_eq!(smart(&app, layer).smart_filters[0].params["strokes"].as_array().unwrap().len(), 2);
    assert_eq!(app.session.active().unwrap().history.past_len(), history + 1);
}

#[test]
fn undoing_every_stored_stroke_can_reset_the_existing_filter() {
    let ctx = egui::Context::default();
    let mut app = fixture(true);
    let layer = first_filter(&mut app, &ctx);
    let source = photocraft_engine::smart_cmds::render_below_filter(&app.session.active().unwrap().doc, smart(&app, layer), 0).unwrap().unwrap();
    open_smart_filter(&mut app, &ctx, layer, 0).unwrap();
    control(&mut app, &json!({"undo":true})).unwrap();
    commit(&mut app);
    assert!(app.distort.liquify.is_none());
    assert_eq!(smart(&app, layer).smart_filters.len(), 1);
    assert_eq!(smart(&app, layer).cache.as_ref().unwrap(), &source);
}

#[test]
fn native_project_preserves_guides_and_pins_without_a_model() {
    let ctx = egui::Context::default();
    let mut app = fixture(true);
    let layer = first_filter(&mut app, &ctx);
    open_smart_filter(&mut app, &ctx, layer, 0).unwrap();
    control(&mut app, &json!({"placeRig":{"kind":"eye","bounds":[10,10,110,70]},"pinHandle":{"index":0,"pinned":true}})).unwrap();
    let guide = app.distort.liquify.as_ref().unwrap().rig.saved_guide();
    commit(&mut app);
    let bytes = photocraft_format::save_to_bytes(&app.session.active().unwrap().doc, &Default::default()).unwrap();
    let loaded = photocraft_format::load_from_bytes(&bytes).unwrap();
    let reloaded = loaded
        .layers
        .iter()
        .find_map(|layer| match &layer.content {
            LayerContent::Smart(sm) => Some(sm),
            _ => None,
        })
        .unwrap();
    assert_eq!(reloaded.smart_filters[0].params["precisionEditor"]["guide"], serde_json::to_value(&guide).unwrap());
    let loaded_layer = loaded.layers.iter().find(|layer| matches!(layer.content, LayerContent::Smart(_))).unwrap().id;
    app.session
        .edit("load native fixture", move |doc, active| {
            *doc = loaded;
            *active = Some(loaded_layer);
            Ok(())
        })
        .unwrap();
    app.sync_views();
    open_smart_filter(&mut app, &ctx, loaded_layer, 0).unwrap();
    assert_eq!(app.distort.liquify.as_ref().unwrap().rig.saved_guide(), guide);
    assert!(app.distort.liquify.as_ref().unwrap().rig.saved_guide().unwrap().pinned[0]);
}

#[test]
fn a_stale_document_does_not_receive_local_edits() {
    let ctx = egui::Context::default();
    let mut app = fixture(false);
    open(&mut app, &ctx).unwrap();
    stroke(&mut app, 35.0);
    app.session
        .edit("external change", |doc, _| {
            doc.name = "Changed elsewhere".into();
            Ok(())
        })
        .unwrap();
    let document = app.session.active().unwrap().doc.clone();
    commit(&mut app);
    assert!(app.distort.liquify.is_some());
    assert_eq!(app.session.active().unwrap().doc, document);
    assert!(app.ui.status.contains("Document changed"));
}

#[test]
fn protection_painting_keeps_manually_fitted_guides() {
    let ctx = egui::Context::default();
    let mut app = fixture(false);
    open(&mut app, &ctx).unwrap();
    control(&mut app, &json!({"placeRig":{"kind":"eye","bounds":[10,10,110,70]}})).unwrap();
    let guide = app.distort.liquify.as_ref().unwrap().rig.saved_guide();
    control(&mut app, &json!({"tool":"freeze","size":20})).unwrap();
    pointer(&mut app, ToolEvent::Down { x: 20.0, y: 20.0, pressure: 1.0 }, egui::Modifiers::NONE);
    pointer(&mut app, ToolEvent::Up { x: 20.0, y: 20.0 }, egui::Modifiers::NONE);
    assert_eq!(app.distort.liquify.as_ref().unwrap().rig.saved_guide(), guide);
}

#[test]
fn before_view_rejects_expression_macros_without_a_field_change() {
    let ctx = egui::Context::default();
    let mut app = fixture(false);
    open(&mut app, &ctx).unwrap();
    control(&mut app, &json!({"placeRig":{"kind":"eye","bounds":[10,10,110,70]},"showBefore":true})).unwrap();
    let field = app.distort.liquify.as_ref().unwrap().field.clone();
    assert!(control(&mut app, &json!({"closeEye":0.25})).is_err());
    assert_eq!(app.distort.liquify.as_ref().unwrap().field, field);
}

#[test]
fn undo_first_facial_edit_keeps_pre_gesture_controls() {
    let ctx = egui::Context::default();
    let mut app = fixture(false);
    open(&mut app, &ctx).unwrap();
    control(&mut app, &json!({"placeRig":{"kind":"eye","bounds":[10,10,110,70]}})).unwrap();
    let guide = app.distort.liquify.as_ref().unwrap().rig.saved_guide();
    control(&mut app, &json!({"closeEye":0.25})).unwrap();
    control(&mut app, &json!({"undo":true})).unwrap();
    assert_eq!(app.distort.liquify.as_ref().unwrap().rig.saved_guide(), guide);
    assert!(app.distort.liquify.as_ref().unwrap().field.is_identity());
}

#[test]
fn smart_filter_row_routes_into_custom_liquify_dialog() {
    let ctx = egui::Context::default();
    let mut app = fixture(true);
    let layer = first_filter(&mut app, &ctx);
    let sm = smart(&app, layer).clone();
    crate::smart_ui::open_editor(&mut app, &ctx, layer.0, &sm, 0);
    assert!(app.distort.liquify.is_some());
    assert_eq!(app.distort.liquify.as_ref().unwrap().layer, layer);
}

#[test]
fn undo_protection_preserves_manual_setup_and_pin_changes() {
    let ctx = egui::Context::default();
    let mut app = fixture(false);
    open(&mut app, &ctx).unwrap();
    control(&mut app, &json!({"placeRig":{"kind":"eye","bounds":[10,10,110,70]},"pinHandle":{"index":0,"pinned":true}})).unwrap();
    let guide = app.distort.liquify.as_ref().unwrap().rig.saved_guide();
    control(&mut app, &json!({"maskAll":true})).unwrap();
    control(&mut app, &json!({"undo":true})).unwrap();
    assert_eq!(app.distort.liquify.as_ref().unwrap().rig.saved_guide(), guide);
    assert!(app.distort.liquify.as_ref().unwrap().rig.adjust);
    control(&mut app, &json!({"redo":true})).unwrap();
    assert_eq!(app.distort.liquify.as_ref().unwrap().rig.saved_guide(), guide);
}

#[test]
fn new_filter_proxy_preserves_pixels_outside_the_selection() {
    let ctx = egui::Context::default();
    let mut app = fixture(false);
    app.session
        .edit("selection", |doc, _| {
            let mut mask = photocraft_raster::Surface::new(photocraft_color::PixelFormat::GRAY8);
            mask.fill_rect(Rect::new(0, 0, 12, 80), &[1.0]);
            doc.selection = Some(mask);
            Ok(())
        })
        .unwrap();
    open(&mut app, &ctx).unwrap();
    stroke(&mut app, 40.0);
    let d = app.distort.liquify.as_ref().unwrap();
    let index = 40 * d.proxy.w + 48;
    let expected = d.proxy.px[index].map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
    assert_eq!(d.out[index], expected);
}

#[test]
fn existing_filter_preview_ignores_a_new_document_selection() {
    let ctx = egui::Context::default();
    let mut app = fixture(true);
    let layer = first_filter(&mut app, &ctx);
    app.session
        .edit("unrelated selection", |doc, _| {
            doc.selection = Some(photocraft_raster::Surface::new(photocraft_color::PixelFormat::GRAY8));
            Ok(())
        })
        .unwrap();
    open_smart_filter(&mut app, &ctx, layer, 0).unwrap();
    let d = app.distort.liquify.as_ref().unwrap();
    let mut expected = vec![[0u8; 4]; d.proxy.w * d.proxy.h];
    d.proxy.render(&d.field, [0, 0, d.proxy.w, d.proxy.h], &mut expected);
    assert_eq!(d.out, expected);
    assert!(d.origin.isolated());
}

#[test]
fn pin_press_is_captured_until_release_and_toggles_only_once() {
    let ctx = egui::Context::default();
    let mut app = fixture(false);
    open(&mut app, &ctx).unwrap();
    control(&mut app, &json!({"placeRig":{"kind":"eye","bounds":[10,10,110,70]}})).unwrap();
    let p = app.distort.liquify.as_ref().unwrap().rig.saved_guide().unwrap().target[0];
    let down = || ToolEvent::Down { x: p[0], y: p[1], pressure: 1.0 };
    pointer(&mut app, down(), egui::Modifiers::SHIFT);
    assert!(app.distort.liquify.as_ref().unwrap().rig.pointer_captured());
    // The per-frame fallback must not synthesize another Down. Also tolerate a
    // duplicated control-channel Down while this physical press is held.
    pointer(&mut app, down(), egui::Modifiers::SHIFT);
    assert!(app.distort.liquify.as_ref().unwrap().rig.saved_guide().unwrap().pinned[0]);
    pointer(&mut app, ToolEvent::Up { x: p[0], y: p[1] }, egui::Modifiers::SHIFT);
    assert!(!app.distort.liquify.as_ref().unwrap().rig.pointer_captured());
    pointer(&mut app, down(), egui::Modifiers::SHIFT);
    assert!(!app.distort.liquify.as_ref().unwrap().rig.saved_guide().unwrap().pinned[0]);
    pointer(&mut app, ToolEvent::Up { x: p[0], y: p[1] }, egui::Modifiers::NONE);
    assert!(app.distort.liquify.as_ref().unwrap().strokes.is_empty());
}

#[test]
fn fully_frozen_macro_keeps_fitted_guides_and_history() {
    let ctx = egui::Context::default();
    let mut app = fixture(false);
    open(&mut app, &ctx).unwrap();
    control(&mut app, &json!({"placeRig":{"kind":"eye","bounds":[10,10,110,70]},"maskAll":true})).unwrap();
    let d = app.distort.liquify.as_ref().unwrap();
    let guide = d.rig.saved_guide();
    let field = d.field.clone();
    let count = d.strokes.len();
    control(&mut app, &json!({"closeEye":0.25})).unwrap();
    let d = app.distort.liquify.as_ref().unwrap();
    assert_eq!(d.field, field);
    assert_eq!(d.rig.saved_guide(), guide);
    assert_eq!(d.strokes.len(), count);
    assert!(d.rig.status.contains("No pixel movement"));
}

#[test]
fn fully_frozen_drag_does_not_move_guides_or_commit_an_operation() {
    let ctx = egui::Context::default();
    let mut app = fixture(false);
    open(&mut app, &ctx).unwrap();
    control(&mut app, &json!({"placeRig":{"kind":"eye","bounds":[10,10,110,70]},"adjustGuides":false,"maskAll":true})).unwrap();
    let d = app.distort.liquify.as_ref().unwrap();
    let guide = d.rig.saved_guide().unwrap();
    let p = guide.target[2];
    let field = d.field.clone();
    let count = d.strokes.len();
    for event in [
        ToolEvent::Down { x: p[0], y: p[1], pressure: 1.0 },
        ToolEvent::Move { x: p[0], y: p[1] + 2.0, pressure: 1.0 },
        ToolEvent::Up { x: p[0], y: p[1] + 2.0 },
    ] {
        pointer(&mut app, event, egui::Modifiers::NONE);
    }
    let d = app.distort.liquify.as_ref().unwrap();
    assert_eq!(d.field, field);
    assert_eq!(d.rig.saved_guide(), Some(guide));
    assert_eq!(d.strokes.len(), count);
    assert!(!d.rig.pointer_captured());
}

#[test]
fn clearing_guides_releases_a_captured_press() {
    let ctx = egui::Context::default();
    let mut app = fixture(false);
    open(&mut app, &ctx).unwrap();
    control(&mut app, &json!({"placeRig":{"kind":"eye","bounds":[10,10,110,70]}})).unwrap();
    let p = app.distort.liquify.as_ref().unwrap().rig.saved_guide().unwrap().target[0];
    pointer(&mut app, ToolEvent::Down { x: p[0], y: p[1], pressure: 1.0 }, egui::Modifiers::SHIFT);
    let d = app.distort.liquify.as_mut().unwrap();
    assert!(d.rig.pointer_captured());
    d.rig.restore_saved(None);
    assert!(!d.rig.pointer_captured());
}

#[test]
fn new_session_rejects_an_oversized_sparse_field_before_opening_dialog() {
    let ctx = egui::Context::default();
    let mut app = fixture(false);
    app.session
        .edit("large sparse canvas", |doc, _| {
            doc.size = photocraft_geom::Size::new(100_000, 100_000);
            Ok(())
        })
        .unwrap();
    let source = app.session.active().unwrap().doc.clone();
    let error = open(&mut app, &ctx).unwrap_err();
    assert!(error.contains("session budget"));
    assert!(app.distort.liquify.is_none());
    assert_eq!(app.session.active().unwrap().doc, source);
}

#[test]
fn existing_filter_opens_at_its_saved_grid_spacing() {
    let ctx = egui::Context::default();
    let mut app = fixture(true);
    let layer = first_filter(&mut app, &ctx);
    app.session
        .execute(
            "layer.smartFilter.setParams",
            json!({
                "layer":layer.0,"index":0,"params":{"meshSize":8.0}
            }),
        )
        .unwrap();
    open_smart_filter(&mut app, &ctx, layer, 0).unwrap();
    let d = app.distort.liquify.as_ref().unwrap();
    assert_eq!(d.cell, 8.0);
    assert_eq!(d.field.cell, 8.0);
}
