//! Hover-to-reveal face controls. Gestures edit the same absolute sliders as the panel.
use super::{FaceSliders, FaceWarp, LiquifyDialog, apply_sliders};
use egui::{Pos2, Rect, Vec2};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Edit {
    Move,
    EyeSize,
    EyeWidth,
    EyeHeight,
    Smile,
    MouthWidth,
    UpperLip,
    LowerLip,
    NoseWidth,
    NoseHeight,
    FaceWidth,
    Chin,
    Forehead,
    Jaw,
    Turn,
}
#[derive(Clone)]
struct Handle {
    edit: Edit,
    point: [f64; 2],
    axis: [f64; 2],
    units: f64,
    label: &'static str,
}
#[derive(Clone)]
struct Drag {
    region: usize,
    handle: Handle,
    start: Pos2,
    zoom: f32,
    sliders: FaceSliders,
}
#[derive(Default)]
pub(super) struct HandleState {
    hover: Option<usize>,
    hot: Option<Edit>,
    drag: Option<Drag>,
}

pub(in super::super) fn captured(d: &LiquifyDialog) -> bool {
    d.face.handles.drag.is_some()
}

fn handles(warp: &FaceWarp, region: usize) -> Vec<Handle> {
    let Ok(regions) = warp.adjusted() else { return Vec::new() };
    let Some(r) = regions.get(region) else { return Vec::new() };
    let p = r.target;
    let width = (p[4][0] - p[0][0]).hypot(p[4][1] - p[0][1]).max(1.0);
    let mut out = Vec::new();
    let mut add = |edit, point, axis, units, label| out.push(Handle { edit, point: warp.project_point(point), axis, units, label });
    if region < 4
        && let Ok(units) = warp.move_units(region)
    {
        let center = p.iter().fold([0.0; 2], |a, p| [a[0] + p[0] / 8.0, a[1] + p[1] / 8.0]);
        let label = match region {
            0 | 1 => "Move eye: drag in any direction",
            2 => "Move mouth: drag in any direction",
            _ => "Move nose: drag in any direction",
        };
        add(Edit::Move, center, [0., 0.], units, label);
    }
    match region {
        0 | 1 => {
            let side = if region == 0 { 0 } else { 4 };
            add(Edit::EyeWidth, p[side], [if side == 0 { -1.0 } else { 1.0 }, 0.], width * 0.00075, "Eye width");
            add(Edit::EyeHeight, p[2], [0., -1.], width * 0.00045, "Eye height");
            add(Edit::EyeSize, p[6], [0., 1.], width * 0.0005, "Eye size");
        }
        2 => {
            add(Edit::Smile, p[0], [0., -1.], width * 0.0008, "Smile");
            add(Edit::MouthWidth, p[4], [1., 0.], width * 0.001, "Mouth width");
            add(Edit::UpperLip, p[2], [0., -1.], width * 0.0004, "Upper lip");
            add(Edit::LowerLip, p[6], [0., 1.], width * 0.0004, "Lower lip");
        }
        3 => {
            add(Edit::NoseWidth, p[4], [1., 0.], width * 0.001, "Nose width");
            add(Edit::NoseHeight, p[6], [0., 1.], width * 0.0006, "Nose length");
        }
        _ => {
            add(Edit::FaceWidth, p[4], [1., 0.], width * 0.0005, "Face width");
            add(Edit::Chin, p[6], [0., 1.], width * 0.0006, "Chin length");
            add(Edit::Forehead, p[2], [0., -1.], width * 0.0005, "Forehead");
            add(Edit::Jaw, p[5], [1., 0.], width * 0.0008, "Jaw width");
            if warp.depth.is_some() {
                add(Edit::Turn, p[2], [1., 0.], width / 90., "Turn head");
            }
        }
    }
    out
}

fn edited(drag: &Drag, delta: Vec2) -> FaceSliders {
    let mut s = drag.sliders.clone();
    let change = (f64::from(delta.x) * drag.handle.axis[0] + f64::from(delta.y) * drag.handle.axis[1]) / (f64::from(drag.zoom) * drag.handle.units);
    let apply = |value: &mut f64| *value = (*value + change).round().clamp(-100., 100.);
    let eye = |left: &mut f64, right: &mut Option<f64>| {
        if drag.region == 1
            && let Some(value) = right
        {
            apply(value);
        } else {
            apply(left);
        }
    };
    match drag.handle.edit {
        Edit::Move => {
            // Older saved edits can unlink eye shape before position overrides existed.
            // Keep the other eye stationary in that case, including a left-eye drag.
            if !s.eyes_linked() {
                s.right_eye_move_x.get_or_insert(s.eye_move_x);
                s.right_eye_move_y.get_or_insert(s.eye_move_y);
            }
            let change_x = f64::from(delta.x) / (f64::from(drag.zoom) * drag.handle.units);
            let change_y = f64::from(delta.y) / (f64::from(drag.zoom) * drag.handle.units);
            let move_axis = |value: &mut f64, change: f64| *value = (*value + change).clamp(-100., 100.);
            let move_eye = |left: &mut f64, right: &mut Option<f64>, change| {
                if drag.region == 1
                    && let Some(value) = right
                {
                    move_axis(value, change);
                } else {
                    move_axis(left, change);
                }
            };
            match drag.region {
                0 | 1 => {
                    move_eye(&mut s.eye_move_x, &mut s.right_eye_move_x, change_x);
                    move_eye(&mut s.eye_move_y, &mut s.right_eye_move_y, change_y);
                }
                2 => {
                    move_axis(&mut s.mouth_move_x, change_x);
                    move_axis(&mut s.mouth_move_y, change_y);
                }
                3 => {
                    move_axis(&mut s.nose_move_x, change_x);
                    move_axis(&mut s.nose_move_y, change_y);
                }
                _ => {}
            }
        }
        Edit::EyeSize => eye(&mut s.eye_size, &mut s.right_eye_size),
        Edit::EyeWidth => eye(&mut s.eye_width, &mut s.right_eye_width),
        Edit::EyeHeight => eye(&mut s.eye_height, &mut s.right_eye_height),
        Edit::Smile => apply(&mut s.smile),
        Edit::MouthWidth => apply(&mut s.mouth_width),
        Edit::UpperLip => apply(&mut s.upper_lip),
        Edit::LowerLip => apply(&mut s.lower_lip),
        Edit::NoseWidth => apply(&mut s.nose_width),
        Edit::NoseHeight => apply(&mut s.nose_height),
        Edit::FaceWidth => apply(&mut s.face_width),
        Edit::Chin => apply(&mut s.chin_height),
        Edit::Forehead => apply(&mut s.forehead),
        Edit::Jaw => apply(&mut s.jaw_width),
        Edit::Turn => {
            s.head_yaw = drag.sliders.head_yaw + f64::from(delta.x) / (f64::from(drag.zoom) * drag.handle.units);
            s.head_pitch = drag.sliders.head_pitch - f64::from(delta.y) / (f64::from(drag.zoom) * drag.handle.units);
            let length = s.head_yaw.hypot(s.head_pitch);
            if length > 30. {
                s.head_yaw *= 30. / length;
                s.head_pitch *= 30. / length;
            }
        }
    }
    s
}

fn screen(d: &LiquifyDialog, area: Rect, h: &Handle) -> Pos2 {
    let offset = if h.edit == Edit::Turn { egui::vec2(30., -36.) } else { egui::vec2(h.axis[0] as f32, h.axis[1] as f32) * 14. };
    super::super::to_screen(d, area, h.point) + offset
}

fn feature_at(warp: &FaceWarp, d: &LiquifyDialog, area: Rect, pos: Pos2, padding: f32) -> Option<usize> {
    let regions = warp.adjusted().ok()?;
    let mut nearest: Option<(usize, f32)> = None;
    for (index, r) in regions.iter().enumerate() {
        let points: Vec<_> = r.target.iter().map(|p| super::super::to_screen(d, area, warp.project_point(*p))).collect();
        let mut inside = false;
        let mut distance = f32::INFINITY;
        for (a, b) in points.iter().zip(points.iter().cycle().skip(1)).take(points.len()) {
            let edge = *b - *a;
            let t = if edge.length_sq() > 0. { ((pos - *a).dot(edge) / edge.length_sq()).clamp(0., 1.) } else { 0. };
            distance = distance.min(pos.distance(*a + edge * t));
            if (a.y > pos.y) != (b.y > pos.y) && pos.x < (b.x - a.x) * (pos.y - a.y) / (b.y - a.y) + a.x {
                inside = !inside;
            }
        }
        if index < 4 {
            let score = if inside { 0. } else { distance };
            if score <= padding && nearest.is_none_or(|(_, old)| score < old) {
                nearest = Some((index, score));
            }
        } else if nearest.is_none() && (inside || distance <= 12.) {
            return Some(index);
        }
    }
    nearest.map(|(index, _)| index)
}

/// Respect the preview's navigation modes; capture only primary face gestures.
pub(in super::super) fn interact(d: &mut LiquifyDialog, ui: &egui::Ui, area: Rect, resp: &egui::Response) {
    let blocked = !d.face.enabled
        || d.opts.view.before
        || d.face.detecting()
        || d.preview_tool != super::super::PreviewTool::Brush
        || ui.input(|i| i.key_down(egui::Key::Space) || i.pointer.middle_down());
    if blocked {
        d.face.handles = Default::default();
        return;
    }
    let Some(face) = d.face.faces.get(d.face.selected) else { return };
    let mut warp = face.warp.clone();
    warp.sliders = d.face.sliders.clone();
    let Some(pos) = ui.input(|i| i.pointer.hover_pos()) else {
        d.face.handles = Default::default();
        return;
    };
    let primary = ui.input(|i| i.pointer.primary_down());
    if let Some(drag) = d.face.handles.drag.clone() {
        let sliders = edited(&drag, pos - drag.start);
        if sliders != d.face.sliders {
            let result = apply_sliders(d, sliders);
            d.face.feedback(result);
            ui.ctx().request_repaint();
        }
        if !primary {
            d.face.handles.drag = None;
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        return;
    }
    if !area.contains(pos) {
        d.face.handles.hover = None;
        d.face.handles.hot = None;
        return;
    }
    // Keep a revealed control visible while moving out from the contour onto its handle.
    let retained = d.face.handles.hover.filter(|region| handles(&warp, *region).iter().any(|h| screen(d, area, h).distance(pos) < 22.));
    let retained_hot = retained.filter(|region| handles(&warp, *region).iter().any(|h| screen(d, area, h).distance(pos) < 9.));
    let core = feature_at(&warp, d, area, pos, 0.).filter(|index| *index < 4);
    // A nearby lip/eye edge must not keep stealing the center of the next feature.
    let center = core.filter(|region| handles(&warp, *region).iter().any(|h| h.edit == Edit::Move && screen(d, area, h).distance(pos) <= 6.));
    let region = center.or(retained_hot).or(core).or(retained).or_else(|| feature_at(&warp, d, area, pos, 14.));
    d.face.handles.hover = region;
    d.face.handles.hot = None;
    let Some(region) = region else { return };
    let controls = handles(&warp, region);
    let nearest = controls.iter().min_by(|a, b| screen(d, area, a).distance(pos).total_cmp(&screen(d, area, b).distance(pos)));
    let hot = nearest.filter(|h| screen(d, area, h).distance(pos) <= 14.);
    d.face.handles.hot = hot.map(|h| h.edit);
    if let Some(h) = hot {
        ui.ctx().set_cursor_icon(if matches!(h.edit, Edit::Turn | Edit::Move) {
            egui::CursorIcon::Move
        } else if h.axis[0] != 0. {
            egui::CursorIcon::ResizeHorizontal
        } else {
            egui::CursorIcon::ResizeVertical
        });
        if resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed()) {
            d.face.handles.drag = Some(Drag { region, handle: h.clone(), start: pos, zoom: d.zoom, sliders: d.face.sliders.clone() });
        }
    }
}

pub(in super::super) fn draw(d: &LiquifyDialog, painter: &egui::Painter, area: Rect) {
    let state = &d.face.handles;
    let Some(region) = state.drag.as_ref().map(|g| g.region).or(state.hover) else { return };
    if !d.face.enabled || d.opts.view.before {
        return;
    }
    let Some(face) = d.face.faces.get(d.face.selected) else { return };
    let mut warp = face.warp.clone();
    warp.sliders = d.face.sliders.clone();
    let t = crate::theme::Tokens::get(painter.ctx());
    if let Ok(regions) = warp.adjusted()
        && let Some(r) = regions.get(region)
    {
        let mut points: Vec<_> = r.target.iter().map(|p| super::super::to_screen(d, area, warp.project_point(*p))).collect();
        if let Some(first) = points.first().copied() {
            points.push(first);
        }
        painter.add(egui::Shape::line(points, egui::Stroke::new(1.3, t.accent)));
    }
    for h in handles(&warp, region) {
        let anchor = super::super::to_screen(d, area, h.point);
        let pos = screen(d, area, &h);
        painter.line_segment([anchor, pos], egui::Stroke::new(1.0, t.accent));
        let active = state.drag.as_ref().is_some_and(|g| g.handle.edit == h.edit) || state.hot == Some(h.edit);
        painter.circle_filled(pos, if active { 8. } else { 6. }, t.dock);
        painter.circle_stroke(pos, if active { 8. } else { 6. }, egui::Stroke::new(1.5, t.accent));
        let axis = egui::vec2(h.axis[0] as f32, h.axis[1] as f32);
        if h.edit == Edit::Move {
            for axis in [Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y] {
                painter.arrow(pos, axis * 6., egui::Stroke::new(1.2, t.accent));
            }
        } else {
            painter.arrow(pos - axis * 4., axis * 8., egui::Stroke::new(1.2, t.accent));
        }
        if h.edit == Edit::Turn {
            painter.arrow(pos + egui::vec2(0., 4.), egui::vec2(0., -8.), egui::Stroke::new(1.2, t.accent));
        }
        if active {
            let text = if h.edit == Edit::Turn { "Turn head: drag sideways / up or down" } else { h.label };
            let galley = painter.layout_no_wrap(text.into(), egui::FontId::proportional(13.), t.text);
            let mut rect = Rect::from_min_size(pos + egui::vec2(14., 14.), galley.size() + egui::vec2(14., 10.));
            rect = rect.translate(egui::vec2((area.right() - rect.right()).min(0.), (area.bottom() - rect.bottom()).min(0.)));
            painter.rect_filled(rect, t.radius_sm, t.dock);
            painter.rect_stroke(rect, t.radius_sm, egui::Stroke::new(1., t.separator), egui::StrokeKind::Inside);
            painter.galley(rect.min + egui::vec2(7., 5.), galley, t.text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_feature_center_takes_priority_over_another_features_retained_edge_handle() {
        let mut app = super::super::tests::app();
        let d = app.distort.liquify.as_mut().unwrap();
        super::super::tests::ready(d);
        d.zoom = 1.;
        d.center = [150., 150.];
        let area = Rect::from_min_size(Pos2::ZERO, egui::vec2(300., 300.));
        let warp = d.face.faces.first().unwrap().warp.clone();
        let lip = handles(&warp, 2).into_iter().find(|h| h.edit == Edit::UpperLip).unwrap();
        let nose = handles(&warp, 3).into_iter().find(|h| h.edit == Edit::Move).unwrap();
        let shift = f64::from(screen(d, area, &lip).y + 5.) - nose.point[1];
        let rig = &mut d.face.faces.first_mut().unwrap().warp.regions[3];
        rig.bounds[1] += shift;
        rig.bounds[3] += shift;
        for p in &mut rig.source {
            p[1] += shift;
        }
        rig.target = rig.source;
        let warp = d.face.faces.first().unwrap().warp.clone();
        let mouth = handles(&warp, 2).into_iter().find(|h| h.edit == Edit::Move).unwrap();
        let nose = handles(&warp, 3).into_iter().find(|h| h.edit == Edit::Move).unwrap();
        let ctx = egui::Context::default();
        crate::PhotocraftApp::setup_context(&ctx, Default::default());
        for pos in [screen(d, area, &mouth), screen(d, area, &nose)] {
            let mut out = ctx.run_ui(egui::RawInput { screen_rect: Some(area), events: vec![egui::Event::PointerMoved(pos)], ..Default::default() }, |ui| {
                let response = ui.interact(area, egui::Id::new("neighbor-center-test"), egui::Sense::click_and_drag());
                interact(d, ui, area, &response);
            });
            out.textures_delta.clear();
        }
        assert_eq!(d.face.handles.hover, Some(3));
        assert_eq!(d.face.handles.hot, Some(Edit::Move));
        assert!(d.strokes.is_empty());
    }
    #[test]
    fn center_moves_use_both_axes_are_absolute_and_respect_eye_linking_and_zoom() {
        let mut app = super::super::tests::app();
        let d = app.distort.liquify.as_mut().unwrap();
        super::super::tests::ready(d);
        let warp = d.face.faces.first().unwrap().warp.clone();
        for region in 0..4 {
            let h = handles(&warp, region).into_iter().find(|h| h.edit == Edit::Move).unwrap();
            let mut g = Drag { region, handle: h, start: Pos2::ZERO, zoom: 2., sliders: FaceSliders::default() };
            let delta = egui::vec2((g.handle.units * 40.) as f32, (g.handle.units * -60.) as f32);
            let s = edited(&g, delta);
            let (x, y) = match region {
                0 | 1 => (s.eye_move_x, s.eye_move_y),
                2 => (s.mouth_move_x, s.mouth_move_y),
                _ => (s.nose_move_x, s.nose_move_y),
            };
            assert!((x - 20.).abs() < 1e-5 && (y + 30.).abs() < 1e-5);
            assert_eq!((s.eye_size, s.nose_width, s.smile), (0., 0., 0.));
            assert_eq!(edited(&g, Vec2::ZERO), g.sliders);
            g.zoom = 1.;
            assert_eq!(edited(&g, delta * 0.5), s);
            if region == 1 {
                g.sliders.right_eye_move_x = Some(10.);
                g.sliders.right_eye_move_y = Some(-10.);
                let independent = edited(&g, delta * 0.5);
                assert_eq!((independent.eye_move_x, independent.eye_move_y), (0., 0.));
                assert!((independent.right_eye_move_x.unwrap() - 30.).abs() < 1e-5);
                assert!((independent.right_eye_move_y.unwrap() + 40.).abs() < 1e-5);
                let limited = edited(&g, egui::vec2(10000., -10000.));
                assert_eq!((limited.right_eye_move_x, limited.right_eye_move_y), (Some(100.), Some(-100.)));
            }
        }
    }

    #[test]
    fn center_move_respects_unlinked_eyes_from_older_shape_only_edits() {
        let h = Handle { edit: Edit::Move, point: [0.; 2], axis: [0.; 2], units: 1., label: "Move eye" };
        let mut g = Drag {
            region: 0,
            handle: h,
            start: Pos2::ZERO,
            zoom: 1.,
            sliders: FaceSliders { right_eye_size: Some(20.), eye_move_x: 5., eye_move_y: -5., ..Default::default() },
        };
        let s = edited(&g, egui::vec2(10., -20.));
        assert_eq!((s.eye_move_x, s.eye_move_y), (15., -25.));
        assert_eq!((s.right_eye_move_x, s.right_eye_move_y), (Some(5.), Some(-5.)));
        g.region = 1;
        let s = edited(&g, egui::vec2(10., -20.));
        assert_eq!((s.eye_move_x, s.eye_move_y), (5., -5.));
        assert_eq!((s.right_eye_move_x, s.right_eye_move_y), (Some(15.), Some(-25.)));
        assert_eq!(s.right_eye_size, Some(20.));
    }

    #[test]
    fn hovering_each_feature_center_captures_a_two_axis_move_without_brush_strokes() {
        for region in 0..4 {
            let mut app = super::super::tests::app();
            let d = app.distort.liquify.as_mut().unwrap();
            super::super::tests::ready(d);
            d.zoom = 1.;
            d.center = [150., 150.];
            let area = Rect::from_min_size(Pos2::ZERO, egui::vec2(300., 300.));
            let ctx = egui::Context::default();
            crate::PhotocraftApp::setup_context(&ctx, Default::default());
            let frame = |d: &mut LiquifyDialog, events| {
                let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(area), events, ..Default::default() }, |ui| {
                    let response = ui.interact(area, egui::Id::new("center-move-test"), egui::Sense::click_and_drag());
                    interact(d, ui, area, &response);
                    draw(d, ui.painter(), area);
                });
                output.textures_delta.clear();
            };
            frame(d, vec![]);
            let warp = d.face.faces.first().unwrap().warp.clone();
            let h = handles(&warp, region).into_iter().find(|h| h.edit == Edit::Move).unwrap();
            let p = screen(d, area, &h);
            frame(d, vec![egui::Event::PointerMoved(p)]);
            assert_eq!(d.face.handles.hover, Some(region));
            assert_eq!(d.face.handles.hot, Some(Edit::Move));
            let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
            frame(d, vec![button(p, true)]);
            assert!(captured(d));
            let end = p + egui::vec2(2., -2.);
            frame(d, vec![egui::Event::PointerMoved(end)]);
            frame(d, vec![button(end, false)]);
            assert!(!captured(d));
            assert_eq!(d.strokes.len(), 1);
            assert_eq!(d.strokes[0].tool, photocraft_algo::liquify::LiquifyTool::FaceWarpV1);
            let s = &d.face.sliders;
            let (x, y) = match region {
                0 | 1 => (s.eye_move_x, s.eye_move_y),
                2 => (s.mouth_move_x, s.mouth_move_y),
                _ => (s.nose_move_x, s.nose_move_y),
            };
            assert!(x > 0. && y < 0., "region {region}: {:?}", d.face.status);
            assert_eq!((s.eye_size, s.smile, s.nose_width), (0., 0., 0.));
            apply_sliders(d, FaceSliders::default()).unwrap();
            assert!(d.field.is_identity());
        }
    }
    #[test]
    fn head_turn_and_forehead_handles_remain_separate_at_every_zoom() {
        let mut app = super::super::tests::app();
        let d = app.distort.liquify.as_mut().unwrap();
        super::super::tests::ready(d);
        let mut warp = d.face.faces.first().unwrap().warp.clone();
        warp.depth = Some(Box::new(photocraft_algo::liquify::face::pose::FaceDepth { bounds: [0., 0., 300., 300.], samples: [[-30.; 25]; 25] }));
        let controls = handles(&warp, 4);
        let turn = controls.iter().find(|h| h.edit == Edit::Turn).unwrap();
        let forehead = controls.iter().find(|h| h.edit == Edit::Forehead).unwrap();
        let area = Rect::from_min_size(Pos2::ZERO, egui::vec2(600., 600.));
        for zoom in [0.5, 1., 2., 4.] {
            d.zoom = zoom;
            assert!(screen(d, area, turn).distance(screen(d, area, forehead)) > 20.);
        }
    }
    #[test]
    fn hover_reveals_one_feature_and_drag_updates_only_face_edit_with_navigation_priority() {
        let mut app = super::super::tests::app();
        let d = app.distort.liquify.as_mut().unwrap();
        super::super::tests::ready(d);
        d.zoom = 1.;
        d.center = [150., 150.];
        let area = Rect::from_min_size(Pos2::ZERO, egui::vec2(300., 300.));
        let ctx = egui::Context::default();
        crate::PhotocraftApp::setup_context(&ctx, Default::default());
        let frame = |d: &mut LiquifyDialog, events| {
            let input = egui::RawInput { screen_rect: Some(area), events, ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| {
                let response = ui.interact(area, egui::Id::new("face-gesture-test"), egui::Sense::click_and_drag());
                interact(d, ui, area, &response);
                draw(d, ui.painter(), area);
            });
            output.textures_delta.clear();
        };
        frame(d, vec![]);
        let warp = d.face.faces.first().unwrap().warp.clone();
        let h = handles(&warp, 0).into_iter().find(|h| h.edit == Edit::EyeWidth).unwrap();
        let p = screen(d, area, &h);
        frame(d, vec![egui::Event::PointerMoved(p)]);
        assert_eq!(d.face.handles.hover, Some(0));
        assert_eq!(d.face.handles.hot, Some(Edit::EyeWidth));
        assert!(d.strokes.is_empty());
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        frame(d, vec![button(p, true)]);
        assert!(captured(d));
        let end = p - egui::vec2(5., 0.);
        frame(d, vec![egui::Event::PointerMoved(end)]);
        assert!(d.face.sliders.eye_width > 0.);
        assert_eq!(d.strokes.len(), 1);
        assert_eq!(d.strokes[0].tool, photocraft_algo::liquify::LiquifyTool::FaceWarpV1);
        frame(d, vec![button(end, false)]);
        assert!(!captured(d));
        let field = d.field.clone();
        for tool in [super::super::super::PreviewTool::Hand, super::super::super::PreviewTool::Zoom] {
            d.preview_tool = tool;
            frame(d, vec![egui::Event::PointerMoved(p), button(p, true)]);
            assert!(!captured(d));
            assert_eq!(d.face.handles.hover, None);
            frame(d, vec![button(p, false)]);
            assert_eq!(d.field, field);
        }
        d.preview_tool = super::super::super::PreviewTool::Brush;
        d.opts.view.before = true;
        frame(d, vec![egui::Event::PointerMoved(p), button(p, true)]);
        assert!(!captured(d));
        assert_eq!(d.field, field);
    }
    #[test]
    fn direct_eye_drag_respects_linking_and_is_absolute_and_zoom_independent() {
        let h = Handle { edit: Edit::EyeWidth, point: [0.; 2], axis: [1., 0.], units: 1., label: "Eye width" };
        let mut g = Drag { region: 1, handle: h, start: Pos2::ZERO, zoom: 2., sliders: FaceSliders::default() };
        assert_eq!(edited(&g, egui::vec2(60., 0.)).eye_width, 30.);
        assert_eq!(edited(&g, egui::vec2(60., 0.)).right_eye_width, None);
        g.sliders.right_eye_width = Some(10.);
        let s = edited(&g, egui::vec2(60., 0.));
        assert_eq!(s.eye_width, 0.);
        assert_eq!(s.right_eye_width, Some(40.));
        g.zoom = 1.;
        assert_eq!(edited(&g, egui::vec2(30., 0.)), s);
        assert_eq!(edited(&g, Vec2::ZERO), g.sliders);
        assert_eq!(edited(&g, egui::vec2(10000., 0.)).right_eye_width, Some(100.));
    }
    #[test]
    fn direct_turn_uses_both_axes_and_caps_combined_rotation() {
        let h = Handle { edit: Edit::Turn, point: [0.; 2], axis: [1., 0.], units: 1., label: "Turn head" };
        let g = Drag { region: 4, handle: h, start: Pos2::ZERO, zoom: 1., sliders: FaceSliders::default() };
        let s = edited(&g, egui::vec2(10., -5.));
        assert_eq!((s.head_yaw, s.head_pitch), (10., 5.));
        let s = edited(&g, egui::vec2(100., 100.));
        assert!((s.head_yaw.hypot(s.head_pitch) - 30.).abs() < 1e-10);
        assert!(s.validate().is_ok());
    }
}
