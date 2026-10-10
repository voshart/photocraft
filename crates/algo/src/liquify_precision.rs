// SPDX-License-Identifier: MIT OR Apache-2.0
//! Precision Liquify helpers. Geometry profiles are serialized as tool variants; preview
//! policy never changes stroke replay. No new dependencies and no inference runtime.

use photocraft_geom::Rect;
use photocraft_raster::Surface;

/// Continuous compact-support profile with a flat core and C2 shoulder.
/// `softness` is 0..1. Even its hardest setting keeps a 15%-radius shoulder,
/// avoiding the discontinuous top-hat boundary of the legacy brush.
/// Invalid inputs have no influence rather than introducing NaNs into a field.
pub fn smooth_falloff(radius_fraction: f64, softness: f64) -> f64 {
    if !radius_fraction.is_finite() || !softness.is_finite() || !(0.0..1.0).contains(&radius_fraction) {
        return 0.0;
    }
    let core = 0.85 * (1.0 - softness.clamp(0.0, 1.0));
    if radius_fraction <= core {
        return 1.0;
    }
    let t = (radius_fraction - core) / (1.0 - core);
    // Evaluate from the outer edge to avoid cancellation close to radius == 1.
    let u = 1.0 - t;
    (u * u * u * (10.0 + u * (-15.0 + 6.0 * u))).clamp(0.0, 1.0)
}

/// Writes only the requested portion of a fully rendered tile. Computing whole
/// tiles keeps exactly the same source footprint/clamping as the final renderer.
/// This is not an alternative interpolation implementation.
pub(super) fn write_requested_tile(out: &mut Surface, tile: Rect, requested: Rect, pixels: &[f32]) {
    let crop = tile.intersect(&requested);
    if crop.is_empty() {
        return;
    }
    let channels = out.format().channels();
    let Some(stride) = (tile.width() as usize).checked_mul(channels) else { return };
    let Some(expected) = stride.checked_mul(tile.height() as usize) else { return };
    if stride == 0 || pixels.len() != expected {
        return;
    }
    if crop == tile {
        out.write_region(tile, pixels);
        return;
    }
    let left = (crop.x0 - tile.x0) as usize * channels;
    let row_len = crop.width() as usize * channels;
    for (index, row) in pixels.chunks_exact(stride).enumerate() {
        let y = i64::from(tile.y0) + index as i64;
        if y < i64::from(crop.y0) || y >= i64::from(crop.y1) {
            continue;
        }
        if let Some(values) = row.get(left..left + row_len) {
            out.write_region(Rect::new(crop.x0, y as i32, crop.x1, y as i32 + 1), values);
        }
    }
}

/// Preview equivalent of the raster engine's selection blend, restricted to a
/// small region. Work in the surface's native channels and premultiplied alpha;
/// conversion to display RGBA belongs to the UI, after this operation.
pub fn mix_selection_region(original: &Surface, warped: &mut Surface, selection: &Surface, region: Rect) {
    if region.is_empty() || original.format() != warped.format() {
        return;
    }
    let format = original.format();
    let channels = format.channels();
    if channels == 0 {
        return;
    }
    for y in region.y0..region.y1 {
        let rect = Rect::new(region.x0, y, region.x1, y + 1);
        let old = original.read_region(rect);
        let mut new = warped.read_region(rect);
        for (x, (a, b)) in old.chunks_exact(channels).zip(new.chunks_exact_mut(channels)).enumerate() {
            let coverage = selection.sample_channel(region.x0 + x as i32, y, 0).clamp(0.0, 1.0);
            if format.alpha {
                let Some((&old_alpha, old_color)) = a.split_last() else { continue };
                let Some((new_alpha, new_color)) = b.split_last_mut() else { continue };
                let alpha = old_alpha + (*new_alpha - old_alpha) * coverage;
                for (o, n) in old_color.iter().zip(new_color.iter_mut()) {
                    let premultiplied = *o * old_alpha + (*n * *new_alpha - *o * old_alpha) * coverage;
                    *n = if alpha > 0.0 { (premultiplied / alpha).clamp(0.0, 1.0) } else { 0.0 };
                }
                *new_alpha = alpha;
            } else {
                for (o, n) in a.iter().zip(b.iter_mut()) {
                    *n = *o + (*n - *o) * coverage;
                }
            }
        }
        warped.write_region(rect, &new);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{LiquifyField, LiquifyStroke, LiquifyTool, apply_liquify, apply_liquify_region};
    use super::*;
    use photocraft_color::{ColorMode, PixelFormat, SampleType};

    #[test]
    fn shoulder_is_bounded_monotonic_and_reaches_zero() {
        for softness in [0.0, 0.25, 0.5, 1.0] {
            assert_eq!(smooth_falloff(0.0, softness), 1.0);
            assert_eq!(smooth_falloff(1.0, softness), 0.0);
            assert_eq!(smooth_falloff(2.0, softness), 0.0);
            let mut previous = 1.0;
            for i in 0..=10_000 {
                let v = smooth_falloff(f64::from(i) / 10_000.0, softness);
                assert!((0.0..=1.0).contains(&v));
                assert!(v <= previous + 1e-12);
                previous = v;
            }
            assert!(smooth_falloff(1.0 - 1e-6, softness) < 1e-12);
        }
    }

    #[test]
    fn invalid_profile_inputs_do_not_poison_the_mapping() {
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(smooth_falloff(v, 0.5), 0.0);
            assert_eq!(smooth_falloff(0.5, v), 0.0);
        }
        assert_eq!(smooth_falloff(-1.0, 0.5), 0.0);
    }

    fn fixture(depth: SampleType) -> (Surface, Rect) {
        let bounds = Rect::new(0, 0, 96, 80);
        let mut source = Surface::new(PixelFormat::new(ColorMode::Rgb, depth, true));
        for y in 0..80 {
            for x in 0..96 {
                source.write_pixel(x, y, &[x as f32 / 96.0, y as f32 / 80.0, ((x / 4 + y / 4) % 2) as f32, 0.75]);
            }
        }
        (source, bounds)
    }

    fn stroke(tool: LiquifyTool) -> LiquifyStroke {
        let mut stroke = LiquifyStroke::new(tool, 50.0);
        stroke.points = vec![vec![40.0, 35.0], vec![48.0, 39.0], vec![54.0, 36.0]];
        stroke
    }

    #[test]
    fn precision_is_a_separate_serialized_tool_not_a_legacy_reinterpretation() {
        let legacy: LiquifyStroke = serde_json::from_str(r#"{"tool":"forwardWarp","size":50,"points":[[40,35],[48,39],[54,36]]}"#).unwrap();
        assert_eq!(legacy.tool, LiquifyTool::ForwardWarp);
        let new = stroke(LiquifyTool::PrecisionWarp);
        let encoded = serde_json::to_string(&new).unwrap();
        assert!(encoded.contains("precisionWarp"));
        assert_eq!(serde_json::from_str::<LiquifyStroke>(&encoded).unwrap(), new);
        let bounds = Rect::new(0, 0, 96, 80);
        let a = LiquifyField::from_strokes(bounds, 2.0, &[legacy]);
        let b = LiquifyField::from_strokes(bounds, 2.0, &[new]);
        assert_ne!(a, b);
    }

    #[test]
    fn partial_render_matches_commit_and_does_not_change_other_pixels_at_all_depths() {
        for depth in [SampleType::U8, SampleType::U16, SampleType::F32] {
            let (source, bounds) = fixture(depth);
            let field = LiquifyField::from_strokes(bounds, 2.0, &[stroke(LiquifyTool::PrecisionWarp)]);
            let full = apply_liquify(&source, &field);
            for region in [Rect::new(25, 20, 63, 52), Rect::new(0, 0, 11, 9), Rect::new(85, 69, 96, 80)] {
                let partial = apply_liquify_region(&source, &field, region);
                assert_eq!(partial.read_region(region), full.read_region(region));
                for y in 0..80 {
                    for x in 0..96 {
                        if x < region.x0 || x >= region.x1 || y < region.y0 || y >= region.y1 {
                            let pixel = Rect::new(x, y, x + 1, y + 1);
                            assert_eq!(partial.read_region(pixel), source.read_region(pixel));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn identity_and_outside_crop_are_exact_noops() {
        let (source, bounds) = fixture(SampleType::U16);
        let identity = LiquifyField::new(bounds, 2.0);
        assert_eq!(apply_liquify_region(&source, &identity, bounds), source);
        let field = LiquifyField::from_strokes(bounds, 2.0, &[stroke(LiquifyTool::PrecisionWarp)]);
        assert_eq!(apply_liquify_region(&source, &field, Rect::new(200, 200, 210, 210)), source);
    }

    #[test]
    fn precision_push_does_not_dab_when_held_still_and_respects_freeze() {
        let (_, bounds) = fixture(SampleType::U8);
        let mut field = LiquifyField::new(bounds, 2.0);
        let s = stroke(LiquifyTool::PrecisionWarp);
        field.stroke_begin(&s, [40.0, 35.0, 1.0]);
        assert!(field.is_identity());
        field.freeze.fill(1.0);
        field.apply_stroke(&s);
        assert!(field.is_identity());
    }

    #[test]
    fn zero_selection_preserves_original() {
        let (source, bounds) = fixture(SampleType::U16);
        let field = LiquifyField::from_strokes(bounds, 2.0, &[stroke(LiquifyTool::PrecisionWarp)]);
        let mut warped = apply_liquify(&source, &field);
        mix_selection_region(&source, &mut warped, &Surface::new(PixelFormat::GRAY8), bounds);
        assert_eq!(warped.read_region(bounds), source.read_region(bounds));
    }
}
