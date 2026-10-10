//! Release-only, 24 MP whole-face timing and deterministic parallel replay check.
use photocraft_algo::liquify::{
    LiquifyField,
    face::{FaceSliders, FaceWarp},
    rig::{RegionKind, RigWarp},
};
use photocraft_geom::Rect;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), String> {
    let mut outline = RigWarp::new(RegionKind::Mouth, [0., -2000., 6000., 6000.])?;
    outline.source = [[1500., 2000.], [1800., 500.], [3000., 0.], [4200., 500.], [4500., 2000.], [4200., 3400.], [3000., 3800.], [1800., 3400.]];
    outline.target = outline.source;
    let face = FaceWarp {
        regions: [
            RigWarp::new(RegionKind::Eye, [800., 600., 2400., 2000.])?,
            RigWarp::new(RegionKind::Eye, [3600., 600., 5200., 2000.])?,
            RigWarp::new(RegionKind::Mouth, [2000., 1900., 4000., 3500.])?,
            RigWarp::new(RegionKind::Mouth, [2500., 1000., 3500., 2600.])?,
            outline,
        ],
        sliders: FaceSliders { eye_size: 20., smile: 30., nose_width: -15., face_width: -25., ..Default::default() },
    };
    let mut oracle: Option<LiquifyField> = None;
    for threads in [1, 4] {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().map_err(|e| e.to_string())?;
        let mut field = LiquifyField::new(Rect::new(0, 0, 6000, 4000), 4.0);
        let start = std::time::Instant::now();
        let area = pool.install(|| face.apply(&mut field))?;
        if area.is_empty() {
            return Err("Benchmark produced no face edit".into());
        }
        println!(
            "24MP threads={threads} face-edit={:.1}ms nodes={} max-displacement={:.1}px",
            start.elapsed().as_secs_f64() * 1000.,
            field.d.len(),
            field.max_displacement()
        );
        if let Some(previous) = &oracle {
            if previous != &field {
                return Err("Parallel replay differs from single-thread output".into());
            }
        } else {
            oracle = Some(field);
        }
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn main() {}
